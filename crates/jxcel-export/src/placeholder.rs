//! 差し込み欄 `{{ 式 }}` の検出と、値の文字列化。

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    /// 元の文字列の中でのバイト位置（`{{` から `}}` の後ろまで）
    pub start: usize,
    pub end: usize,
    /// 前後の空白を除いた式
    pub expr: String,
}

/// `{{ 式 }}` を前から順に見つける。式が空のもの、`}}` で閉じていないものは対象外。
pub fn find(text: &str) -> Vec<Match> {
    let mut out = vec![];
    let mut from = 0;
    while let Some(open) = text[from..].find("{{") {
        let start = from + open;
        let Some(close) = text[start + 2..].find("}}") else {
            break;
        };
        let end = start + 2 + close + 2;
        let expr = text[start + 2..end - 2].trim();
        if expr.is_empty() {
            from = start + 2;
        } else {
            out.push(Match {
                start,
                end,
                expr: expr.to_string(),
            });
            from = end;
        }
    }
    out
}

/// 差し込む文字列。null は空、文字列はそのまま、数値・真偽値は JS と同じ表記、配列やオブジェクトは JSON。
pub fn display(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// 文字列中の差し込み欄をすべて置換する（ファイル名など、プレーンな文字列用）。
pub fn replace_all(
    text: &str,
    resolve: &mut dyn FnMut(&str) -> Result<Value, String>,
) -> Result<String, (String, String)> {
    let mut out = String::new();
    let mut last = 0;
    for m in find(text) {
        out.push_str(&text[last..m.start]);
        let v = resolve(&m.expr).map_err(|e| (m.expr.clone(), e))?;
        out.push_str(&display(&v));
        last = m.end;
    }
    out.push_str(&text[last..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finds_placeholders_in_order_and_trims() {
        let m = find("宛先: {{ 取引先 }} 様 / 合計 {{数量 * 単価}} 円 / {{}} / {{ }} / {{未閉じ");
        assert_eq!(
            m.iter().map(|m| m.expr.as_str()).collect::<Vec<_>>(),
            ["取引先", "数量 * 単価"]
        );
        assert_eq!(
            &"宛先: {{ 取引先 }} 様"[m[0].start..m[0].end],
            "{{ 取引先 }}"
        );
        assert!(find("波括弧なし").is_empty());
        assert_eq!(find("{{a}}{{b}}").len(), 2);
    }

    #[test]
    fn display_follows_js_notation() {
        assert_eq!(display(&json!(null)), "");
        assert_eq!(display(&json!("あ")), "あ");
        assert_eq!(display(&json!(3)), "3");
        assert_eq!(display(&json!(1.5)), "1.5");
        assert_eq!(display(&json!(true)), "true");
        assert_eq!(display(&json!([1, "a"])), r#"[1,"a"]"#);
    }

    #[test]
    fn replace_all_substitutes_and_reports_failing_expr() {
        let ok = replace_all("{{a}}-{{b}}.docx", &mut |e| Ok(json!(e.to_uppercase()))).unwrap();
        assert_eq!(ok, "A-B.docx");
        let err = replace_all("x{{bad}}", &mut |e| Err(format!("{e} は未定義"))).unwrap_err();
        assert_eq!(err, ("bad".to_string(), "bad は未定義".to_string()));
    }
}
