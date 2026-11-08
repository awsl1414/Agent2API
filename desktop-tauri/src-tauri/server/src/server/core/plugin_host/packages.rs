//! 包在停止旧宿主之前完成解包与验签；登记失败时恢复同版本重装的原目录。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use agent2api_provider_protocol as proto;

use super::install::{self, InstalledPack, PackRegistry, PackSummary, SignatureState};

// 安装与下载共用有限的 ZIP 硬上限；解压总量与文件数仍由 install 独立限制。
pub(crate) const MAX_PACK_BYTES: usize = 24 * 1024 * 1024;

static SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub struct PreparedPack {
    staging: PathBuf,
    manifest: proto::PackManifest,
    entry: String,
    sha256: String,
    signed: bool,
}

impl Drop for PreparedPack {
    fn drop(&mut self) {
        if self.staging.exists() {
            let _ = std::fs::remove_dir_all(&self.staging);
        }
    }
}

pub(super) fn ensure_entry_executable(path: &Path) -> Result<(), String> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("读取插件入口失败（{}）：{error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("插件入口不是普通文件：{}", path.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = metadata.permissions().mode();
        if mode & 0o100 == 0 {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode | 0o100)).map_err(
                |error| format!("设置插件入口执行权限失败（{}）：{error}", path.display()),
            )?;
            let updated = std::fs::metadata(path)
                .map_err(|error| format!("读取插件入口权限失败（{}）：{error}", path.display()))?;
            if updated.permissions().mode() & 0o100 == 0 {
                return Err(format!(
                    "插件入口仍不可执行，请检查数据目录是否支持 Unix 执行权限：{}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

pub fn prepare(
    bytes: &[u8],
    expected_id: Option<&str>,
    expected_version: Option<&str>,
    require_signed: bool,
) -> Result<PreparedPack, String> {
    if bytes.is_empty() || bytes.len() > MAX_PACK_BYTES {
        return Err(format!(
            "插件包必须为非空且不超过 {} MiB 的 zip",
            MAX_PACK_BYTES / (1024 * 1024)
        ));
    }
    let staging = install::plugins_dir().join(".staging").join(format!(
        "pack-{}-{}-{}",
        std::process::id(),
        crate::server::logging::now_ms(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed),
    ));
    std::fs::create_dir_all(&staging).map_err(|error| format!("创建临时目录失败：{error}"))?;
    let prepared = (|| {
        install::extract_zip(bytes, &staging)?;
        let manifest = install::read_manifest(&staging)?;
        install::validate_manifest(&manifest, proto::PROTOCOL_VERSION)?;
        if let Some(reason) = super::compatibility::version_error(&manifest.id, &manifest.version) {
            return Err(reason);
        }
        if manifest.id.is_empty()
            || !manifest
                .id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err("插件包 id 只能包含小写字母、数字和连字符".to_string());
        }
        if manifest.version.is_empty()
            || !manifest
                .version
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'+'))
            || crate::server::core::update::compare_versions(&manifest.version, &manifest.version)
                .is_none()
        {
            return Err("插件包版本号不是合法的语义化版本".to_string());
        }
        if expected_id.is_some_and(|id| id != manifest.id)
            || expected_version.is_some_and(|version| version != manifest.version)
        {
            return Err("下载包的 id 或版本与所选发行版不一致，已拒绝安装".to_string());
        }
        let target = super::host_target();
        let entry = manifest
            .entry
            .get(&target)
            .cloned()
            .ok_or_else(|| format!("插件包没有为当前平台（{target}）提供入口"))?;
        if !staging.join(&entry).is_file() {
            return Err(format!("包里的入口二进制不存在：{entry}"));
        }
        let (signature, problem) = install::verify_signature(&staging, &manifest, &target);
        if signature == SignatureState::Invalid {
            return Err(format!("签名校验失败：{}", problem.unwrap_or_default()));
        }
        if require_signed && signature != SignatureState::Verified {
            return Err("自动安装只接受通过官方签名校验的插件包，请手动安装未签名包".to_string());
        }
        ensure_entry_executable(&staging.join(&entry))?;
        Ok(PreparedPack {
            staging: staging.clone(),
            manifest,
            entry,
            sha256: install::sha256_hex(bytes),
            signed: signature.is_signed(),
        })
    })();
    if prepared.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    prepared
}

