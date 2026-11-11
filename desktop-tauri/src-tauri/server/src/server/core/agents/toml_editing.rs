use serde_json::Value;
use toml_edit::{DocumentMut, Item, Table, TableLike};

pub fn parse(text: &str) -> Result<Value, String> {
    let text = text.trim_start_matches('\u{feff}');
    if text.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    let table: toml::Table = toml::from_str(text).map_err(|e| format!("配置不是合法 TOML: {e}"))?;
    serde_json::to_value(table).map_err(|e| format!("TOML 无法读取: {e}"))
}

pub fn render(original: &str, desired: &Value) -> Result<String, String> {
    let bom = original.starts_with('\u{feff}');
    let text = original.trim_start_matches('\u{feff}');
    let before = parse(text)?;
    if before == *desired {
        return Ok(original.into());
    }
    let mut document = text
        .parse::<DocumentMut>()
        .map_err(|e| format!("TOML 编辑失败: {e}"))?;
    let before = before.as_object().ok_or("TOML 根必须是表")?;
    let after = desired.as_object().ok_or("TOML 根必须是表")?;
    patch(document.as_table_mut(), before, after)?;
    let mut rendered = document.to_string();
    if text.contains("\r\n") {
        rendered = rendered.replace("\r\n", "\n").replace('\n', "\r\n");
    }
    if bom {
        rendered.insert(0, '\u{feff}');
    }
    if parse(&rendered)? != *desired {
        return Err("TOML 编辑结果与预期不一致，已停止写入".into());
    }
    Ok(rendered)
}

fn patch(
    table: &mut dyn TableLike,
    before: &serde_json::Map<String, Value>,
    after: &serde_json::Map<String, Value>,
) -> Result<(), String> {
    for key in before.keys().filter(|key| !after.contains_key(*key)) {
        table.remove(key);
    }
    for (key, value) in after {
        if before.get(key) == Some(value) {
            continue;
        }
        if let (Some(old), Some(new)) = (
            before.get(key).and_then(Value::as_object),
            value.as_object(),
        ) {
            let child = table
                .get_mut(key)
                .and_then(Item::as_table_like_mut)
                .ok_or_else(|| format!("TOML {key} 不是表，拒绝覆盖"))?;
            patch(child, old, new)?;
        } else {
            let mut replacement = item(value)?;
            if let (Some(old), Some(new)) = (
                table.get(key).and_then(Item::as_value),
                replacement.as_value_mut(),
            ) {
                *new.decor_mut() = old.decor().clone();
            }
            if let Some(existing) = table.get_mut(key) {
                *existing = replacement;
            } else {
                table.insert(key, replacement);
            }
        }
    }
    Ok(())
}

