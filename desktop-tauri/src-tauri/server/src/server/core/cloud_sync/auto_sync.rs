//! 自动同步：本地数据变了 → 防抖合并 → 按实质变化自动上传。
//!
//! ── 触发源为什么分成两路 ────────────────────────────────────
//! 业务数据落在两类地方：
//!   · **三张表**（`accounts` / `account_usage_records` / `checkin_records`）——
//!     用 SQLite 的 `update_hook` 监听。这是最可靠的一路：不管谁写的（账号
//!     增删改、token 刷新、签到记账、余额查询），钩子都会响。
//!   · **kv 键**（配置、Agent 配置、代理池 / 订阅）—— 走**显式通知**，不用钩子。
//!
//! ── kv 为什么不能用 update_hook ─────────────────────────────
//! `kv` 表同时承载 `logsNextId` 这类**每秒都在变**的本机状态，钩子必须能分辨
//! 「改的是哪个键」。而 `update_hook` 的回调是在 SQLite 执行语句**期间**被调用
//! 的（此时连接锁已被 `Db::with` 持有），在回调里再查一次库属于重入 —— 轻则
//! 拖慢每一次日志写入，重则死锁。所以 kv 这一路改为在**写入点**显式调用
//! [`notify_change`]：`config::update`（配置项，逐键比对同步范围）、
//! `agents::save_client`、`proxy_pool::write_items`、`subscriptions::write_items`、
//! `proxy_settings::write_raw` 五处。代价是将来新增一个「同步范围内的 kv 写入点」
//! 时要记得加一行通知 —— 这条约束写在 `snapshot::is_synced_kv_key` 的注释里。
//!
//! ── 为什么要防抖 + 上限 ────────────────────────────────────
//! 一次「添加账号」会连着写好几行（账号 + 用量记录），token 刷新也可能在几秒内
//! 连写两三次。每个信号都上传一次等于把同一个快照重复传 N 遍。策略与 cc-switch
//! 一致：**收到第一个信号后等 1 秒**，期间的新信号合并；若信号连续不断，最多
//! 等 10 秒就必须传一次（否则高频写入会让同步永远不触发）。
//!
//! ── 为什么上传前还要比内容哈希 ──────────────────────────────
//! 防抖只能合并「短时间内的多次写」。真正的问题是：`update_hook` 会在**内容
//! 没变**的写入上响（例如 token 刷新写回同样的值）。每次上传都是几十 KB 到
//! 几 MB 的外网请求，长期跑下来既费流量又可能撞上云盘限流。所以上传前先构建
//! 快照、算哈希：与 `lastLocalHash` 相同**且**密钥指纹、远端目标也没变 → 直接
//! 跳过，一个请求都不发。这条判断也让「刚下载完」不会立刻回传（下载后写库会
//! 触发钩子，但内容与远端一致 → 跳过）。抑制守卫因此是第二道保险，不是唯一依赖。
//!
//! ── 最小间隔与失败退避 ──────────────────────────────────────
//! 防抖只管「一次写入风暴」，管不住「每隔几秒写一次」的长期频率；最小间隔把
//! 两次上传拉开到用户设定的分钟数。失败按 60 秒基数逐次翻倍、指数部分封顶
//! 1 小时，且最终不小于用户间隔，避免网络故障时把请求打成一串。到期时间、
//! 目标身份和连续失败次数都落在状态里：换目标后旧指纹与旧退避一起作废，
//! 进程启动与抑制结束时各补查一次，避免重启或下载期间丢掉待办变更。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use zeroize::Zeroizing;

use crate::server::core::crypto::{self, SecretScope};
use crate::server::db::Db;
use crate::server::logging;

use super::{settings, snapshot};

/// 第一个信号之后的等待窗口（合并窗口内到达的信号不再单独上传）
const DEBOUNCE_MS: u64 = 1_000;
/// 连续变更的合并上限，不替代两次上传之间的最小间隔。
const MAX_WAIT_MS: u64 = 10_000;
/// 失败退避的基数与指数部分的上限。
const RETRY_BASE_MS: i64 = 60_000;
const RETRY_MAX_MS: i64 = 3_600_000;

static CHANGE_TX: OnceLock<Sender<()>> = OnceLock::new();
static SUPPRESS_DEPTH: AtomicUsize = AtomicUsize::new(0);

/// 抑制守卫释放补查的次数。只在测试里读，用来断言「结束补查」确实发生。
#[cfg(test)]
static RECHECK_CALLS: AtomicUsize = AtomicUsize::new(0);

/// 抑制守卫：作用域内不触发自动上传（下载应用期间用）。
///
/// 用计数而不是布尔：将来若有嵌套场景，布尔会让内层的 `drop` 提前解除抑制。
pub(crate) struct SuppressionGuard;

