//! 只用于本机去重的业务指纹；上传快照仍保留原始密文与观测时间。

use hmac::{Hmac, Mac};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::server::core::crypto;

use super::snapshot::Snapshot;

const HASH_VERSION: &str = "business-v1";

pub(crate) fn content_hash(snapshot: &Snapshot, key: Option<&[u8; 32]>) -> Result<String, String> {
    let mut digest = Sha256::new();
    text(&mut digest, HASH_VERSION);
    text(&mut digest, &snapshot.format);
    digest.update(snapshot.version.to_be_bytes());
    digest.update(snapshot.schema_version.to_be_bytes());
    digest.update((snapshot.tables.len() as u64).to_be_bytes());

    for (table, rows) in &snapshot.tables {
        text(&mut digest, table);
        let mut row_hashes = Vec::with_capacity(rows.len());
        for row in rows {
            let mut row_digest = Sha256::new();
            hash_row(&mut row_digest, table, row, key)?;
            row_hashes.push(<[u8; 32]>::from(row_digest.finalize()));
        }
        // 表的物理行序不是业务顺序；账号顺序由 priority 等实际字段表达。
        row_hashes.sort_unstable();
        digest.update((row_hashes.len() as u64).to_be_bytes());
        for hash in row_hashes {
            digest.update(hash);
        }
    }
    digest.update((snapshot.kv.len() as u64).to_be_bytes());
    for (name, raw) in &snapshot.kv {
        text(&mut digest, name);
        match serde_json::from_str::<Value>(raw) {
            Ok(value) => hash_value(&mut digest, &value, key, 0)?,
            Err(_) => text(&mut digest, raw),
        }
    }
    let bytes = digest.finalize();
    let hash = if let Some(key) = key {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key)
            .map_err(|_| "无法初始化同步内容指纹".to_string())?;
        mac.update(&bytes);
        format!("{:x}", mac.finalize().into_bytes())
    } else {
        format!("{bytes:x}")
    };
    Ok(format!("{HASH_VERSION}:{hash}"))
}

fn hash_row(
    digest: &mut Sha256,
    table: &str,
    row: &Map<String, Value>,
    key: Option<&[u8; 32]>,
) -> Result<(), String> {
    let mut fields: Vec<_> = row.iter().filter(|(name, _)| {
        table != "account_usage_records"
            || !matches!(name.as_str(), "last_attempt_at" | "last_success_at" | "updated_at")
    }).collect();
    fields.sort_unstable_by(|a, b| a.0.cmp(b.0));
    digest.update((fields.len() as u64).to_be_bytes());
    for (name, value) in fields {
        text(digest, name);
        if table == "accounts" && name == "data" {
            let raw = value.as_str().ok_or_else(|| "账号数据不是有效文本，无法比较同步内容".to_string())?;
            let decoded = if crypto::is_envelope(raw) {
                let key = key.ok_or_else(|| "缺少账号密钥，无法比较同步内容；请恢复 account.key".to_string())?;
                Zeroizing::new(crypto::open_with_key(raw, key)
                    .map_err(|_| "账号数据无法解密，已停止同步；请核对 account.key".to_string())?)
            } else {
                Zeroizing::new(raw.as_bytes().to_vec())
            };
            let mut account: Value = serde_json::from_slice(&decoded)
                .map_err(|_| "账号数据不是有效 JSON，无法比较同步内容".to_string())?;
            if let Some(object) = account.as_object_mut() {
                // 只排除账号写回时刻；凭证到期、签到和插件台账里的时间仍参与比较。
                object.remove("updatedAt");
            }
            hash_value(digest, &account, key, 0)?;
        } else if table == "account_usage_records" && name == "usage" {
            if let Some(usage) = value.as_str().and_then(|raw| serde_json::from_str::<Value>(raw).ok()) {
                hash_value(digest, &usage, key, 0)?;
            } else {
                hash_value(digest, value, key, 0)?;
            }
        } else {
            hash_value(digest, value, key, 0)?;
        }
    }
    Ok(())
}

fn hash_value(
    digest: &mut Sha256,
    value: &Value,
    key: Option<&[u8; 32]>,
    depth: usize,
) -> Result<(), String> {
    if depth > 128 {
        return Err("同步内容嵌套过深，无法生成指纹".to_string());
    }
    match value {
        Value::Null => digest.update(b"n"),
        Value::Bool(value) => digest.update(if *value { b"t" } else { b"f" }),
        Value::Number(value) => {
            digest.update(b"d");
            text(digest, &value.to_string());
        }
        Value::String(value) => {
            digest.update(b"s");
            // 相同凭据重新加密会更换随机 nonce；只在内存里比较解密后的字节。
            // 不属于账号密钥域的信封保留原文，不能丢弃未知配置。
            let decoded = key.filter(|_| crypto::is_envelope(value))
                .and_then(|key| crypto::open_with_key(value, key).ok())
                .map(Zeroizing::new);
            let bytes = decoded.as_deref().map(Vec::as_slice).unwrap_or_else(|| value.as_bytes());
            digest.update((bytes.len() as u64).to_be_bytes());
            digest.update(bytes);
        }
        Value::Array(values) => {
            digest.update(b"a");
            digest.update((values.len() as u64).to_be_bytes());
            for value in values {
                hash_value(digest, value, key, depth + 1)?;
            }
        }
        Value::Object(object) => {
            digest.update(b"o");
            digest.update((object.len() as u64).to_be_bytes());
            let mut fields: Vec<_> = object.iter().collect();
            fields.sort_unstable_by(|a, b| a.0.cmp(b.0));
            for (name, value) in fields {
                text(digest, name);
                hash_value(digest, value, key, depth + 1)?;
            }
        }
    }
    Ok(())
}

