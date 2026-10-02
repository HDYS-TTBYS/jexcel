//! docx への差し込み。
//!
//! Word は 1 つの `{{差し込み欄}}` を、書式の違いなどで複数の「ラン」（`<w:r>`）に分けて保存することがある
//! （`{{取引` と `先}}` が別のラン、など）。そのため、段落ごとにランのテキストをつなげてから欄を探し、
//! 置換後の文字列は欄の最初のランに入れ（書式はそのランのものになる）、残りのランからは欄の部分を取り除く。
//!
//! 行ループ: 表の行（`<w:tr>`）のどこかに `{{#each 式}}` と書くと、その行が式の配列の要素の数だけ繰り返される
//! （印は取り除かれ、行の中の欄は要素ごとに差し込まれる。要素が 0 件ならその行は消える）。ループの入れ子は使えない。

use serde_json::Value;

use crate::package::Package;
use crate::placeholder::{display, find};
use crate::xml::{self, Element, Node};
use crate::{loop_source, Error, Result, Source};

type Resolver<'a> = dyn FnMut(&str) -> std::result::Result<Value, String> + 'a;

/// 差し込みの途中経過。ループは文書の出現順に番号を振る。
struct Ctx<'a> {
    src: &'a mut dyn Source,
    next_loop: usize,
}

/// いまの位置: ループの外か、`index` 番目のループの `item` 番目の要素の行の中か。
#[derive(Clone, Copy)]
enum Scope {
    Top,
    Item { index: usize, item: usize },
}

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

pub fn process(pkg: &mut Package, src: &mut dyn Source) -> Result<()> {
    let names: Vec<String> = pkg
        .names()
        .filter(|n| is_target(n))
        .map(String::from)
        .collect();
    let mut ctx = Ctx { src, next_loop: 0 };
    for name in names {
        let mut doc = xml::parse(pkg.get(&name).expect("listed"))?;
        if walk(&mut doc.root, &mut ctx, Scope::Top)? {
            pkg.set(&name, xml::write(&doc)?);
        }
    }
    Ok(())
}

fn expr_error(expr: &str, message: impl Into<String>) -> Error {
    Error::Expr {
        expr: expr.to_string(),
        message: message.into(),
    }
}

fn walk(el: &mut Element, ctx: &mut Ctx, scope: Scope) -> Result<bool> {
    let mut changed = false;
    if el.name == "w:p" {
        changed |= paragraph(el, &mut |expr| resolve(ctx, scope, expr))?;
    }
    let children = std::mem::take(&mut el.children);
    let mut out = Vec::with_capacity(children.len());
    for child in children {
        let Node::Element(mut c) = child else {
            out.push(child);
            continue;
        };
        let marker = if c.name == "w:tr" {
            row_marker(&c)?
        } else {
            None
        };
        match (marker, scope) {
            (None, _) => {
                changed |= walk(&mut c, ctx, scope)?;
                out.push(Node::Element(c));
            }
            (Some(_), Scope::Item { .. }) => {
                return Err(expr_error(
                    "#each",
                    "行ループの中に行ループは書けません（入れ子は未対応です）",
                ));
            }
            (Some(source), Scope::Top) => {
                let index = ctx.next_loop;
                ctx.next_loop += 1;
                let n = ctx
                    .src
                    .loop_len(index, &source)
                    .map_err(|m| expr_error(&format!("#each {source}"), m))?;
                changed = true;
                for item in 0..n {
                    let mut row = c.clone();
                    walk(&mut row, ctx, Scope::Item { index, item })?;
                    out.push(Node::Element(row));
                }
            }
        }
    }
    el.children = out;
    Ok(changed)
}

/// 欄の式を値にする。ループの印は、繰り返しの中では空文字（印を消す）、外ではエラー。
fn resolve(ctx: &mut Ctx, scope: Scope, expr: &str) -> std::result::Result<Value, String> {
    if loop_source(expr).is_some() {
        return match scope {
            Scope::Item { .. } => Ok(Value::String(String::new())),
            Scope::Top => Err("{{#each}} は、表の行の中に書いてください".into()),
        };
    }
    match scope {
        Scope::Top => ctx.src.value(expr),
        Scope::Item { index, item } => ctx.src.item_value(index, item, expr),
    }
}

/// 行（`<w:tr>`）の中にある `{{#each 式}}` の式。複数あればエラー。
fn row_marker(tr: &Element) -> Result<Option<String>> {
    let mut found: Vec<String> = vec![];
    collect_markers(tr, &mut found);
    match found.len() {
        0 => Ok(None),
        1 => Ok(Some(found.remove(0))),
        _ => Err(expr_error(
            "#each",
            "1 つの行に {{#each}} は 1 つだけ書けます",
        )),
    }
}

fn collect_markers(el: &Element, found: &mut Vec<String>) {
    if el.name == "w:p" {
        let mut paths = vec![];
        collect_t(el, &mut vec![], &mut paths);
        let text: String = paths.iter().map(|p| element_ref(el, p).text()).collect();
        for m in find(&text) {
            if let Some(src) = loop_source(&m.expr) {
                found.push(src.to_string());
            }
        }
    }
    for c in &el.children {
        if let Node::Element(ce) = c {
            collect_markers(ce, found);
        }
    }
}

fn element_ref<'a>(root: &'a Element, path: &[usize]) -> &'a Element {
    let mut cur = root;
    for &i in path {
        cur = match &cur.children[i] {
            Node::Element(e) => e,
            _ => unreachable!("collect_t が返す位置は要素"),
        };
    }
    cur
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
