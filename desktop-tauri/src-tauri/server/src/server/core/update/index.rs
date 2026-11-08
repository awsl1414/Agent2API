//! 应用更新与强制更新共用插件索引仓库的 app.json；只信任官方确认过的策略。
//!
//! ── 「官方」在 fork 里指哪份索引 ───────────────────────
//! 这里的来源字符串（见 [`AppIndex::required`] 拼的 `source` 字段）与
//! `resolve()` 读的 URL 都跟着 [`plugin_host::update::INDEX_REPO`] 走，
//! fork 已把它指向自有索引 `awsl1414/Agent2plugin`：app.repo 指回
//! awsl1414/Agent2API，强制更新策略也由 fork 自己发布。所以「官方确认」
//! 的实际含义是 fork 维护者在自有索引里发布的内容，不再经过上游。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    client,
    version::{compare_versions, DEFAULT_REPO},
};
use crate::server::core::{plugin_host, task_state};

const TRUSTED_KEY: &str = "updateIndexTrustedApp";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ForceRule {
    pub minimum_version: Option<String>,
    pub target_version: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ForceUpdate {
    pub stable: ForceRule,
    pub beta: ForceRule,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppIndex {
    pub repo: String,
    pub force_update: ForceUpdate,
}

impl Default for AppIndex {
    fn default() -> Self {
        Self {
            repo: DEFAULT_REPO.to_string(),
            force_update: ForceUpdate::default(),
        }
    }
}

impl AppIndex {
    pub fn required(&self, current: &str) -> Option<Value> {
        let rule = if super::is_prerelease_version(current) {
            &self.force_update.beta
        } else {
            &self.force_update.stable
        };
        let minimum = rule.minimum_version.as_deref()?.trim();
        if compare_versions(current, minimum) != Some(-1) {
            return None;
        }
        let target = rule
            .target_version
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(minimum)
            .trim();
        if compare_versions(target, minimum).is_none_or(|order| order < 0) {
            return None;
        }
        Some(
            json!({ "required": true, "minimumVersion": minimum, "targetVersion": target,
                "message": if rule.message.trim().is_empty() { "当前版本已停止提供模型服务，请先完成更新。" } else { rule.message.trim() },
                "source": format!("https://github.com/{}/blob/main/app.json", plugin_host::update::INDEX_REPO),
            }),
        )
    }
}

pub fn cached() -> AppIndex {
    task_state::read(TRUSTED_KEY)
        .ok()
        .and_then(|state| state.value)
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

pub async fn resolve() -> AppIndex {
    let previous = cached();
    let url = format!(
        "https://raw.githubusercontent.com/{}/main/app.json",
        plugin_host::update::INDEX_REPO
    );
    let response = match client::fetch_official(&url, &[], Some(20_000)).await {
        Ok(response) if response.status().is_success() => response,
        _ => return previous,
    };
    let Ok(value) = response.json::<Value>().await else {
        return previous;
    };
    let Some(index) = parse(&value) else {
        return previous;
    };
    let _ = task_state::store_value(TRUSTED_KEY, json!(index));
    index
}

fn parse(value: &Value) -> Option<AppIndex> {
    if value.get("schema").and_then(Value::as_u64) != Some(1) {
        return None;
    }
    let index: AppIndex = serde_json::from_value(value.get("app")?.clone()).ok()?;
    if !is_repo_slug(&index.repo) {
        return None;
    }
    for rule in [&index.force_update.stable, &index.force_update.beta] {
        if rule.message.chars().count() > 2000 {
            return None;
        }
        if let Some(minimum) = rule
            .minimum_version
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            compare_versions(minimum, minimum)?;
            if let Some(target) = rule
                .target_version
                .as_deref()
                .filter(|value| !value.is_empty())
            {
                if compare_versions(target, minimum).is_none_or(|order| order < 0) {
                    return None;
                }
            }
        }
    }
    Some(index)
}

fn is_repo_slug(value: &str) -> bool {
    let mut parts = value.split('/');
    let (Some(owner), Some(repo), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    [owner, repo].iter().all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// fork 自有索引的 app.json 必须能被 parse 接受 —— 这是「更新检查完全
    /// 走自有链路」的最后一环：索引格式或仓库 slug 的校验收紧时，如果自
    /// 家的 app.json 过不了，用户侧表现是静默回退到兜底仓库。内容与
    /// awsl1414/Agent2plugin 的 app.json 保持同形，改索引格式时同步这里。
    #[test]
    fn parse_accepts_own_fork_index() {
        let document = json!({
            "schema": 1,
            "updated": "2026-11-08",
            "app": {
                "repo": "awsl1414/Agent2API",
                "forceUpdate": {
                    "stable": { "minimumVersion": null, "targetVersion": null, "message": "" },
                    "beta": { "minimumVersion": null, "targetVersion": null, "message": "" }
                }
            }
        });
        let index = parse(&document).expect("自有索引的 app.json 应当通过校验");
        assert_eq!(index.repo, "awsl1414/Agent2API");
        assert!(index.force_update.stable.minimum_version.is_none());
        assert!(index.required("3.4.3-beta.1").is_none());
    }

    /// 强制更新的 source 字段跟着 INDEX_REPO 走：用户看到的「策略来源」
    /// 应当指向 fork 自有索引，而不是上游。
    #[test]
    fn required_source_points_at_own_index() {
        let index = AppIndex {
            repo: "awsl1414/Agent2API".to_string(),
            force_update: ForceUpdate {
                stable: ForceRule {
                    minimum_version: Some("3.0.0".to_string()),
                    target_version: None,
                    message: String::new(),
                },
                beta: ForceRule::default(),
            },
        };
        let required = index.required("2.9.0").expect("低于最低版本应触发强制更新");
        assert_eq!(
            required["source"].as_str().unwrap(),
            "https://github.com/awsl1414/Agent2plugin/blob/main/app.json"
        );
    }
}
