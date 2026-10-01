//! docx への差し込み。
//!
//! Word は 1 つの `{{差し込み欄}}` を、書式の違いなどで複数の「ラン」（`<w:r>`）に分けて保存することがある
//! （`{{取引` と `先}}` が別のラン、など）。そのため、段落ごとにランのテキストをつなげてから欄を探し、
//! 置換後の文字列は欄の最初のランに入れ（書式はそのランのものになる）、残りのランからは欄の部分を取り除く。

use serde_json::Value;

use crate::package::Package;
use crate::placeholder::{display, find};
use crate::xml::{self, Element, Node};
use crate::{Error, Result};

type Resolver<'a> = dyn FnMut(&str) -> std::result::Result<Value, String> + 'a;

/// 差し込みを行う部品（本文・ヘッダー・フッター・脚注）。
fn is_target(name: &str) -> bool {
    let Some(file) = name.strip_prefix("word/") else {
        return false;
    };
    file == "document.xml"
        || file == "footnotes.xml"
        || file == "endnotes.xml"
        || ((file.starts_with("header") || file.starts_with("footer")) && file.ends_with(".xml"))
}

pub fn process(pkg: &mut Package, resolve: &mut Resolver) -> Result<()> {
    let names: Vec<String> = pkg
        .names()
        .filter(|n| is_target(n))
        .map(String::from)
        .collect();
    for name in names {
        let mut doc = xml::parse(pkg.get(&name).expect("listed"))?;
        if walk(&mut doc.root, resolve)? {
            pkg.set(&name, xml::write(&doc)?);
        }
    }
    Ok(())
}

fn walk(el: &mut Element, resolve: &mut Resolver) -> Result<bool> {
    let mut changed = false;
    if el.name == "w:p" {
        changed |= paragraph(el, resolve)?;
    }
    for child in &mut el.children {
        if let Node::Element(c) = child {
            changed |= walk(c, resolve)?;
        }
    }
    Ok(changed)
}

/// 段落の `<w:t>` の位置（段落からの子の添字の並び）を文書順に集める。入れ子の段落（テキストボックス）は別に処理する。
fn collect_t(el: &Element, path: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
    for (i, c) in el.children.iter().enumerate() {
        let Node::Element(ce) = c else { continue };
        match ce.name.as_str() {
            "w:t" => {
                path.push(i);
                out.push(path.clone());
                path.pop();
            }
            "w:p" => {}
            _ => {
                path.push(i);
                collect_t(ce, path, out);
                path.pop();
            }
        }
    }
}

fn element_at<'a>(root: &'a mut Element, path: &[usize]) -> &'a mut Element {
    let mut cur = root;
    for &i in path {
        cur = match &mut cur.children[i] {
            Node::Element(e) => e,
            _ => unreachable!("collect_t が返す位置は要素"),
        };
    }
    cur
}

fn paragraph(p: &mut Element, resolve: &mut Resolver) -> Result<bool> {
    let mut paths = vec![];
    collect_t(p, &mut vec![], &mut paths);
    if paths.is_empty() {
        return Ok(false);
    }
    let mut texts: Vec<String> = paths.iter().map(|pa| element_at(p, pa).text()).collect();
    let starts: Vec<usize> = texts
        .iter()
        .scan(0, |acc, t| {
            let s = *acc;
            *acc += t.len();
            Some(s)
        })
        .collect();
    let full: String = texts.concat();
    let matches = find(&full);
    if matches.is_empty() {
        return Ok(false);
    }

    // 値は文書の順（前から）に解決する（欄の一覧や、最初に失敗した欄の報告が文書順になる）。
    // 置換は後ろの欄から行う（前の欄の位置が、後ろの置換でずれないように）。
    let mut replacements = Vec::with_capacity(matches.len());
    for m in &matches {
        let value = resolve(&m.expr).map_err(|message| Error::Expr {
            expr: m.expr.clone(),
            message,
        })?;
        replacements.push(display(&value));
    }

    let mut touched = vec![false; texts.len()];
    let piece_at = |pos: usize| {
        starts
            .iter()
            .rposition(|&s| s <= pos)
            .expect("位置は範囲内")
    };
    for (m, replacement) in matches.iter().zip(&replacements).rev() {
        let (a, b) = (piece_at(m.start), piece_at(m.end - 1));
        let prefix = texts[a][..m.start - starts[a]].to_string();
        let suffix = texts[b][m.end - starts[b]..].to_string();
        if a == b {
            texts[a] = format!("{prefix}{replacement}{suffix}");
        } else {
            texts[a] = format!("{prefix}{replacement}");
            for t in texts.iter_mut().take(b).skip(a + 1) {
                t.clear();
            }
            texts[b] = suffix;
        }
        for t in touched.iter_mut().take(b + 1).skip(a) {
            *t = true;
        }
    }

    // 文書順に書き戻す。改行を含む値は `<w:br/>` で区切る（後ろから処理して、前の添字をずらさない）
    for (i, path) in paths.iter().enumerate().rev() {
        if !touched[i] {
            continue;
        }
        let text = texts[i].replace("\r\n", "\n");
        let (idx, parent_path) = path.split_last().expect("t は段落の子孫");
        let parent = element_at(p, parent_path);
        let parts: Vec<&str> = text.split('\n').collect();
        let nodes: Vec<Node> = parts
            .iter()
            .enumerate()
            .flat_map(|(n, part)| {
                let mut t = Element::new("w:t");
                t.set_attr("xml:space", "preserve");
                t.set_text(part);
                let mut v = vec![];
                if n > 0 {
                    v.push(Node::Element(Element::new("w:br")));
                }
                v.push(Node::Element(t));
                v
            })
            .collect();
        parent.children.splice(*idx..=*idx, nodes);
    }
    Ok(true)
}