impl SuppressionGuard {
    pub(crate) fn new() -> Self {
        SUPPRESS_DEPTH.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for SuppressionGuard {
    fn drop(&mut self) {
        let previous = SUPPRESS_DEPTH.try_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
            Some(value.saturating_sub(1))
        });
        if previous == Ok(1) {
            #[cfg(test)]
            RECHECK_CALLS.fetch_add(1, Ordering::SeqCst);
            // 抑制期间也可能有真正的本地写入，结束后补查一次，不能丢掉这些变更。
            notify_change();
        }
    }
}

fn is_suppressed() -> bool {
    SUPPRESS_DEPTH.load(Ordering::SeqCst) > 0
}

/// 这个表名要不要触发自动同步（唯一判定，钩子与测试共用）
pub(crate) fn should_trigger_for_table(table: &str) -> bool {
    snapshot::SYNCED_TABLES.contains(&table)
}

/// 发一个「本地数据变了」的信号。非阻塞：通道满（已有待处理信号）时直接丢弃 ——
/// 我们要的是「脏了」这件事，不是每一次变化。
///
/// **可以在持有数据库连接锁的钩子里调用**：不做 IO、不打日志、不取锁。
pub(crate) fn notify_change() {
    if is_suppressed() {
        return;
    }
    let Some(tx) = CHANGE_TX.get() else {
        return;
    };
    match tx.try_send(()) {
        Ok(()) => {}
        // 缓冲 1：满了说明已有一个待处理信号，本次合并进去即可
        Err(TrySendError::Full(_)) | Err(TrySendError::Closed(_)) => {}
    }
}

/// SQLite `update_hook` 的回调入口。
///
/// ── 硬约束：这里必须极快 ────────────────────────────────────
/// 钩子在语句执行期间被调用（连接锁已被持有）。因此只做一次数组查找 + 一次
/// 非阻塞 send，**绝不**查库、不打日志（`logging::log` 要写同一个库 → 死锁）。
pub(crate) fn on_table_change(table: &str) {
    if should_trigger_for_table(table) {
        notify_change();
    }
}

/// 启动自动同步工作线程（`bootstrap` 里调一次）。
///
/// ── 为什么线程里现读设置 ────────────────────────────────────
/// 用户在设置页改开关后要立刻生效，不必重启。工作线程每轮都从
/// `settings::load()` 读最新值（一次 kv 查询，频率是「有信号时」）。
pub(crate) fn spawn_worker(db: Db) {
    if CHANGE_TX.get().is_some() {
        return;
    }
    let (tx, rx) = channel::<()>(1);
    if CHANGE_TX.set(tx).is_err() {
        return;
    }
    crate::spawn_task(async move {
        run_worker(db, rx).await;
    });
    // 上次退出前可能有没传完的变更，启动就补查一次。
    notify_change();
}

async fn run_worker(db: Db, mut rx: Receiver<()>) {
    while rx.recv().await.is_some() {
        let started = Instant::now();
        // 防抖：等到「安静 1 秒」或「总共 10 秒」，以先到者为准
        loop {
            let elapsed = started.elapsed();
            if elapsed >= Duration::from_millis(MAX_WAIT_MS) {
                break;
            }
            let remaining = Duration::from_millis(MAX_WAIT_MS) - elapsed;
            let wait = Duration::from_millis(DEBOUNCE_MS).min(remaining);
            match tokio::time::timeout(wait, rx.recv()).await {
                Ok(Some(())) => continue,
                // 通道关闭 = 进程在退出
                Ok(None) => return,
                // 安静窗口到了
                Err(_) => break,
            }
        }
        let mut retry = None;
        loop {
            if !wait_until_ready(&mut rx, retry.as_ref()).await {
                break;
            }
            match run_once(&db).await {
                RunOutcome::Complete => break,
                RunOutcome::Deferred => retry = None,
                RunOutcome::Retry(delay) => retry = Some(delay),
            }
        }
    }
}

struct RetryDelay {
    at: i64,
    target_id: String,
    last_sync_at: Option<i64>,
}

/// 本轮的最早可传时刻。失败退避只在「目标没换、期间也没成功同步过」时继续生效。
fn due_at(settings: &settings::CloudSyncSettings, retry: Option<&RetryDelay>) -> i64 {
    let retry_floor = retry
        .filter(|delay| {
            delay.target_id == settings.target_id()
                && delay.last_sync_at == settings.status.last_sync_at
        })
        .map(|delay| delay.at)
        .unwrap_or(0);
    settings.auto_sync_due_at().max(retry_floor)
}

