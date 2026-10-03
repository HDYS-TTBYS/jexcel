//! zip（docx / xlsx の実体）をメモリ上で読み書きする。テンプレートは小さいので、全エントリを読み込む。

use std::io::{Cursor, Read, Write};

use crate::{Error, Result};

pub struct Package {
    entries: Vec<(String, Vec<u8>)>,
}

impl Package {
    pub fn read(bytes: &[u8]) -> Result<Self> {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|e| Error::NotATemplate(format!("zip として読めません: {e}")))?;
        let mut entries = vec![];
        for i in 0..zip.len() {
            let mut f = zip.by_index(i).map_err(|e| Error::Zip(e.to_string()))?;
            if f.is_dir() {
                continue;
            }
            let mut buf = vec![];
            f.read_to_end(&mut buf)
                .map_err(|e| Error::Zip(e.to_string()))?;
            entries.push((f.name().to_string(), buf));
        }
        Ok(Self { entries })
    }

    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, b)| b.as_slice())
    }

    pub fn set(&mut self, name: &str, bytes: Vec<u8>) {
        match self.entries.iter_mut().find(|(n, _)| n == name) {
            Some((_, b)) => *b = bytes,
            None => self.entries.push((name.to_string(), bytes)),
        }
    }

    /// 部品を取り除く（無ければ何もしない）。
    pub fn remove(&mut self, name: &str) {
        self.entries.retain(|(n, _)| n != name);
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(n, _)| n.as_str())
    }

    /// 元の順序のまま書き出す（`[Content_Types].xml` が先頭のテンプレートは先頭のまま）。
    pub fn write(&self) -> Result<Vec<u8>> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in &self.entries {
            w.start_file(name, opts)
                .map_err(|e| Error::Zip(e.to_string()))?;
            w.write_all(bytes).map_err(|e| Error::Zip(e.to_string()))?;
        }
        Ok(w.finish()
            .map_err(|e| Error::Zip(e.to_string()))?
            .into_inner())
    }
}
