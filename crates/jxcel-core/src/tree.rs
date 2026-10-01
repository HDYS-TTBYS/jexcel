//! ファイルの永続化形式。
//!
//! メモリ上のモデルを「パス → バイト列」の展開ツリーに変換し、それを zip に詰める。
//! git にはこの展開ツリーをそのままコミットするので、行単位で意味のある差分になる。
//! 出力は決定的（キー順固定・行は ID 順・zip のタイムスタンプ固定）。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};

use crate::model::{Column, DataSchema, JxcelFile, Macro, Row, Sheet};
use crate::{Error, Result, FORMAT_VERSION};

pub type FileTree = BTreeMap<String, Vec<u8>>;

const MANIFEST: &str = "manifest.json";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    format_version: u32,
    name: String,
    sheets: Vec<String>,
    /// マクロの一覧（順序つき）。本体は `macros/<id>.ts`。旧形式のファイルには無い。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    macros: Vec<MacroMeta>,
}

#[derive(Serialize, Deserialize)]
struct MacroMeta {
    id: String,
    name: String,
}

#[derive(Serialize, Deserialize)]
struct SheetFile {
    id: String,
    name: String,
    schemas: Vec<SchemaFile>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SchemaFile {
    id: String,
    name: String,
    columns: Vec<Column>,
    row_order: Vec<String>,
}

fn sheet_path(sheet_id: &str) -> String {
    format!("sheets/{sheet_id}/sheet.json")
}

fn macro_path(id: &str) -> String {
    format!("macros/{id}.ts")
}

fn rows_path(sheet_id: &str, schema_id: &str) -> String {
    format!("sheets/{sheet_id}/{schema_id}.rows.jsonl")
}

fn to_json<T: Serialize>(v: &T) -> Vec<u8> {
    let mut s = serde_json::to_string_pretty(v).expect("serialize");
    s.push('\n');
    s.into_bytes()
}

fn from_json<T: for<'de> Deserialize<'de>>(path: &str, bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|source| Error::Json {
        path: path.into(),
        source,
    })
}

/// ID はパス要素として使うため、安全な文字だけを許可する。
fn check_id(id: &str) -> Result<()> {
    if !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        Ok(())
    } else {
        Err(Error::Invalid(format!("不正な ID: {id:?}")))
    }
}

impl JxcelFile {
    pub fn to_tree(&self) -> Result<FileTree> {
        let mut tree = FileTree::new();
        for sheet in &self.sheets {
            check_id(&sheet.id)?;
            let mut schemas = vec![];
            for s in &sheet.schemas {
                check_id(&s.id)?;
                schemas.push(SchemaFile {
                    id: s.id.clone(),
                    name: s.name.clone(),
                    columns: s.columns.clone(),
                    row_order: s.rows.iter().map(|r| r.id.clone()).collect(),
                });
                // 行本体は ID 順。並べ替えは row_order だけが変わる。
                let mut rows: Vec<&Row> = s.rows.iter().collect();
                rows.sort_by(|a, b| a.id.cmp(&b.id));
                let mut buf = vec![];
                for r in rows {
                    serde_json::to_writer(&mut buf, r).expect("serialize");
                    buf.push(b'\n');
                }
                tree.insert(rows_path(&sheet.id, &s.id), buf);
            }
            tree.insert(
                sheet_path(&sheet.id),
                to_json(&SheetFile {
                    id: sheet.id.clone(),
                    name: sheet.name.clone(),
                    schemas,
                }),
            );
        }
        // マクロのソースはそのままのバイト列で保存する（git の差分でコードとして読めるように）
        for m in &self.macros {
            check_id(&m.id)?;
            tree.insert(macro_path(&m.id), m.source.clone().into_bytes());
        }
        tree.insert(
            MANIFEST.into(),
            to_json(&Manifest {
                format_version: FORMAT_VERSION,
                name: self.name.clone(),
                sheets: self.sheets.iter().map(|s| s.id.clone()).collect(),
                macros: self
                    .macros
                    .iter()
                    .map(|m| MacroMeta {
                        id: m.id.clone(),
                        name: m.name.clone(),
                    })
                    .collect(),
            }),
        );
        Ok(tree)
    }

