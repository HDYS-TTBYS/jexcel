//! 同梱のサンプルマクロ。`samples/*.ts` を埋め込み、先頭のコメントから名前と説明を取り出す。
//!
//! 追加するときは `samples/` に `.ts` を置き、下の `FILES` に足す
//! （足し忘れはテストが検出する。全サンプルは実データで実行するテストもある）。

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Sample {
    /// ファイル名（拡張子なし）
    pub id: String,
    pub name: String,
    pub description: String,
    /// マクロのソース。`@name` / `@desc` の行は取り除き、説明は普通のコメントとして先頭に付く。
    pub source: String,
}

const FILES: &[(&str, &str)] = &[
    (
        "normalize-text",
        include_str!("../samples/normalize-text.ts"),
    ),
    (
        "remove-duplicates",
        include_str!("../samples/remove-duplicates.ts"),
    ),
    (
        "remove-empty-rows",
        include_str!("../samples/remove-empty-rows.ts"),
    ),
    ("fill-down", include_str!("../samples/fill-down.ts")),
    ("summarize", include_str!("../samples/summarize.ts")),
];

pub fn samples() -> Vec<Sample> {
    FILES.iter().map(|(id, src)| parse(id, src)).collect()
}

fn parse(id: &str, src: &str) -> Sample {
    let (mut name, mut description) = (id.to_string(), String::new());
    let mut body = vec![];
    for line in src.lines() {
        if let Some(n) = line.strip_prefix("// @name ") {
            name = n.trim().to_string();
        } else if let Some(d) = line.strip_prefix("// @desc ") {
            description = d.trim().to_string();
        } else {
            body.push(line);
        }
    }
    let source = format!(
        "// {description}\n{}\n",
        body.join("\n").trim_start_matches('\n')
    );
    Sample {
        id: id.to_string(),
        name,
        description,
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{compute, run, transpile, Options};
    use jxcel_core::{Column, DataSchema, DataType, JxcelFile, Row, Sheet};
    use serde_json::{json, Value};
    use std::collections::BTreeMap;

    fn sample(id: &str) -> Sample {
        samples()
            .into_iter()
            .find(|s| s.id == id)
            .unwrap_or_else(|| panic!("サンプル {id} がありません"))
    }

    /// サンプルの既定の名前（シート1 / データ / 列1 / 列2 / 集計）に合わせたファイル。
    fn fixture(rows: &[(Option<&str>, Option<i64>)]) -> JxcelFile {
        let mut data = DataSchema::new(
            "データ",
            vec![
                Column::new("c1", "列1", DataType::String),
                Column::new("c2", "列2", DataType::Int),
            ],
        );
        data.id = "data".into();
        for (i, (a, b)) in rows.iter().enumerate() {
            let mut cells: BTreeMap<String, Value> = BTreeMap::new();
            if let Some(a) = a {
                cells.insert("c1".into(), json!(a));
            }
            if let Some(b) = b {
                cells.insert("c2".into(), json!(b));
            }
            data.rows.push(Row {
                id: format!("r{i}"),
                cells,
            });
        }
        let mut summary = DataSchema::new(
            "集計",
            vec![
                Column::new("k", "区分", DataType::String),
                Column::new("s", "合計", DataType::Int),
                Column::new("n", "件数", DataType::Int),
            ],
        );
        summary.id = "sum".into();
        // 前回の集計が残っている状態
        summary.rows.push(Row {
            id: "old".into(),
            cells: [("k".to_string(), json!("古い"))].into(),
        });
        JxcelFile {
            name: "t".into(),
            sheets: vec![Sheet {
                id: "sh".into(),
                name: "シート1".into(),
                schemas: vec![data, summary],
            }],
            macros: vec![],
            exports: Default::default(),
            forms: Default::default(),
            templates: Default::default(),
        }
    }

    fn go(id: &str, f: &JxcelFile) -> crate::MacroRun {
        run(&sample(id).source, f, &Options::default()).unwrap_or_else(|e| panic!("{id}: {e}"))
    }

    fn col1(f: &JxcelFile) -> Vec<Option<String>> {
        f.sheets[0].schemas[0]
            .rows
            .iter()
            .map(|r| r.cells.get("c1").and_then(|v| v.as_str()).map(String::from))
            .collect()
    }

    #[test]
    fn every_sample_file_is_registered_and_well_formed() {
        // samples/ にあるのに FILES に無いファイル（足し忘れ）を検出する
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/samples");
        let mut on_disk: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| {
                e.unwrap()
                    .file_name()
                    .to_string_lossy()
                    .trim_end_matches(".ts")
                    .to_string()
            })
            .collect();
        on_disk.sort();
        let mut registered: Vec<String> = samples().into_iter().map(|s| s.id).collect();
        registered.sort();
        assert_eq!(on_disk, registered);

        for s in samples() {
            assert!(
                !s.name.is_empty() && s.name != s.id,
                "{}: @name がありません",
                s.id
            );
            assert!(!s.description.is_empty(), "{}: @desc がありません", s.id);
            assert!(!s.source.contains("@name") && !s.source.contains("@desc"));
            assert!(s.source.starts_with(&format!("// {}", s.description)));
            transpile(&s.source).unwrap_or_else(|e| panic!("{}: {e}", s.id));
        }
    }

    #[test]
    fn normalize_text() {
        let f = fixture(&[
            (Some("  Ａｂｃ　１２３ "), Some(1)),
            (Some("ﾊﾟｿｺﾝ"), None),
            (Some("ok"), Some(2)),
            (None, None),
        ]);
        let r = go("normalize-text", &f);
        assert_eq!(
            col1(&r.file),
            [
                Some("Abc 123".into()),
                Some("パソコン".into()),
                Some("ok".into()),
                None
            ]
        );
        assert_eq!(r.result, Some(json!(2)));
        assert_eq!(r.ops, 2); // 変わったセルだけ更新する
    }

    #[test]
    fn remove_duplicates() {
        let f = fixture(&[
            (Some("a"), Some(1)),
            (Some("b"), Some(2)),
            (Some("a"), Some(3)),
            (Some("c"), None),
            (Some("b"), Some(4)),
        ]);
        let r = go("remove-duplicates", &f);
        assert_eq!(
            col1(&r.file),
            [Some("a".into()), Some("b".into()), Some("c".into())]
        );
        // 残るのは最初の行
        assert_eq!(r.file.sheets[0].schemas[0].rows[0].cells["c2"], json!(1));
        assert_eq!(r.result, Some(json!(2)));
    }

    #[test]
    fn remove_empty_rows() {
        let f = fixture(&[
            (Some("x"), Some(1)),
            (None, None),
            (Some("  "), None),
            (None, Some(0)),
            (Some(""), None),
        ]);
        let r = go("remove-empty-rows", &f);
        // 「列2 が 0」の行は空ではない。空白だけ・空文字だけ・全部空の行が消える
        assert_eq!(r.file.sheets[0].schemas[0].rows.len(), 2);
        assert_eq!(r.result, Some(json!(3)));
    }

    #[test]
    fn fill_down() {
        let f = fixture(&[
            (None, Some(0)),
            (Some("A"), None),
            (None, None),
            (Some(" "), None),
            (Some("B"), None),
            (None, None),
        ]);
        let r = go("fill-down", &f);
        // 先頭の空欄は埋める元が無いのでそのまま
        assert_eq!(
            col1(&r.file),
            [
                None,
                Some("A".into()),
                Some("A".into()),
                Some("A".into()),
                Some("B".into()),
                Some("B".into())
            ]
        );
        assert_eq!(r.result, Some(json!(3)));
    }

    #[test]
    fn summarize_rebuilds_the_summary_table() {
        let f = fixture(&[
            (Some("y"), Some(2)),
            (Some("x"), Some(1)),
            (Some("x"), Some(3)),
            (Some("x"), None),
        ]);
        let r = go("summarize", &f);
        let rows = &r.file.sheets[0].schemas[1].rows;
        let got: Vec<(Value, Value, Value)> = rows
            .iter()
            .map(|r| {
                (
                    r.cells["k"].clone(),
                    r.cells["s"].clone(),
                    r.cells["n"].clone(),
                )
            })
            .collect();
        // 古い行は消え、区分の順（x → y）で、合計は空を無視し、件数は行数
        assert_eq!(
            got,
            [
                (json!("x"), json!(4), json!(3)),
                (json!("y"), json!(2), json!(1))
            ]
        );
        assert_eq!(r.result, Some(json!(2)));
        // もう一度実行しても同じ（作り直し）
        let again = go("summarize", &r.file);
        assert_eq!(again.file.sheets[0].schemas[1].rows.len(), 2);
    }

    #[test]
    fn samples_skip_computed_columns() {
        // 計算列があっても、サンプルは書き込もうとせず（書き込めば例外）、判定にも使わない
        let mut f = fixture(&[(Some(" Ａ "), Some(1)), (None, None)]);
        f.sheets[0].schemas[0]
            .columns
            .push(Column::new("cc", "計算", DataType::String).computed("export default () => 'x'"));
        let r = go("normalize-text", &f);
        assert_eq!(col1(&r.file)[0].as_deref(), Some("A"));
        let r = go("remove-empty-rows", &f);
        // 計算列の値 'x' は判定に使わないので、空の行は消える
        assert_eq!(r.file.sheets[0].schemas[0].rows.len(), 1);
        let _ = compute(&r.file, &Options::default());
    }
}