async fn wait_until_ready(rx: &mut Receiver<()>, retry: Option<&RetryDelay>) -> bool {
    loop {
        let settings = settings::load();
        if !settings.enabled || !settings.auto_sync {
            return false;
        }
        let now = logging::now_ms();
        let due = due_at(&settings, retry);
        if due <= now {
            return true;
        }
        let wait_ms = due.saturating_sub(now).clamp(1, 30_000) as u64;
        match tokio::time::timeout(Duration::from_millis(wait_ms), rx.recv()).await {
            Ok(Some(())) | Err(_) => {}
            Ok(None) => return false,
        }
    }
}

/// 连续失败的退避：60 秒起、逐次翻倍、封顶 1 小时，且不小于用户设的间隔。
fn backoff_ms(failures: u32, interval_minutes: u32) -> i64 {
    RETRY_BASE_MS
        .saturating_mul(1i64 << failures.saturating_sub(1).min(6))
        .min(RETRY_MAX_MS)
        .max(i64::from(interval_minutes) * 60_000)
}

enum RunOutcome {
    Complete,
    Deferred,
    Retry(RetryDelay),
}

async fn run_once(db: &Db) -> RunOutcome {
    let _guard = super::sync_lock().lock().await;
    let settings = settings::load();
    if !settings.enabled || !settings.auto_sync {
        return RunOutcome::Complete;
    }
    if is_suppressed() || due_at(&settings, None) > logging::now_ms() {
        return RunOutcome::Deferred;
    }
    match upload_if_changed(db, &settings).await {
        Ok(()) => RunOutcome::Complete,
        Err(error) => {
            let failures = settings.status.consecutive_failures.saturating_add(1);
            let backoff = backoff_ms(failures, settings.auto_sync_interval_minutes);
            let now = logging::now_ms();
            let retry_at = now.saturating_add(backoff);
            let saved = settings::update_status_for(&settings, |status| {
                status.record_auto_failure(now, &error, failures, retry_at);
            });
            let detail = saved
                .err()
                .map(|cause| format!("；同步状态未保存：{cause}"))
                .unwrap_or_default();
            logging::log_with_level(
                "[CloudSync]",
                &format!("⚠️ 云同步（自动）失败：{error}{detail}"),
                "warn",
            );
            RunOutcome::Retry(RetryDelay {
                at: retry_at,
                target_id: settings.target_id(),
                last_sync_at: settings.status.last_sync_at,
            })
        }
    }
}