    pub fn from_tree(tree: &FileTree) -> Result<Self> {
        let get = |p: &str| tree.get(p).ok_or_else(|| Error::NotFound(p.into()));
        let manifest: Manifest = from_json(MANIFEST, get(MANIFEST)?)?;
        if manifest.format_version != FORMAT_VERSION {
            return Err(Error::UnsupportedVersion(manifest.format_version));
        }
        let mut sheets = vec![];
        for sid in &manifest.sheets {
            check_id(sid)?;
            let p = sheet_path(sid);
            let sf: SheetFile = from_json(&p, get(&p)?)?;
            let mut schemas = vec![];
            for sc in sf.schemas {
                check_id(&sc.id)?;
                let rp = rows_path(sid, &sc.id);
                let mut by_id: BTreeMap<String, Row> = BTreeMap::new();
                for line in get(&rp)?.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
                    let row: Row = from_json(&rp, line)?;
                    by_id.insert(row.id.clone(), row);
                }
                let mut rows = vec![];
                for id in &sc.row_order {
                    rows.push(
                        by_id
                            .remove(id)
                            .ok_or_else(|| Error::Invalid(format!("{rp}: 行 {id} がありません")))?,
                    );
                }
                if let Some(extra) = by_id.keys().next() {
                    return Err(Error::Invalid(format!("{rp}: rowOrder にない行 {extra}")));
                }
                schemas.push(DataSchema {
                    id: sc.id,
                    name: sc.name,
                    columns: sc.columns,
                    rows,
                });
            }
            sheets.push(Sheet {
                id: sf.id,
                name: sf.name,
                schemas,
            });
        }
        let mut macros = vec![];
        for meta in manifest.macros {
            check_id(&meta.id)?;
            let p = macro_path(&meta.id);
            let source = String::from_utf8(get(&p)?.clone())
                .map_err(|_| Error::Invalid(format!("{p}: UTF-8 ではありません")))?;
            macros.push(Macro {
                id: meta.id,
                name: meta.name,
                source,
            });
        }
        Ok(JxcelFile {
            name: manifest.name,
            sheets,
            macros,
        })
    }

    /// zip（ファイルの実体）に書き出す。同じ内容なら同じバイト列になる。
    pub fn to_zip(&self) -> Result<Vec<u8>> {
        write_zip_tree(&self.to_tree()?)
    }

    pub fn from_zip(bytes: &[u8]) -> Result<Self> {
        Self::from_tree(&read_zip_tree(bytes)?)
    }
}

/// 展開ツリーを zip に詰める。パス順・固定タイムスタンプで決定的。
pub fn write_zip_tree(tree: &FileTree) -> Result<Vec<u8>> {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (path, bytes) in tree {
        w.start_file(path, opts)?;
        w.write_all(bytes)?;
    }
    Ok(w.finish()?.into_inner())
}

