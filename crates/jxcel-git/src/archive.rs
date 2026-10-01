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
    // フィールドは宣言順に破棄される。Windows は開いたままのファイルを含むディレクトリを
    // 消せないので、先に履歴（リポジトリのハンドル）を閉じてから作業場所を消す。
    history: History,
    // 履歴の作業場所。Archive が生きている間だけ存在する。
    dir: tempfile::TempDir,
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

    /// 現在の状態を履歴に記録し、ファイルのバイト列を返す。履歴は 1 つのパックにまとめて詰める。
    /// 内容が直前と同じならコミットは増えないが、バイト列は返す。
    pub fn save(&mut self, file: &JxcelFile, message: &str) -> Result<Vec<u8>> {
        self.history.commit(file, message)?;
        self.history.pack()?;
        let mut tree: FileTree = file.to_tree()?;
        pack_dir(self.dir.path(), self.dir.path(), &mut tree)?;
        Ok(write_zip_tree(&tree)?)
    }
}

/// zip に入れるリポジトリ内のトップレベル項目。`git init` が複製するテンプレート
/// （hooks のサンプル等）や作業用の一時ファイルは、マシン依存のゴミなので入れない。
const KEEP: [&str; 4] = ["HEAD", "config", "objects", "refs"];

/// ベアリポジトリの中身を `history/` 配下として集める。パス順は zip 側で整う。
fn pack_dir(root: &Path, dir: &Path, out: &mut FileTree) -> Result<()> {
    let io = |e: std::io::Error| Error::from(jxcel_core::Error::from(e));
    for entry in fs::read_dir(dir).map_err(io)? {
        let entry = entry.map_err(io)?;
        if dir == root && !KEEP.iter().any(|k| entry.file_name() == *k) {
            continue;
        }
        let path = entry.path();
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
        let mut a = Archive::create().unwrap();
        let v1 = a.save(&file(1), "初回").unwrap();

        // 開き直して編集 → 保存 を繰り返しても履歴が引き継がれる
        let (mut a, f) = Archive::open(&v1).unwrap();
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
        let mut a = a;
        let v3 = a.save(&restored, "unused").unwrap();
        let (a, f) = Archive::open(&v3).unwrap();
        assert_eq!(f, file(1));
        assert_eq!(a.history().log().unwrap().len(), 3);
    }

    #[test]
    fn plain_reader_ignores_history_and_unchanged_save_is_stable() {
        let mut a = Archive::create().unwrap();
        let v1 = a.save(&file(1), "初回").unwrap();
        assert_eq!(JxcelFile::from_zip(&v1).unwrap(), file(1));
        assert!(read_zip_tree(&v1)
            .unwrap()
            .keys()
            .any(|k| k.starts_with("history/")));

        let (mut a, f) = Archive::open(&v1).unwrap();
        let again = a.save(&f, "変更なし").unwrap();
        assert_eq!(again, v1);
        assert_eq!(a.history().log().unwrap().len(), 1);
    }

    fn history_entries(zip: &[u8]) -> Vec<String> {
        read_zip_tree(zip)
            .unwrap()
            .into_keys()
            .filter(|k| k.starts_with("history/objects/"))
            .collect()
    }

    #[test]
    fn history_is_packed_and_stays_small() {
        // 200 行のファイルで、毎回 1 行だけ変えて 30 回保存する（実運用に近い変更）
        let big = |n: i64| {
            let mut f = file(0);
            let s = &mut f.sheets[0].schemas[0];
            s.rows = (0..200)
                .map(|i| Row {
                    id: format!("r{i:03}"),
                    cells: [("qty".to_string(), json!(if i == n { 1000 + n } else { i }))].into(),
                })
                .collect();
            f
        };
        let mut a = Archive::create().unwrap();
        let mut zip = vec![];
        let mut first_size = 0;
        for n in 0..30 {
            zip = a.save(&big(n), &format!("v{n}")).unwrap();
            if n == 0 {
                first_size = zip.len();
            }
            a = Archive::open(&zip).unwrap().0;
        }
        eprintln!("1世代: {first_size} バイト / 30世代: {} バイト", zip.len());

        // 履歴のファイル数は一定（パック 1 組）で、テンプレート等のゴミは入らない
        let tree = read_zip_tree(&zip).unwrap();
        let history: Vec<_> = tree.keys().filter(|k| k.starts_with("history/")).collect();
        let pack_files = history
            .iter()
            .filter(|k| k.starts_with("history/objects/pack/pack-"))
            .count();
        assert_eq!(pack_files, 2, "{history:?}"); // .pack と .idx
        for k in &history {
            let top = k.split('/').nth(1).unwrap();
            assert!(KEEP.contains(&top), "想定外のエントリ: {k}");
        }
        // 同じ履歴をパックしないで詰めた場合（旧形式）より、はっきり小さい
        let loose = Archive::create().unwrap();
        for n in 0..30 {
            loose.history().commit(&big(n), &format!("v{n}")).unwrap();
        }
        let mut loose_tree = big(29).to_tree().unwrap();
        pack_dir(loose.dir.path(), loose.dir.path(), &mut loose_tree).unwrap();
        let loose_len = write_zip_tree(&loose_tree).unwrap().len();
        eprintln!("パックなし: {loose_len} バイト");
        assert!(
            zip.len() * 2 < loose_len,
            "packed={} loose={loose_len}",
            zip.len()
        );

        // パック後も全リビジョンを読め、差分も取れる
        let (a, f) = Archive::open(&zip).unwrap();
        assert_eq!(f, big(29));
        let log = a.history().log().unwrap();
        assert_eq!(log.len(), 30);
        assert_eq!(a.history().load(&log[29].id).unwrap(), big(0));
        // v28 → v29: 28 行目が元に戻り、29 行目が変わる
        assert_eq!(a.history().diff(&log[1].id, &log[0].id).unwrap().len(), 2);
    }

    #[test]
    fn packing_is_stable_and_upgrades_loose_archives() {
        // 緩いオブジェクトのままの旧形式（pack を呼ばずに詰めたもの）
        let a = Archive::create().unwrap();
        a.history().commit(&file(1), "初回").unwrap();
        a.history().commit(&file(2), "変更").unwrap();
        let mut tree = file(2).to_tree().unwrap();
        pack_dir(a.dir.path(), a.dir.path(), &mut tree).unwrap();
        let legacy = write_zip_tree(&tree).unwrap();
        assert!(history_entries(&legacy)
            .iter()
            .any(|e| !e.contains("/pack/")));

        // 開いて保存すると、履歴は保ったままパックに移行する
        let (mut a, f) = Archive::open(&legacy).unwrap();
        let packed = a.save(&f, "unused").unwrap();
        assert!(history_entries(&packed)
            .iter()
            .all(|e| e.starts_with("history/objects/pack/")));
        assert_eq!(a.history().log().unwrap().len(), 2);

        // 変更なしで開き直して保存しても、バイト列は変わらない
        let (mut a, f) = Archive::open(&packed).unwrap();
        assert_eq!(a.save(&f, "変更なし").unwrap(), packed);
        let (a, _) = Archive::open(&packed).unwrap();
        assert_eq!(a.history().log().unwrap().len(), 2);
    }

    #[test]
    fn repeated_saves_on_one_archive() {
        // 開き直さずに保存を繰り返す（アプリのセッションと同じ使い方）。
        // Windows は開いたままのパックを削除できないので、ここで失敗しないことが重要。
        let mut a = Archive::create().unwrap();
        let mut last = vec![];
        for n in 0..5 {
            last = a.save(&file(n), &format!("v{n}")).unwrap();
        }
        assert_eq!(a.history().log().unwrap().len(), 5);
        let (b, f) = Archive::open(&last).unwrap();
        assert_eq!(f, file(4));
        assert_eq!(b.history().log().unwrap().len(), 5);
        assert_eq!(history_entries(&last).len(), 2);
    }

    #[test]
    fn opens_file_without_history() {
        let plain = file(7).to_zip().unwrap();
        let (mut a, f) = Archive::open(&plain).unwrap();
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
