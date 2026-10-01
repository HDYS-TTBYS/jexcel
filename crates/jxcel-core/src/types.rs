use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::model::Column;

/// DB 的なデータ型。`Object` でネスト、`Any` で型なし、`Custom` で拡張（レジストリ経由）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DataType {
    String,
    Int,
    Float,
    /// 精度を失わないため文字列表現（例: "12.50"）で保持する。
    Decimal,
    Bool,
    /// ISO 8601 の日付（YYYY-MM-DD）
    Date,
    /// RFC 3339 の日時
    DateTime,
    Enum {
        values: Vec<String>,
    },
    Object {
        fields: Vec<Column>,
    },
    Array {
        item: Box<DataType>,
    },
    Any,
    /// 将来の JS による型拡張。レジストリに検証関数が登録されていなければ不正とみなす。
    Custom {
        name: String,
    },
}

type Validator = fn(&Value) -> std::result::Result<(), String>;

/// カスタム型の検証関数を保持する。JS 拡張の差し込み口。
#[derive(Default, Clone)]
pub struct TypeRegistry {
    custom: BTreeMap<String, Validator>,
}

impl TypeRegistry {
    pub fn register(&mut self, name: impl Into<String>, validator: Validator) {
        self.custom.insert(name.into(), validator);
    }
}

impl DataType {
    /// 値が型に適合するか検証する。`Null` は常に許容する（必須制約は Column 側）。
    pub fn validate(&self, value: &Value, reg: &TypeRegistry) -> std::result::Result<(), String> {
        if value.is_null() {
            return Ok(());
        }
        match self {
            DataType::Any => Ok(()),
            DataType::String => value
                .is_string()
                .then_some(())
                .ok_or_else(|| "文字列ではありません".into()),
            DataType::Int => (value.is_i64() || value.is_u64())
                .then_some(())
                .ok_or_else(|| "整数ではありません".into()),
            DataType::Float => value
                .is_number()
                .then_some(())
                .ok_or_else(|| "数値ではありません".into()),
            DataType::Bool => value
                .is_boolean()
                .then_some(())
                .ok_or_else(|| "真偽値ではありません".into()),
            DataType::Decimal => match value.as_str() {
                Some(s) if is_decimal(s) => Ok(()),
                _ => Err("10進数の文字列ではありません".into()),
            },
            DataType::Date => match value.as_str() {
                Some(s) if is_date(s) => Ok(()),
                _ => Err("YYYY-MM-DD 形式の日付ではありません".into()),
            },
            DataType::DateTime => match value.as_str() {
                Some(s) if is_datetime(s) => Ok(()),
                _ => Err("RFC 3339 形式の日時ではありません".into()),
            },
            DataType::Enum { values } => match value.as_str() {
                Some(s) if values.iter().any(|v| v == s) => Ok(()),
                _ => Err(format!("{values:?} のいずれかではありません")),
            },
            DataType::Array { item } => {
                let arr = value.as_array().ok_or("配列ではありません")?;
                for (i, v) in arr.iter().enumerate() {
                    item.validate(v, reg).map_err(|e| format!("[{i}]: {e}"))?;
                }
                Ok(())
            }
            DataType::Object { fields } => {
                let obj = value.as_object().ok_or("オブジェクトではありません")?;
                for key in obj.keys() {
                    if !fields.iter().any(|f| &f.id == key) {
                        return Err(format!("未定義のフィールド: {key}"));
                    }
                }
                for f in fields {
                    match obj.get(&f.id) {
                        Some(v) => f.validate(v, reg).map_err(|e| format!("{}: {e}", f.name))?,
                        None if f.required => return Err(format!("{} は必須です", f.name)),
                        None => {}
                    }
                }
                Ok(())
            }
            DataType::Custom { name } => match reg.custom.get(name) {
                Some(v) => v(value),
                None => Err(format!("未登録のカスタム型: {name}")),
            },
        }
    }
}

