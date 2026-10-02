//! 内蔵 git による履歴管理。ユーザーには git を見せず「履歴・差分・復元」として提供する。
//!
//! 履歴はベアリポジトリに保存し、コミットの中身は `jxcel-core` の展開ツリー
//! （manifest.json / sheets/**）そのもの。そのため標準の git ツールでも行単位で読める。
//! リポジトリの置き場所は呼び出し側が決める。

use git2::{ObjectType, Oid, Repository, Signature, TreeWalkMode, TreeWalkResult};
use jxcel_core::diff::{diff, Change};
use jxcel_core::tree::FileTree;
use jxcel_core::JxcelFile;
pub mod archive;

use std::collections::BTreeMap;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("git error: {0}")]
    Git(#[from] git2::Error),
    #[error(transparent)]
    Core(#[from] jxcel_core::Error),
    #[error("revision not found: {0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct CommitInfo {
    pub id: String,
    pub message: String,
    /// UNIX 秒
    pub time: i64,
}

pub struct History {
    repo: Repository,
}

impl History {
    /// `dir` に履歴リポジトリを開く。なければ作る。
    pub fn open_or_init(dir: &Path) -> Result<Self> {
        let repo = match Repository::open_bare(dir) {
            Ok(r) => r,
            Err(_) => {
                // テンプレート（hooks のサンプル等）を複製しない。マシンごとに内容が違い、無駄に大きいので。
                let mut opts = git2::RepositoryInitOptions::new();
                opts.bare(true).external_template(false);
                Repository::init_opts(dir, &opts)?
            }
        };
        Ok(Self { repo })
    }

    /// 現在の状態を履歴に記録する。直前と同一内容なら何もせず `None`。
    pub fn commit(&self, file: &JxcelFile, message: &str) -> Result<Option<String>> {
        let tree_oid = self.write_tree(&file.to_tree()?)?;
        let parent = self.head_commit()?;
        if parent.as_ref().is_some_and(|p| p.tree_id() == tree_oid) {
            return Ok(None);
        }
        let tree = self.repo.find_tree(tree_oid)?;
        let sig = Signature::now("jxcel", "jxcel@localhost")?;
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        let oid = self
            .repo
            .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)?;
        Ok(Some(oid.to_string()))
    }

    /// 新しい順のコミット一覧。履歴がなければ空。
    pub fn log(&self) -> Result<Vec<CommitInfo>> {
        if self.head_commit()?.is_none() {
            return Ok(vec![]);
        }
        let mut walk = self.repo.revwalk()?;
        walk.push_head()?;
        walk.set_sorting(git2::Sort::TOPOLOGICAL)?;
        let mut out = vec![];
        for oid in walk {
            let c = self.repo.find_commit(oid?)?;
            out.push(CommitInfo {
                id: c.id().to_string(),
                message: c.message().unwrap_or("").trim_end().to_string(),
                time: c.time().seconds(),
            });
        }
        Ok(out)
    }

    /// 指定リビジョン時点のファイル状態。
    pub fn load(&self, rev: &str) -> Result<JxcelFile> {
        let oid = Oid::from_str(rev).map_err(|_| Error::NotFound(rev.into()))?;
        let commit = self
            .repo
            .find_commit(oid)
            .map_err(|_| Error::NotFound(rev.into()))?;
        let mut tree = FileTree::new();
        let mut walk_err = None;
        commit.tree()?.walk(TreeWalkMode::PreOrder, |dir, entry| {
            if entry.kind() == Some(ObjectType::Blob) {
                match self.repo.find_blob(entry.id()) {
                    Ok(b) => {
                        tree.insert(
                            format!("{dir}{}", entry.name().unwrap_or("")),
                            b.content().to_vec(),
                        );
                    }
                    Err(e) => {
                        walk_err = Some(e);
                        return TreeWalkResult::Abort;
                    }
                }
            }
            TreeWalkResult::Ok
        })?;
        if let Some(e) = walk_err {
            return Err(e.into());
        }
        Ok(JxcelFile::from_tree(&tree)?)
    }

    /// 2 つのリビジョン間の構造的な差分。
    pub fn diff(&self, from: &str, to: &str) -> Result<Vec<Change>> {
        Ok(diff(&self.load(from)?, &self.load(to)?))
    }

    /// 過去のリビジョンに戻す。履歴は書き換えず、復元結果を新しいコミットとして追加する。
    pub fn restore(&self, rev: &str) -> Result<JxcelFile> {
        let file = self.load(rev)?;
        self.commit(&file, &format!("復元: {}", &rev[..rev.len().min(8)]))?;
        Ok(file)
    }

    /// 緩いオブジェクトを 1 つのパックにまとめる（差分圧縮が効き、ファイル数も一定になる）。
    ///
    /// 緩いオブジェクトがなければ何もしない。内容が変わらない保存でバイト列が変わらないための条件でもある。
    /// パックは単一スレッドで作るので、同じ履歴からは同じパックができる。
    pub fn pack(&mut self) -> Result<()> {
        if self.head_commit()?.is_none() {
            return Ok(());
        }
        let objects = self.repo.path().join("objects");
        let loose: Vec<_> = read_dir(&objects)?
            .into_iter()
            .filter(|p| is_loose_dir(p))
            .collect();
        if loose.is_empty() {
            return Ok(());
        }

        let mut walk = self.repo.revwalk()?;
        walk.push_head()?;
        let mut builder = self.repo.packbuilder()?;
        builder.set_threads(1);
        builder.insert_walk(&mut walk)?;
        let pack_dir = objects.join("pack");
        std::fs::create_dir_all(&pack_dir).map_err(io)?;
        let before = read_dir(&pack_dir)?;
        builder.write(&pack_dir, 0)?;
        let after = read_dir(&pack_dir)?;

        // 今回書いたパック（増えたファイル）だけを残し、古いパックを消す。
        // 同一内容のパックが既にあって増えなかった場合は、何も消さない。
        let written: Vec<_> = after.iter().filter(|p| !before.contains(p)).collect();
        let stale: Vec<_> = if written.is_empty() { vec![] } else { before };

        // Windows では、libgit2 がメモリマップしているパックファイルは削除できない。
        // 古いパックを開いたままのハンドルを手放すため、リポジトリを開き直してから消す。
        drop(builder);
        drop(walk);
        let git_dir = self.repo.path().to_owned();
        self.repo = Repository::open_bare(&git_dir)?;
        for old in stale {
            std::fs::remove_file(&old).map_err(io)?;
        }
        for dir in loose {
            std::fs::remove_dir_all(&dir).map_err(io)?;
        }
        Ok(())
    }

    fn head_commit(&self) -> Result<Option<git2::Commit<'_>>> {
        match self.repo.head() {
            Ok(h) => Ok(Some(h.peel_to_commit()?)),
            Err(e)
                if e.code() == git2::ErrorCode::UnbornBranch
                    || e.code() == git2::ErrorCode::NotFound =>
            {
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// 作業ディレクトリを持たないので、blob を直接書いてツリーを組み立てる。
    fn write_tree(&self, files: &FileTree) -> Result<Oid> {
        let entries: Vec<(&str, &[u8])> = files
            .iter()
            .map(|(p, b)| (p.as_str(), b.as_slice()))
            .collect();
        self.build_tree(&entries)
    }

    /// `entries` は現在のディレクトリからの相対パス。先頭要素ごとにまとめて再帰する。
    fn build_tree(&self, entries: &[(&str, &[u8])]) -> Result<Oid> {
        let mut builder = self.repo.treebuilder(None)?;
        let mut subdirs: BTreeMap<&str, Vec<(&str, &[u8])>> = BTreeMap::new();
        for (path, bytes) in entries {
            match path.split_once('/') {
                Some((dir, rest)) => subdirs.entry(dir).or_default().push((rest, bytes)),
                None => {
                    builder.insert(path, self.repo.blob(bytes)?, 0o100644)?;
                }
            }
        }
        for (dir, sub) in subdirs {
            builder.insert(dir, self.build_tree(&sub)?, 0o040000)?;
        }
        Ok(builder.write()?)
    }
}

fn io(e: std::io::Error) -> Error {
    jxcel_core::Error::from(e).into()
}

fn read_dir(dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    let mut out = vec![];
    for entry in std::fs::read_dir(dir).map_err(io)? {
        out.push(entry.map_err(io)?.path());
    }
    out.sort();
    Ok(out)
}

/// `objects/ab/` のような、緩いオブジェクトのディレクトリか。
fn is_loose_dir(path: &Path) -> bool {
    path.is_dir()
        && path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.len() == 2 && n.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;
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
            macros: Default::default(),
            exports: Default::default(),
            forms: Default::default(),
            templates: Default::default(),
        }
    }

    #[test]
    fn commit_log_diff_restore() {
        let dir = tempfile::tempdir().unwrap();
        let h = History::open_or_init(dir.path()).unwrap();
        assert!(h.log().unwrap().is_empty());

        let c1 = h.commit(&file(1), "初回").unwrap().unwrap();
        assert!(h.commit(&file(1), "変更なし").unwrap().is_none());
        let c2 = h.commit(&file(2), "数量変更").unwrap().unwrap();

        let log = h.log().unwrap();
        assert_eq!(
            log.iter().map(|c| c.message.as_str()).collect::<Vec<_>>(),
            ["数量変更", "初回"]
        );

        assert_eq!(h.load(&c1).unwrap(), file(1));
        let d = h.diff(&c1, &c2).unwrap();
        assert!(
            matches!(&d[..], [Change::CellChanged { row, column, old, new, .. }]
            if row == "r1" && column == "qty" && *old == json!(1) && *new == json!(2))
        );

        assert_eq!(h.restore(&c1).unwrap(), file(1));
        let log = h.log().unwrap();
        assert_eq!(log.len(), 3);
        assert!(log[0].message.starts_with("復元"));
        assert_eq!(h.load(&log[0].id).unwrap(), file(1));
    }

    #[test]
    fn reopen_and_bad_rev() {
        let dir = tempfile::tempdir().unwrap();
        let c = History::open_or_init(dir.path())
            .unwrap()
            .commit(&file(1), "a")
            .unwrap()
            .unwrap();
        let h = History::open_or_init(dir.path()).unwrap();
        assert_eq!(h.log().unwrap().len(), 1);
        assert_eq!(h.load(&c).unwrap(), file(1));
        assert!(matches!(h.load("zzz"), Err(Error::NotFound(_))));
        assert!(matches!(h.load(&"0".repeat(40)), Err(Error::NotFound(_))));
    }
}