impl PreparedPack {
    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    pub(super) fn record(&self, registry: &PackRegistry) -> InstalledPack {
        let previous = registry
            .packs
            .iter()
            .find(|pack| pack.id == self.manifest.id);
        let providers = self
            .manifest
            .providers
            .iter()
            .map(|provider| {
                let id = provider.id.0.clone();
                let enabled = previous
                    .and_then(|pack| pack.providers.get(&id))
                    .copied()
                    .or_else(|| {
                        super::ownership::migrated_enabled(registry, &self.manifest.id, &id)
                    })
                    .unwrap_or(provider.default_enabled);
                (id, enabled)
            })
            .collect();
        InstalledPack {
            id: self.manifest.id.clone(),
            version: self.manifest.version.clone(),
            enabled: previous.map(|pack| pack.enabled).unwrap_or(true),
            providers,
            entry: self.entry.clone(),
            sha256: self.sha256.clone(),
            signed: self.signed,
            installed_at: crate::server::logging::now_ms(),
            manifest_providers: self.manifest.providers.clone(),
        }
    }

    pub fn install(self) -> Result<PackSummary, String> {
        let transaction = self.install_pending()?;
        let id = transaction.id.clone();
        transaction.commit();
        install::list_packs()
            .into_iter()
            .find(|pack| pack.id == id)
            .ok_or_else(|| "安装后读不到注册表条目".to_string())
    }