fn is_decimal(s: &str) -> bool {
    let body = s.strip_prefix('-').unwrap_or(s);
    let mut parts = body.splitn(2, '.');
    let int = parts.next().unwrap_or("");
    let frac = parts.next();
    !int.is_empty()
        && int.bytes().all(|b| b.is_ascii_digit())
        && frac.is_none_or(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()))
}

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let (Ok(y), Ok(m), Ok(d)) = (
        s[0..4].parse::<u32>(),
        s[5..7].parse::<u32>(),
        s[8..10].parse::<u32>(),
    ) else {
        return false;
    };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let max = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=max).contains(&d)
}

fn is_datetime(s: &str) -> bool {
    // "YYYY-MM-DDTHH:MM:SS" + 任意の小数秒 + ("Z" | ±HH:MM)
    let Some((date, time)) = s.split_once(['T', 't']) else {
        return false;
    };
    if !is_date(date) || time.len() < 9 {
        return false;
    }
    let tb = time.as_bytes();
    let digits = |r: std::ops::Range<usize>| tb[r].iter().all(u8::is_ascii_digit);
    if !(digits(0..2) && tb[2] == b':' && digits(3..5) && tb[5] == b':' && digits(6..8)) {
        return false;
    }
    if time[0..2].parse::<u32>().unwrap() > 23
        || time[3..5].parse::<u32>().unwrap() > 59
        || time[6..8].parse::<u32>().unwrap() > 60
    {
        return false;
    }
    let mut rest = &time[8..];
    if let Some(r) = rest.strip_prefix('.') {
        let n = r.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 {
            return false;
        }
        rest = &r[n..];
    }
    match rest {
        "Z" | "z" => true,
        _ => {
            let b = rest.as_bytes();
            b.len() == 6
                && (b[0] == b'+' || b[0] == b'-')
                && b[1..3].iter().all(u8::is_ascii_digit)
                && b[3] == b':'
                && b[4..6].iter().all(u8::is_ascii_digit)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ok(t: &DataType, v: Value) -> bool {
        t.validate(&v, &TypeRegistry::default()).is_ok()
    }

    #[test]
    fn scalars() {
        assert!(ok(&DataType::Int, json!(3)));
        assert!(!ok(&DataType::Int, json!(3.5)));
        assert!(ok(&DataType::Float, json!(3.5)));
        assert!(ok(&DataType::Decimal, json!("-12.50")));
        assert!(!ok(&DataType::Decimal, json!("12.")));
        assert!(!ok(&DataType::Decimal, json!(12.5)));
        assert!(ok(&DataType::Date, json!("2024-02-29")));
        assert!(!ok(&DataType::Date, json!("2023-02-29")));
        assert!(ok(
            &DataType::DateTime,
            json!("2024-01-02T03:04:05.678+09:00")
        ));
        assert!(ok(&DataType::DateTime, json!("2024-01-02T03:04:05Z")));
        assert!(!ok(&DataType::DateTime, json!("2024-01-02 03:04:05")));
        assert!(ok(&DataType::Int, Value::Null));
        assert!(ok(&DataType::Any, json!({"a": [1, "x"]})));
    }

    #[test]
    fn enum_array_object_custom() {
        let e = DataType::Enum {
            values: vec!["a".into(), "b".into()],
        };
        assert!(ok(&e, json!("a")));
        assert!(!ok(&e, json!("c")));

        let arr = DataType::Array {
            item: Box::new(DataType::Int),
        };
        assert!(ok(&arr, json!([1, 2])));
        assert!(!ok(&arr, json!([1, "x"])));

        let obj = DataType::Object {
            fields: vec![Column::new("n", "名前", DataType::String).required()],
        };
        assert!(ok(&obj, json!({"n": "x"})));
        assert!(!ok(&obj, json!({})));
        assert!(!ok(&obj, json!({"n": "x", "z": 1})));

        let c = DataType::Custom {
            name: "even".into(),
        };
        assert!(!ok(&c, json!(2)));
        let mut reg = TypeRegistry::default();
        reg.register("even", |v| match v.as_i64() {
            Some(n) if n % 2 == 0 => Ok(()),
            _ => Err("偶数ではありません".into()),
        });
        assert!(c.validate(&json!(2), &reg).is_ok());
        assert!(c.validate(&json!(3), &reg).is_err());
    }
}