async fn upload_if_changed(
    db: &Db,
    settings: &settings::CloudSyncSettings,
) -> Result<(), String> {
    settings.validate()?;
    // 快照、哈希、密钥同一次准备完再交给上传，避免排队等网络期间换了另一把密钥。
    let snapshot = snapshot::build(db)?;
    let key = crypto::backup_key(SecretScope::Accounts)?.map(Zeroizing::new);
    let content_hash = snapshot.content_hash(key.as_deref())?;
    let key_fingerprint = key.as_deref().map(|bytes| crypto::fingerprint_of(bytes));
    let target_id = settings.target_id();
    if settings
        .status
        .should_skip_upload(&content_hash, key_fingerprint.as_deref(), &target_id)
    {
        if settings.status.has_failure_state() {
            settings::update_status_for(settings, |status| status.clear_failure())?;
        }
        return Ok(());
    }
    super::upload_locked(settings, snapshot, content_hash, key).await?;
    logging::verbose("[CloudSync]", "自动同步：已上传本地变更");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 两个用例都改全局抑制计数，必须串行，否则互相读到对方的深度。
    static GUARD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn only_business_tables_trigger() {
        assert!(should_trigger_for_table("accounts"));
        assert!(should_trigger_for_table("checkin_records"));
        assert!(should_trigger_for_table("account_usage_records"));
        assert!(!should_trigger_for_table("logs"));
        assert!(!should_trigger_for_table("requests"));
        assert!(!should_trigger_for_table("debug_traffic"));
        assert!(!should_trigger_for_table("kv"));
    }

    #[test]
    fn suppression_guard_is_scoped() {
        let _serial = GUARD_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        assert!(!is_suppressed());
        {
            let _guard = SuppressionGuard::new();
            assert!(is_suppressed());
            {
                let _nested = SuppressionGuard::new();
                assert!(is_suppressed());
            }
            assert!(is_suppressed(), "内层释放后外层仍应抑制");
        }
        assert!(!is_suppressed());
    }

    #[test]
    fn suppression_guard_end_requests_one_recheck() {
        let _serial = GUARD_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let before = RECHECK_CALLS.load(Ordering::SeqCst);
        {
            let outer = SuppressionGuard::new();
            {
                let inner = SuppressionGuard::new();
                drop(inner);
                assert_eq!(
                    RECHECK_CALLS.load(Ordering::SeqCst),
                    before,
                    "内层释放不该补查"
                );
            }
            drop(outer);
        }
        assert_eq!(
            RECHECK_CALLS.load(Ordering::SeqCst),
            before + 1,
            "最外层释放后必须补查一次，不能丢掉抑制期间的本地写入"
        );
    }

    #[test]
    fn backoff_grows_from_60s_and_caps_at_one_hour() {
        // 用户间隔取 1 分钟，才看得到 60 秒起、逐次翻倍的指数项本身。
        let expected = [
            60_000, 120_000, 240_000, 480_000, 960_000, 1_920_000, 3_600_000, 3_600_000,
        ];
        for (index, want) in expected.iter().enumerate() {
            let failures = index as u32 + 1;
            assert_eq!(backoff_ms(failures, 1), *want, "第 {failures} 次失败");
        }
        assert_eq!(backoff_ms(u32::MAX, 1), RETRY_MAX_MS);
    }

    #[test]
    fn backoff_never_shorter_than_user_interval() {
        // 指数项封顶 1 小时；但用户把间隔设得更大时，最终值仍以用户间隔为下限。
        assert_eq!(backoff_ms(1, 1), 60_000);
        assert_eq!(
            backoff_ms(1, 5),
            5 * 60_000,
            "默认 5 分钟间隔下首次退避也不能短于它"
        );
        assert_eq!(backoff_ms(8, 5), RETRY_MAX_MS);
        assert_eq!(backoff_ms(8, 120), 120 * 60_000);
        assert_eq!(backoff_ms(1, 1440), 1440 * 60_000);
    }

    #[test]
    fn consecutive_failures_drive_exponential_retry_state() {
        let mut status = settings::SyncStatus::default();
        let mut now = 1_000_000_i64;
        let expected = [
            60_000, 120_000, 240_000, 480_000, 960_000, 1_920_000, 3_600_000, 3_600_000,
        ];
        for want in expected {
            let failures = status.consecutive_failures.saturating_add(1);
            let backoff = backoff_ms(failures, 1);
            assert_eq!(backoff, want);
            let retry_at = now.saturating_add(backoff);
            status.record_auto_failure(now, "远端失败", failures, retry_at);
            assert!(status.has_failure_state());
            assert_eq!(status.retry_at, Some(retry_at));
            assert_eq!(status.consecutive_failures, failures);
            now = retry_at;
        }
        assert_eq!(status.consecutive_failures, 8);
    }

    #[test]
    fn due_at_defers_within_min_interval_window() {
        let mut settings = settings::CloudSyncSettings::default();
        settings.auto_sync_interval_minutes = 5;
        settings.status.last_upload_at = Some(1_000_000);
        let due = due_at(&settings, None);
        assert_eq!(due, 1_000_000 + 5 * 60_000);
        assert!(due > 1_000_000, "最小间隔窗口内必须延后，不能立即再传");

        settings.status.retry_at = Some(9_000_000);
        assert_eq!(due_at(&settings, None), 9_000_000, "退避时刻更晚时以它为准");
    }

    #[test]
    fn retry_floor_is_dropped_after_target_change_or_new_sync() {
        let settings = settings::CloudSyncSettings::default();
        let retry = RetryDelay {
            at: 9_000_000,
            target_id: settings.target_id(),
            last_sync_at: settings.status.last_sync_at,
        };
        assert_eq!(
            due_at(&settings, Some(&retry)),
            9_000_000,
            "目标与同步时间都没变，沿用退避"
        );

        let mut moved = settings.clone();
        moved.webdav.base_url = "https://other.example.com/dav".to_string();
        assert!(
            due_at(&moved, Some(&retry)) < 9_000_000,
            "换目标后旧退避必须失效，否则新目标会被拖到 1 小时后才首次上传"
        );

        let mut newer = settings.clone();
        newer.status.last_sync_at = Some(1);
        assert!(
            due_at(&newer, Some(&retry)) < 9_000_000,
            "期间成功同步过，旧退避也该失效"
        );
    }

    #[test]
    fn webdav_and_s3_failures_are_recorded_as_auto_failures() {
        for error in ["WebDAV 503", "S3 连接超时"] {
            let mut status = settings::SyncStatus::default();
            let failures = status.consecutive_failures.saturating_add(1);
            let retry_at = 1_000_000 + backoff_ms(failures, 5);
            status.record_auto_failure(1_000_000, error, failures, retry_at);
            assert_eq!(status.last_error.as_deref(), Some(error));
            assert_eq!(status.last_error_source.as_deref(), Some("auto"));
            assert_eq!(status.consecutive_failures, 1);
            assert_eq!(status.retry_at, Some(retry_at));
        }
    }

    #[test]
    fn success_after_failure_clears_retry_state() {
        let mut status = settings::SyncStatus::default();
        status.record_auto_failure(1_000_000, "WebDAV 503", 2, 1_120_000);
        status.clear_failure();
        assert!(!status.has_failure_state());
        assert_eq!(status.retry_at, None);
        assert_eq!(status.consecutive_failures, 0);
        assert_eq!(status.last_error, None);
    }
}