pub fn read_zip_tree(bytes: &[u8]) -> Result<FileTree> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut tree = FileTree::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i)?;
        if f.is_dir() {
            continue;
        }
        let name = f.name().to_string();
        if name.starts_with('/') || name.split('/').any(|c| c == ".." || c.is_empty()) {
            return Err(Error::Invalid(format!("不正なパス: {name}")));
        }
        let mut buf = vec![];
        f.read_to_end(&mut buf)?;
        tree.insert(name, buf);
    }
    Ok(tree)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::types::DataType;
    use serde_json::json;

    pub fn sample() -> JxcelFile {
        let mut schema = DataSchema::new(
            "在庫",
            vec![
                Column::new("name", "品名", DataType::String).required(),
                Column::new("qty", "数量", DataType::Int),
                Column::new("meta", "備考", DataType::Any),
            ],
        );
        schema.id = "s1".into();
        for (id, name, qty) in [("r1", "ねじ", 10), ("r2", "ナット", 5)] {
            schema.rows.push(Row {
                id: id.into(),
                cells: [
                    ("name".to_string(), json!(name)),
                    ("qty".to_string(), json!(qty)),
                    ("meta".to_string(), json!({"z": 1, "a": [true, null]})),
                ]
                .into(),
            });
        }
        JxcelFile {
            name: "台帳".into(),
            sheets: vec![Sheet {
                id: "sh1".into(),
                name: "倉庫".into(),
                schemas: vec![schema],
            }],
            macros: Default::default(),
        }
    }

    #[test]
    fn roundtrip_tree_and_zip() {
        let f = sample();
        assert_eq!(JxcelFile::from_tree(&f.to_tree().unwrap()).unwrap(), f);
        assert_eq!(JxcelFile::from_zip(&f.to_zip().unwrap()).unwrap(), f);
    }

    #[test]
    fn deterministic() {
        let f = sample();
        assert_eq!(f.to_zip().unwrap(), f.to_zip().unwrap());
    }

    #[test]
    fn reorder_only_touches_order() {
        let a = sample();
        let mut b = a.clone();
        b.sheets[0].schemas[0].rows.reverse();
        let (ta, tb) = (a.to_tree().unwrap(), b.to_tree().unwrap());
        let changed: Vec<_> = ta.keys().filter(|k| ta[*k] != tb[*k]).collect();
        assert_eq!(changed, vec!["sheets/sh1/sheet.json"]);
        assert_eq!(JxcelFile::from_tree(&tb).unwrap(), b);
    }

    #[test]
    fn macros_roundtrip_as_plain_ts_files() {
        let mut f = sample();
        let src = "export default (jx: Jxcel) => {\n  jx.log(\"こんにちは\");\n};\n";
        f.macros.push(Macro {
            id: "m1".into(),
            name: "あいさつ".into(),
            source: src.into(),
        });
        let tree = f.to_tree().unwrap();
        // ソースは変換されず、そのままのバイト列で入る
        assert_eq!(tree["macros/m1.ts"], src.as_bytes());
        assert_eq!(JxcelFile::from_tree(&tree).unwrap(), f);
        assert_eq!(JxcelFile::from_zip(&f.to_zip().unwrap()).unwrap(), f);
        // マクロの編集は、そのマクロのファイルだけを変える
        let mut g = f.clone();
        g.macros[0].source.push_str("// 追記\n");
        let (a, b) = (f.to_tree().unwrap(), g.to_tree().unwrap());
        let changed: Vec<_> = a.keys().filter(|k| a[*k] != b[*k]).collect();
        assert_eq!(changed, vec!["macros/m1.ts"]);
    }

    #[test]
    fn old_files_without_macros_still_open() {
        // manifest に macros がない旧形式
        let mut t = sample().to_tree().unwrap();
        t.insert(
            MANIFEST.into(),
            r#"{"formatVersion":1,"name":"台帳","sheets":["sh1"]}"#
                .as_bytes()
                .to_vec(),
        );
        let f = JxcelFile::from_tree(&t).unwrap();
        assert!(f.macros.is_empty());
        // マクロのないファイルの manifest には macros キーを出さない（既存ファイルの差分を増やさない）
        let out = sample().to_tree().unwrap();
        assert!(!String::from_utf8_lossy(&out[MANIFEST]).contains("macros"));
    }

    #[test]
    fn rejects_bad_input() {
        let mut t = sample().to_tree().unwrap();
        t.remove("sheets/sh1/s1.rows.jsonl");
        assert!(matches!(JxcelFile::from_tree(&t), Err(Error::NotFound(_))));

        let mut f = sample();
        f.sheets[0].id = "../x".into();
        assert!(f.to_tree().is_err());

        let mut t = sample().to_tree().unwrap();
        t.insert(
            MANIFEST.into(),
            br#"{"formatVersion":99,"name":"","sheets":[]}"#.to_vec(),
        );
        assert!(matches!(
            JxcelFile::from_tree(&t),
            Err(Error::UnsupportedVersion(99))
        ));
    }
}