fn item(value: &Value) -> Result<Item, String> {
    if let Some(object) = value.as_object() {
        let mut table = Table::new();
        table.set_implicit(true);
        for (key, value) in object {
            table.insert(key, item(value)?);
        }
        return Ok(Item::Table(table));
    }
    let text = toml::to_string(&serde_json::json!({ "value": value }))
        .map_err(|e| format!("TOML 值无法生成: {e}"))?;
    let mut document = text
        .parse::<DocumentMut>()
        .map_err(|e| format!("TOML 值无法解析: {e}"))?;
    document.remove("value").ok_or_else(|| "TOML 值缺失".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::core::agents::{files, ConfigFormat};
    use serde_json::json;
    use toml_edit::{Item as TomlItem, Value as TomlValue};

    // 上游回归（aba58858 / a61a61ab 的 apply_tests.rs）挂到本文件的测试模块：
    // fork 没有 apply_tests.rs，但 files::parse/render 与 ConfigFormat 的签名与上游一致，
    // 保留 Jsonc + Toml 双格式循环是为了让 TOML 侧与「本来就没坏」的 JSONC 侧同框对照。
    #[test]
    fn jsonc_and_toml_edits_preserve_unrelated_fields_and_comments() {
        for (format, original) in [
            (
                ConfigFormat::Jsonc,
                "{\n  // keep this comment\n  \"model\": \"old\",\n  \"future\": {\"enabled\": true}\n}\n",
            ),
            (
                ConfigFormat::Toml,
                "# keep this comment\nmodel = \"old\"\n[future]\nenabled = true\n",
            ),
        ] {
            let mut desired = files::parse(format, original).unwrap();
            desired["model"] = json!("new");
            let rendered = files::render(format, original, &desired).unwrap();
            let text = std::str::from_utf8(&rendered).unwrap();
            assert!(text.contains("keep this comment"), "注释丢失: {text}");
            assert_eq!(files::parse(format, text).unwrap(), desired);
            assert_eq!(desired["future"]["enabled"], true);
        }
    }

    // 根因回归：改已有 model 值时，键前缀注释与无关的 [future] 表都要原样留下。
    // insert 覆盖已有键会 fmt() 清掉 key 的 leaf_decor，注释正是挂在那里。
    #[test]
    fn changing_an_existing_key_keeps_its_prefix_comment_and_unrelated_table() {
        let original = "# keep this comment\nmodel = \"old\"\n[future]\nenabled = true\n";
        let mut desired = parse(original).unwrap();
        desired["model"] = json!("new");
        let rendered = render(original, &desired).unwrap();
        assert!(
            rendered.contains("# keep this comment\nmodel = \"new\""),
            "注释没留在键前缀上: {rendered}"
        );
        assert!(rendered.contains("[future]"), "无关表头丢失: {rendered}");
        assert!(
            rendered.contains("enabled = true"),
            "无关表内容丢失: {rendered}"
        );
        assert_eq!(parse(&rendered).unwrap(), desired);
    }

    // 边界：嵌套表内改值同样走 patch 递归，子表头、子表内注释与兄弟键不能被动到。
    #[test]
    fn editing_a_nested_table_value_keeps_header_comments_and_siblings() {
        let original = concat!(
            "model = \"old\"\n",
            "\n[future]\n",
            "# nested note\n",
            "enabled = true\n",
            "retries = 3\n",
        );
        let mut desired = parse(original).unwrap();
        desired["future"]["enabled"] = json!(false);
        let rendered = render(original, &desired).unwrap();
        assert!(rendered.contains("[future]"), "子表头丢失: {rendered}");
        assert!(
            rendered.contains("# nested note"),
            "子表内注释丢失: {rendered}"
        );
        assert!(rendered.contains("retries = 3"), "兄弟键丢失: {rendered}");
        assert!(
            rendered.contains("enabled = false"),
            "嵌套值没改: {rendered}"
        );
        assert_eq!(parse(&rendered).unwrap(), desired);
    }

    // 边界：新增键走 insert 分支，原有注释与键排版不能被重排。
    #[test]
    fn adding_a_new_key_preserves_comments_and_existing_content() {
        let original = "# keep this comment\nmodel = \"old\"\n";
        let mut desired = parse(original).unwrap();
        desired["extra"] = json!(42);
        let rendered = render(original, &desired).unwrap();
        assert!(
            rendered.contains("# keep this comment\nmodel = \"old\""),
            "原有注释或键排版被改写: {rendered}"
        );
        assert!(rendered.contains("extra = 42"), "新键没写入: {rendered}");
        assert_eq!(parse(&rendered).unwrap(), desired);
    }

    // 边界：键前缀注释（key 的 leaf_decor）与行内注释（value 的 decor）改值时都留在原处。
    #[test]
    fn changing_a_value_keeps_key_prefix_and_inline_comments() {
        let original = "# keep this comment\nmodel = \"old\" # inline note\n";
        let mut desired = parse(original).unwrap();
        desired["model"] = json!("new");
        let rendered = render(original, &desired).unwrap();
        assert!(
            rendered.contains("# keep this comment\nmodel = \"new\""),
            "键前缀注释或排版被改写: {rendered}"
        );
        assert!(
            rendered.contains("# inline note"),
            "行内注释丢失: {rendered}"
        );
        assert_eq!(parse(&rendered).unwrap(), desired);
    }

    // 错误路径：desired 根不是表时必须明确报错，而不是静默写入或 panic。
    #[test]
    fn render_rejects_a_non_table_root() {
        let err = render("model = \"old\"\n", &json!(["not", "a", "table"])).unwrap_err();
        assert!(err.contains("TOML 根必须是表"), "报错不明确: {err}");
    }

    // 错误路径：把标量节点当表递归修改时，patch 必须明确拒绝，不能静默覆盖。
    #[test]
    fn patch_rejects_scalar_as_table() {
        let mut table = Table::new();
        table.insert("model", TomlItem::Value(TomlValue::from("old")));
        let before = json!({"model": {"x": 1}}).as_object().unwrap().clone();
        let after = json!({"model": {"x": 2}}).as_object().unwrap().clone();
        let err = patch(&mut table, &before, &after).unwrap_err();
        assert!(err.contains("不是表"), "报错不明确: {err}");
    }
}