fn text(digest: &mut Sha256, value: &str) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::core::cloud_sync::snapshot::{SNAPSHOT_FORMAT, SNAPSHOT_VERSION};
    use std::collections::BTreeMap;

    /// 造一个最小快照：一张表一行（或两行）accounts，其余空。
    fn snapshot_with_rows(rows: Vec<Map<String, Value>>, exported_at: i64) -> Snapshot {
        Snapshot {
            format: SNAPSHOT_FORMAT.to_string(),
            version: SNAPSHOT_VERSION,
            schema_version: crate::server::db::schema::SCHEMA_VERSION,
            exported_at,
            tables: BTreeMap::from([("accounts".to_string(), rows)]),
            kv: BTreeMap::new(),
        }
    }

    fn account_row(name: &str, priority: i64, updated_at: i64) -> Map<String, Value> {
        Map::from_iter([
            ("id".to_string(), Value::String(name.to_string())),
            ("name".to_string(), Value::String(name.to_string())),
            ("priority".to_string(), Value::from(priority)),
            ("updatedAt".to_string(), Value::from(updated_at)),
        ])
    }

    #[test]
    fn row_order_and_exported_at_do_not_change_the_hash() {
        let key = [7u8; 32];
        // 同一内容、不同行序与不同导出时刻：业务上没变，指纹必须相等 ——
        // 旧的字节哈希在这里每次都不同，自动同步因此白跑一趟（#502）。
        let a = snapshot_with_rows(
            vec![account_row("a", 1, 100), account_row("b", 2, 200)],
            1_000,
        );
        let b = snapshot_with_rows(
            vec![account_row("b", 2, 200), account_row("a", 1, 100)],
            9_999_999,
        );
        let hash_a = content_hash(&a, Some(&key)).expect("指纹可算");
        let hash_b = content_hash(&b, Some(&key)).expect("指纹可算");
        assert_eq!(hash_a, hash_b);
        assert!(hash_a.starts_with("business-v1:"));
    }

    #[test]
    fn account_write_back_time_is_excluded_but_real_fields_are_not() {
        let key = [7u8; 32];
        // 写回时刻 updatedAt 在 accounts.data 的 JSON 里（hash_row 解开 data
        // 后剔除它）；priority 是业务字段，参与比较。
        let data = |updated_at: i64| {
            format!(r#"{{"name":"account-one","priority":1,"updatedAt":{updated_at}}}"#)
        };
        let a = snapshot_with_rows(
            vec![{
                let mut row = account_row("a", 1, 0);
                row.insert("data".to_string(), Value::String(data(100)));
                row
            }],
            0,
        );
        let b = snapshot_with_rows(
            vec![{
                let mut row = account_row("a", 1, 0);
                row.insert("data".to_string(), Value::String(data(999_999)));
                row
            }],
            0,
        );
        assert_eq!(
            content_hash(&a, Some(&key)).expect("指纹可算"),
            content_hash(&b, Some(&key)).expect("指纹可算")
        );
        let changed = snapshot_with_rows(
            vec![{
                let mut row = account_row("a", 2, 0);
                row.insert("data".to_string(), Value::String(data(100)));
                row
            }],
            0,
        );
        assert_ne!(
            content_hash(&a, Some(&key)).expect("指纹可算"),
            content_hash(&changed, Some(&key)).expect("指纹可算")
        );
    }

    #[test]
    fn envelope_accounts_compare_by_decrypted_content() {
        // 同一份账号数据，两次加密的信封字节不同（随机 nonce）；
        // 指纹解开后比较，不该把加密差异当成业务差异。
        let key = [7u8; 32];
        let plaintext = String::from_utf8_lossy(b"{\"name\":\"account-one\",\"priority\":1}").into_owned();
        let envelope_a = crate::server::core::crypto::seal_with_key(plaintext.as_bytes(), &key)
            .expect("加密成功");
        let envelope_b = crate::server::core::crypto::seal_with_key(plaintext.as_bytes(), &key)
            .expect("加密成功");
        assert_ne!(envelope_a, envelope_b, "两次加密的信封字节应不同");

        let mut row_a = account_row("a", 1, 0);
        row_a.insert("data".to_string(), Value::String(envelope_a));
        let mut row_b = account_row("a", 1, 0);
        row_b.insert("data".to_string(), Value::String(envelope_b));
        let a = snapshot_with_rows(vec![row_a], 0);
        let b = snapshot_with_rows(vec![row_b], 0);
        assert_eq!(content_hash(&a, Some(&key)).expect("指纹可算"), content_hash(&b, Some(&key)).expect("指纹可算"));
    }
}
