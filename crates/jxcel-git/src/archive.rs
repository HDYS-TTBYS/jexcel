//! 履歴を内包した単一ファイル。
//!
//! zip の中に、現在の状態（`manifest.json` / `sheets/**`）と、履歴のベアリポジトリ
//! （`history/HEAD` / `history/objects/**` / `history/refs/**`）を同居させる。
//! `jxcel-core` の `from_zip` は `history/` を無視するので、履歴を知らない読み手でも
//! 現在の状態は読める。開くときは履歴を一時ディレクトリに展開し、保存時に詰め直す。

use jxcel_core::tree::{read_zip_tree, write_zip_tree, FileTree};
use jxcel_core::JxcelFile;
use std::fs;
use std::path::Path;

use crate::{Error, History, Result};

const HISTORY_DIR: &str = "history";

pub struct Archive {
    // 履歴の作業場所。Archive が生きている間だけ存在する。
    dir: tempfile::TempDir,
    history: History,
}

impl Archive {
    /// 履歴が空の新規アーカイブ。
    pub fn create() -> Result<Self> {
        let dir = tempfile::tempdir().map_err(jxcel_core::Error::from)?;
        let history = History::open_or_init(dir.path())?;
        Ok(Self { dir, history })
    }

    /// 既存のファイルを開き、現在の状態と履歴を取り出す。履歴のないファイルでも開ける。
    pub fn open(bytes: &[u8]) -> Result<(Self, JxcelFile)> {
        let tree = read_zip_tree(bytes)?;
        let file = JxcelFile::from_tree(&tree)?;
        let archive = Self::create()?;
        let prefix = format!("{HISTORY_DIR}/");
        for (path, data) in &tree {
            if let Some(rel) = path.strip_prefix(&prefix) {
                let dest = archive.dir.path().join(rel);
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent).map_err(jxcel_core::Error::from)?;
                }
                fs::write(dest, data).map_err(jxcel_core::Error::from)?;
            }
        }
        // 展開した内容でリポジトリを開き直す
        let history = History::open_or_init(archive.dir.path())?;
        Ok((Self { history, ..archive }, file))
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    /// 現在の状態を履歴に記録し、ファイルのバイト列を返す。
    /// 内容が直前と同じならコミットは増えないが、バイト列は返す。
    pub fn save(&self, file: &JxcelFile, message: &str) -> Result<Vec<u8>> {
        self.history.commit(file, message)?;
        let mut tree: FileTree = file.to_tree()?;
        pack_dir(self.dir.path(), self.dir.path(), &mut tree)?;
        Ok(write_zip_tree(&tree)?)
    }
}

/// ベアリポジトリの中身を `history/` 配下として集める。パス順は zip 側で整う。
fn pack_dir(root: &Path, dir: &Path, out: &mut FileTree) -> Result<()> {
    let io = |e: std::io::Error| Error::from(jxcel_core::Error::from(e));
    for entry in fs::read_dir(dir).map_err(io)? {
        let path = entry.map_err(io)?.path();
        if path.is_dir() {
            pack_dir(root, &path, out)?;
        } else {
            let rel = path.strip_prefix(root).expect("under root");
            let rel = rel.to_string_lossy().replace('\\', "/");
            out.insert(format!("{HISTORY_DIR}/{rel}"), fs::read(&path).map_err(io)?);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jxcel_core::diff::Change;
    use jxcel_core::types::DataType;
    use jxcel_core::{Column, DataSchema, Row, Sheet};
    use serde_json::json;

    fn file(qty: i64) -> JxcelFile {
        let mut s = DataSchema::new("在庫", vec![Column::new("qty", "数量", DataType::Int)]);
        s.id = "s1".into();
        s.rows.push(Row {
            id: "r1".into(),
            cells: [("qty".to_string(), json!(qty))].into(),
        });
        JxcelFile {
            name: "台帳".into(),
            sheets: vec![Sheet {
                id: "sh1".into(),
                name: "倉庫".into(),
                schemas: vec![s],
            }],
        }
    }

    #[test]
    fn history_travels_inside_the_file() {
        let a = Archive::create().unwrap();
        let v1 = a.save(&file(1), "初回").unwrap();

        // 開き直して編集 → 保存 を繰り返しても履歴が引き継がれる
        let (a, f) = Archive::open(&v1).unwrap();
        assert_eq!(f, file(1));
        let v2 = a.save(&file(2), "数量変更").unwrap();

        let (a, f) = Archive::open(&v2).unwrap();
        assert_eq!(f, file(2));
        let log = a.history().log().unwrap();
        assert_eq!(
            log.iter().map(|c| c.message.as_str()).collect::<Vec<_>>(),
            ["数量変更", "初回"]
        );
        assert_eq!(a.history().load(&log[1].id).unwrap(), file(1));
        let d = a.history().diff(&log[1].id, &log[0].id).unwrap();
        assert!(matches!(&d[..], [Change::CellChanged { column, .. }] if column == "qty"));

        // 復元もファイルに残る
        let restored = a.history().restore(&log[1].id).unwrap();
        let v3 = a.save(&restored, "unused").unwrap();
        let (a, f) = Archive::open(&v3).unwrap();
        assert_eq!(f, file(1));
        assert_eq!(a.history().log().unwrap().len(), 3);
    }

    #[test]
    fn plain_reader_ignores_history_and_unchanged_save_is_stable() {
        let a = Archive::create().unwrap();
        let v1 = a.save(&file(1), "初回").unwrap();
        assert_eq!(JxcelFile::from_zip(&v1).unwrap(), file(1));
        assert!(read_zip_tree(&v1)
            .unwrap()
            .keys()
            .any(|k| k.starts_with("history/")));

        let (a, f) = Archive::open(&v1).unwrap();
        let again = a.save(&f, "変更なし").unwrap();
        assert_eq!(again, v1);
        assert_eq!(a.history().log().unwrap().len(), 1);
    }

    #[test]
    fn opens_file_without_history() {
        let plain = file(7).to_zip().unwrap();
        let (a, f) = Archive::open(&plain).unwrap();
        assert_eq!(f, file(7));
        assert!(a.history().log().unwrap().is_empty());
        let saved = a.save(&f, "取り込み").unwrap();
        assert_eq!(
            Archive::open(&saved)
                .unwrap()
                .0
                .history()
                .log()
                .unwrap()
                .len(),
            1
        );
    }
}
