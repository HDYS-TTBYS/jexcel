//! Excel・Word・LibreOffice などで開いて確かめるためのファイル一式を作る。
//!
//! ```text
//! cargo run -p jxcel-export --example make_verification_set -- 出力先ディレクトリ
//! ```
//!
//! テンプレート（`tests/fixtures/*.docx|xlsx` と `verification/templates/*.xlsx`）に、固定のサンプルデータ
//! （ねじ 3 個 + 付属 a・b、板 1 個 + 付属なし）を差し込んだ結果を書き出す。docx は 0 件の場合（表やブロックが
//! 消える）も作る。何を確かめるかは `verification/README.md`。

use jxcel_export::{render_with_notes, Kind, Source};
use serde_json::{json, Value};
use std::path::Path;

type Item = (&'static str, i64, Vec<&'static str>);

/// 外側のループが「明細」（品目・数）、入れ子のループが「付属」（名）のサンプル。
struct Sample {
    items: Vec<Item>,
}

impl Source for Sample {
    fn value(&mut self, expr: &str) -> Result<Value, String> {
        match expr {
            "請求番号" => Ok(json!("INV-7")),
            "合計" => Ok(json!(999)),
            e => Err(format!("未定義: {e}")),
        }
    }

    fn loop_len(
        &mut self,
        _index: usize,
        parent: Option<usize>,
        path: &[usize],
        source: &str,
    ) -> Result<usize, String> {
        match (parent, source) {
            (None, "明細") => Ok(self.items.len()),
            (Some(_), "付属") => Ok(self.items[path[0]].2.len()),
            _ => Err(format!("未定義のループ: {source}")),
        }
    }

    fn item_value(&mut self, _index: usize, path: &[usize], expr: &str) -> Result<Value, String> {
        let item = &self.items[path[0]];
        match (path.len(), expr) {
            (_, "品目") => Ok(json!(item.0)),
            (1, "数") => Ok(json!(item.1)),
            (1, "_n") => Ok(json!(path[0] + 1)),
            (2, "_n") => Ok(json!(path[1] + 1)),
            (2, "名") => Ok(json!(item.2[path[1]])),
            (_, e) => Err(format!("未定義: {e}")),
        }
    }
}

fn items() -> Vec<Item> {
    vec![("ねじ", 3, vec!["a", "b"]), ("板", 1, vec![])]
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("使い方: make_verification_set 出力先ディレクトリ");
        std::process::exit(2);
    });
    let out = Path::new(&out);
    std::fs::create_dir_all(out).expect("出力先を作れません");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut made = 0;
    let mut failed = 0;

    // (テンプレート, 0 件版も作るか)
    let mut templates: Vec<(std::path::PathBuf, bool)> = [
        "tests/fixtures/nested.docx",
        "tests/fixtures/nested-table.docx",
        "tests/fixtures/paragraphs.docx",
        "tests/fixtures/cell-paragraphs.docx",
        "tests/fixtures/nested.xlsx",
    ]
    .into_iter()
    .map(|p| (root.join(p), true))
    .collect();
    let mut extra: Vec<_> = std::fs::read_dir(root.join("verification/templates"))
        .expect("verification/templates がありません")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    extra.sort();
    templates.extend(extra.into_iter().map(|p| (p, false)));

    for (path, with_empty) in templates {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let kind = match path.extension().and_then(|e| e.to_str()) {
            Some("docx") => Kind::Docx,
            Some("xlsx") => Kind::Xlsx,
            _ => continue,
        };
        let template = std::fs::read(&path).expect("テンプレートを読めません");
        let stem = name
            .trim_end_matches(&format!(".{}", kind.extension()))
            .to_string();
        let mut cases = vec![(format!("{stem}.out.{}", kind.extension()), items())];
        if with_empty {
            cases.push((format!("{stem}.out-empty.{}", kind.extension()), vec![]));
        }
        for (file, items) in cases {
            match render_with_notes(kind, &template, &mut Sample { items }) {
                Ok((bytes, notes)) => {
                    std::fs::write(out.join(&file), bytes).expect("書き込めません");
                    println!(
                        "OK   {file}{}",
                        if notes.is_empty() {
                            String::new()
                        } else {
                            format!("  注意: {notes:?}")
                        }
                    );
                    made += 1;
                }
                Err(e) => {
                    println!("NG   {file}: {e}");
                    failed += 1;
                }
            }
        }
    }
    println!(
        "\n{made} 個を {} に作りました（失敗 {failed}）",
        out.display()
    );
    if failed > 0 {
        std::process::exit(1);
    }
}