    pub(super) fn install_pending(self) -> Result<InstalledTransaction, String> {
        let mut registry = PackRegistry::load();
        let previous_registry = registry.clone();
        let record = self.record(&registry);
        let version_dir = install::plugins_dir()
            .join(&self.manifest.id)
            .join(&self.manifest.version);
        let parent = version_dir
            .parent()
            .ok_or_else(|| "插件目录无效".to_string())?;
        std::fs::create_dir_all(parent).map_err(|error| format!("创建包目录失败：{error}"))?;
        let backup = parent.join(format!(
            ".previous-{}-{}",
            crate::server::logging::now_ms(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let replaced = version_dir.exists();
        if replaced {
            install::move_dir_retrying(&version_dir, &backup)
                .map_err(|error| format!("备份旧版本失败：{error}"))?;
        }
        if let Err(error) = install::move_dir_retrying(&self.staging, &version_dir) {
            if replaced {
                install::move_dir_retrying(&backup, &version_dir).map_err(|restore| {
                    format!(
                        "落盘失败：{error}；恢复旧文件失败：{restore}；备份保留在 {}",
                        backup.display()
                    )
                })?;
            }
            return Err(format!("落盘失败：{error}"));
        }
        match registry.packs.iter_mut().find(|pack| pack.id == record.id) {
            Some(slot) => *slot = record,
            None => registry.packs.push(record),
        }
        if let Err(error) = registry.save() {
            install::remove_dir_retrying(&version_dir).map_err(|restore| {
                format!("保存插件登记失败：{error}；移除新文件失败：{restore}；旧备份已保留")
            })?;
            if replaced {
                install::move_dir_retrying(&backup, &version_dir).map_err(|restore| {
                    format!(
                        "保存插件登记失败：{error}；恢复旧文件失败：{restore}；备份保留在 {}",
                        backup.display()
                    )
                })?;
            }
            return Err(error);
        }
        Ok(InstalledTransaction {
            id: self.manifest.id.clone(),
            previous_registry,
            version_dir,
            backup: replaced.then_some(backup),
        })
    }
}

pub(super) struct InstalledTransaction {
    id: String,
    previous_registry: PackRegistry,
    version_dir: PathBuf,
    backup: Option<PathBuf>,
}

impl InstalledTransaction {
    pub fn commit(self) {
        if let Some(backup) = self.backup {
            if let Err(error) = install::remove_dir_retrying(&backup) {
                crate::server::logging::log(
                    "[Plugin]",
                    &format!("插件更新成功，但临时备份清理失败：{error}"),
                );
            }
        }
    }

    pub fn rollback(self, keep_new_files: bool) -> Result<(), String> {
        // 入口完好却起不来的环境类失败（见 `SPAWN_ENV_FAILURE_TAG`）保留新版本
        // 目录现场：文件留给用户诊断（ldd / patchelf），登记照常回滚。只对
        // 全新安装生效 —— 更新失败时目录必须让位给备份恢复，现场由
        // last_error 与事件日志承载。
        let keep_new_files = keep_new_files && self.backup.is_none();
        if !keep_new_files {
            install::remove_dir_retrying(&self.version_dir).map_err(|error| {
                format!("移除未能启动的插件文件失败，旧版本备份已保留：{error}")
            })?;
        } else {
            crate::server::logging::log(
                "[Plugin]",
                &format!(
                    "插件启动失败源于运行环境，版本目录现场保留在 {}（登记已回滚，可诊断后重装）",
                    self.version_dir.display(),
                ),
            );
        };
        if let Some(backup) = self.backup {
            install::move_dir_retrying(&backup, &self.version_dir)
                .map_err(|error| format!("恢复插件文件失败，旧版本备份已保留：{error}"))?;
        }
        self.previous_registry
            .save()
            .map_err(|error| format!("恢复插件登记失败：{error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── 测试用的最小 store 模式 zip ────────────────────────────
    //
    // `zip` crate 没开 write feature（仓库只读包，见 Cargo.toml 注释），
    // 而这里的用例只需要「合法的未签名包」。与 scripts/pack.mjs 的产物同构：
    // store（不压缩）+ UTF-8 名字标志。可执行位进 central directory 的
    // external attributes 高 16 位（解包侧 `unix_mode()` 读的就是它）。

    fn crc32(data: &[u8]) -> u32 {
        // 逐位实现，免建 256 项表：测试里就几个小条目，速度足够
        (0..data.len()).fold(0xFFFF_FFFF_u32, |acc, i| {
            let acc = acc ^ (data[i] as u32);
            (0..8).fold(acc, |acc, _| {
                if acc & 1 == 1 {
                    (acc >> 1) ^ 0xEDB8_8320
                } else {
                    acc >> 1
                }
            })
        }) ^ 0xFFFF_FFFF
    }

    /// entries: (名字, 内容, 是否可执行)
    fn store_zip(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut locals = Vec::new();
        let mut centrals = Vec::new();
        let mut offset = 0_u32;
        for (name, data, executable) in entries {
            let name = name.as_bytes();
            let crc = crc32(data);
            let mode: u32 = if *executable { 0o100_755 } else { 0o100_644 };
            let local_len_before = locals.len() as u32;
            locals.extend_from_slice(&0x0403_4b50_u32.to_le_bytes());
            locals.extend_from_slice(&20_u16.to_le_bytes()); // 版本
            locals.extend_from_slice(&0x0800_u16.to_le_bytes()); // UTF-8 名字
            locals.extend_from_slice(&0_u16.to_le_bytes()); // store
            locals.extend_from_slice(&[0, 0, 0, 0]); // 时间/日期（0 合法）
            locals.extend_from_slice(&crc.to_le_bytes());
            locals.extend_from_slice(&(data.len() as u32).to_le_bytes());
            locals.extend_from_slice(&(data.len() as u32).to_le_bytes());
            locals.extend_from_slice(&(name.len() as u16).to_le_bytes());
            locals.extend_from_slice(&0_u16.to_le_bytes()); // extra
            locals.extend_from_slice(name);
            locals.extend_from_slice(data);
            centrals.extend_from_slice(&0x0201_4b50_u32.to_le_bytes());
            centrals.extend_from_slice(&(3_u16 << 8 | 20).to_le_bytes()); // unix + 版本
            centrals.extend_from_slice(&20_u16.to_le_bytes());
            centrals.extend_from_slice(&0x0800_u16.to_le_bytes());
            centrals.extend_from_slice(&0_u16.to_le_bytes());
            centrals.extend_from_slice(&[0, 0, 0, 0]);
            centrals.extend_from_slice(&crc.to_le_bytes());
            centrals.extend_from_slice(&(data.len() as u32).to_le_bytes());
            centrals.extend_from_slice(&(data.len() as u32).to_le_bytes());
            centrals.extend_from_slice(&(name.len() as u16).to_le_bytes());
            centrals.extend_from_slice(&0_u16.to_le_bytes()); // extra
            centrals.extend_from_slice(&0_u16.to_le_bytes()); // comment
            centrals.extend_from_slice(&0_u16.to_le_bytes()); // 起始盘号
            centrals.extend_from_slice(&0_u16.to_le_bytes()); // 内部属性
            centrals.extend_from_slice(&((mode << 16) as u32).to_le_bytes()); // 外部属性
            centrals.extend_from_slice(&local_len_before.to_le_bytes());
            centrals.extend_from_slice(name);
            offset += (locals.len() as u32) - local_len_before;
        }
        let central_start = offset;
        let central_size = centrals.len() as u32;
        let mut out = locals;
        out.extend_from_slice(&centrals);
        out.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&central_size.to_le_bytes());
        out.extend_from_slice(&central_start.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out
    }

    fn pack_json(id: &str, version: &str) -> Vec<u8> {
        let target = super::super::host_target();
        let manifest = serde_json::json!({
            "id": id,
            "name": "测试包",
            "version": version,
            "protocol": agent2api_provider_protocol::PROTOCOL_VERSION,
            "providers": [{"id": "demo", "label": "Demo", "order": 0, "default_enabled": true}],
            "entry": { target: "bin/app" },
        });
        // pack.mjs 产出的 pack.json 是带缩进的；这里格式无所谓，验签读的是内容
        serde_json::to_vec_pretty(&manifest).expect("序列化 pack.json")
    }

    /// 一份合法的未签名测试包（入口二进制内容随意，合法性校验只看结构）。
    fn unsigned_test_pack(id: &str, version: &str) -> Vec<u8> {
        let pack = pack_json(id, version);
        store_zip(&[
            ("pack.json", &pack, false),
            ("bin/app", b"test-entry-binary".as_slice(), true),
        ])
    }

    // ── 强制签名路径（spec ②：未签名包在强制路径仍拒绝）─────────
    //
    // prepare 的 require_signed 就是策略落点：自动安装 / 自动补齐恒 true
    // （operations::start_cached / start_required），手动安装恒 false。

    #[test]
    fn unsigned_pack_rejected_on_required_path() {
        let bytes = unsigned_test_pack("test-unsigned", "1.0.0");
        let err = match prepare(&bytes, None, None, true) {
            Err(err) => err,
            Ok(_) => panic!("强制签名路径必须拒绝未签名包"),
        };
        assert!(
            err.contains("自动安装只接受通过官方签名校验的插件包"),
            "报错要说清楚为什么拒绝：{err}"
        );
    }

    #[test]
    fn unsigned_pack_allowed_on_manual_path() {
        // 同一份包，手动口径（require_signed=false）放行，登记为未签名
        let bytes = unsigned_test_pack("test-unsigned", "1.0.0");
        let pack = prepare(&bytes, None, None, false).expect("手动路径放行未签名包");
        assert!(!pack.signed, "放行的未签名包要如实标记 signed=false");
        // PreparedPack::drop 会清掉 staging，无需手工清理
    }

    // ── 端到端验收（本地手跑，CI 无样本不跑）──────────────────
    //
    // 验证「自动安装路径」（require_signed=true）对 CI 产出的签名包放行：
    // AGENT2API_E2E_SIGNED_ZIP 指向 fork CI 签名的 zip（版本需过网关版本门）。

    #[test]
    #[ignore = "需要本地样本：AGENT2API_E2E_SIGNED_ZIP 指向 fork 签名的插件包"]
    fn e2e_signed_pack_installs_on_automatic_path() {
        let Some(zip_path) = std::env::var_os("AGENT2API_E2E_SIGNED_ZIP") else {
            panic!("设 AGENT2API_E2E_SIGNED_ZIP 指向签名包 zip 后重跑");
        };
        let bytes = std::fs::read(&zip_path).expect("读签名包");
        let pack = prepare(&bytes, None, None, true).expect("自动安装口径必须放行 fork 签名包");
        assert!(pack.signed, "签名包必须标记 signed=true");
        assert_eq!(pack.manifest.id, "agent2api-providers");
    }

    /// 篡改回归：签名包里改一个字节，同一份签名验不过 → 自动路径拒绝。
    /// AGENT2API_E2E_TAMPERED_ZIP 指向本地改过入口二进制后重打的 store zip。
    #[test]
    #[ignore = "需要本地样本：AGENT2API_E2E_TAMPERED_ZIP 指向被篡改的签名包"]
    fn e2e_tampered_signed_pack_rejected_on_automatic_path() {
        let Some(zip_path) = std::env::var_os("AGENT2API_E2E_TAMPERED_ZIP") else {
            panic!("设 AGENT2API_E2E_TAMPERED_ZIP 指向篡改包后重跑");
        };
        let bytes = std::fs::read(&zip_path).expect("读篡改包");
        let err = match prepare(&bytes, None, None, true) {
            Err(err) => err,
            Ok(_) => panic!("篡改包必须被拒绝"),
        };
        assert!(
            err.contains("签名校验失败"),
            "篡改包必须以签名校验失败拒绝：{err}"
        );
    }
}
