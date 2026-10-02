//! docx への差し込み。
//!
//! Word は 1 つの `{{差し込み欄}}` を、書式の違いなどで複数の「ラン」（`<w:r>`）に分けて保存することがある
//! （`{{取引` と `先}}` が別のラン、など）。そのため、段落ごとにランのテキストをつなげてから欄を探し、
//! 置換後の文字列は欄の最初のランに入れ（書式はそのランのものになる）、残りのランからは欄の部分を取り除く。
//!
//! 段落のループ: 表の外（本文・ヘッダー・フッター・脚注）の段落に `{{#each 式}}` と書くと、その段落が要素の数だけ
//! 繰り返される。`{{/each}}` を別の段落に書くと、その間のブロック（段落・表）がひとまとめに繰り返される
//! （`{{#each}}` だけの段落・`{{/each}}` だけの段落は出力しない）。入れ子にもでき、間に挟んだ表の行のループは
//! 外側の要素の中で入れ子として展開される。表のセルの中の印は、その行のループの印。
//!
//! 行ループ: 表の行（`<w:tr>`）のどこかに `{{#each 式}}` と書くと、その行が式の配列の要素の数だけ繰り返される
//! （印は取り除かれ、行の中の欄は要素ごとに差し込まれる。要素が 0 件ならその行は消える）。
//! 別の行に `{{/each}}` と書くと、その間の行（印の行を含む）がひとまとめに繰り返される。
//! ループは入れ子にできる（`{{/each}}` を使う形の中に、さらに `{{#each}}`〜`{{/each}}` を書く。
//! 繰り返す行の中の表の中のループも同じ）。内側の式では外側の要素のキーも使える。

use serde_json::Value;

use crate::package::Package;
use crate::placeholder::{display, find};
use crate::xml::{self, Element, Node};
use crate::{count_blocks, marker, number, parse_blocks, Block, Error, Marker, Result, Source};

type Resolver<'a> = dyn FnMut(&str) -> std::result::Result<Value, String> + 'a;

/// 差し込みの途中経過。ループは文書の出現順（外側が先）に番号を振る。
struct Ctx<'a> {
    src: &'a mut dyn Source,
    next_loop: usize,
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
        if walk(&mut doc.root, &mut ctx, &mut vec![])? {
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

/// いまの位置: 外側から順に（ループの番号, 要素の位置）。ループの外なら空。
type Scope = Vec<(usize, usize)>;

fn path_of(scope: &Scope) -> Vec<usize> {
    scope.iter().map(|f| f.1).collect()
}

/// 段落のループ（表の外の `{{#each}}`）を書ける、ブロックの並びを直接持つ要素。
/// 表のセルは含めない（セルの中の印は、その行のループの印）。
fn is_block_container(name: &str) -> bool {
    matches!(
        name,
        "w:body" | "w:hdr" | "w:ftr" | "w:footnote" | "w:endnote"
    )
}

/// 要素 `el`（段落か表）を処理する。表は行のループを展開する。
fn visit(el: &mut Element, ctx: &mut Ctx, scope: &mut Scope) -> Result<bool> {
    if el.name == "w:tbl" {
        table(el, ctx, scope)
    } else {
        walk(el, ctx, scope)
    }
}

fn walk(el: &mut Element, ctx: &mut Ctx, scope: &mut Scope) -> Result<bool> {
    if is_block_container(&el.name) && direct_paragraph_markers(el).iter().any(|m| !m.is_empty()) {
        return block_container(el, ctx, scope);
    }
    let mut changed = false;
    if el.name == "w:p" {
        changed |= paragraph(el, &mut |expr| resolve(ctx, scope, expr))?;
    }
    // 行がすべて消えた表（Word は行のない表を開けない）
    let mut emptied = vec![];
    for (i, child) in el.children.iter_mut().enumerate() {
        let Node::Element(c) = child else { continue };
        if c.name == "w:tbl" {
            let t = table(c, ctx, scope)?;
            changed |= t;
            if t && !has_rows(c) {
                emptied.push(i);
            }
        } else {
            changed |= walk(c, ctx, scope)?;
        }
    }
    for i in emptied.into_iter().rev() {
        if el.name == "w:tc" {
            // セルは段落で終わらなければならないので、空の段落に置き換える
            el.children[i] = Node::Element(Element::new("w:p"));
        } else {
            el.children.remove(i);
        }
    }
    Ok(changed)
}

fn has_rows(tbl: &Element) -> bool {
    tbl.children
        .iter()
        .any(|n| matches!(n, Node::Element(e) if e.name == "w:tr"))
}

/// 直下の段落ごとの、ループの印（段落以外は空）。
fn direct_paragraph_markers(el: &Element) -> Vec<Vec<Marker>> {
    el.children
        .iter()
        .filter_map(|n| match n {
            Node::Element(e) => Some(e),
            _ => None,
        })
        .map(|e| {
            let mut found = vec![];
            if e.name == "w:p" {
                collect_markers(e, &mut found);
            }
            found
        })
        .collect()
}

/// 本文などの、表の外の段落のループ。`{{#each 式}}` の段落から `{{/each}}` の段落まで（表などの
/// 間のブロックを含む）が、要素の数だけ繰り返される。印だけの段落は出力しない。
fn block_container(el: &mut Element, ctx: &mut Ctx, scope: &mut Scope) -> Result<bool> {
    let children = std::mem::take(&mut el.children);
    // 要素（段落・表・節の設定など）ごとに、直前の要素以降のテキストなどをひとまとめにして持つ
    let mut items: Vec<TableRow> = vec![];
    let mut pending: Vec<Node> = vec![];
    for child in children {
        match child {
            Node::Element(e) => items.push(TableRow {
                before: std::mem::take(&mut pending),
                row: e,
            }),
            other => pending.push(other),
        }
    }
    let trailing = pending;

    let marks: Vec<Vec<Marker>> = items
        .iter()
        .map(|r| {
            let mut found = vec![];
            if r.row.name == "w:p" {
                collect_markers(&r.row, &mut found);
            }
            found
        })
        .collect();
    let mut blocks = parse_blocks(&marks)?;
    number(&mut blocks, &mut ctx.next_loop);
    // 中の表のループの番号は、ブロックを何回繰り返しても同じになるよう、要素ごとに先に確保する
    let mut cursor = ctx.next_loop;
    let mut base = Vec::with_capacity(items.len());
    for r in &items {
        base.push(cursor);
        cursor += if r.row.name == "w:tbl" {
            table_loops(&r.row)?
        } else {
            static_loops(&r.row)?
        };
    }
    let mut out: Vec<Node> = vec![];
    let mut changed = false;
    if !items.is_empty() {
        emit(
            &Emit {
                rows: &items,
                row_base: &base,
                drop_marker_paragraphs: true,
            },
            (0, items.len() - 1),
            &blocks,
            ctx,
            scope,
            &mut out,
            &mut changed,
        )?;
    }
    out.extend(trailing);
    el.children = out;
    ctx.next_loop = cursor;
    Ok(changed)
}

/// 段落のテキストが、ループの印を除くと空か。
fn is_marker_only(p: &Element) -> bool {
    let mut paths = vec![];
    collect_t(p, &mut vec![], &mut paths);
    let text: String = paths.iter().map(|pa| element_ref(p, pa).text()).collect();
    let found = find(&text);
    if !found.iter().any(|m| marker(&m.expr).is_some()) {
        return false;
    }
    let mut rest = String::new();
    let mut at = 0;
    for m in &found {
        rest.push_str(&text[at..m.start]);
        if marker(&m.expr).is_none() {
            rest.push_str(&text[m.start..m.end]);
        }
        at = m.end;
    }
    rest.push_str(&text[at..]);
    rest.trim().is_empty()
}

/// 表。行のループ（1 行ずつの `{{#each}}`、または `{{#each}}`〜`{{/each}}` で囲んだ複数の行）を展開する。
fn table(tbl: &mut Element, ctx: &mut Ctx, scope: &mut Scope) -> Result<bool> {
    let children = std::mem::take(&mut tbl.children);
    // 行ごとに、直前の行以降に出てきた行以外の要素（`tblPr` など）をひとまとめにして持つ
    let mut rows: Vec<TableRow> = vec![];
    let mut pending: Vec<Node> = vec![];
    for child in children {
        match child {
            Node::Element(e) if e.name == "w:tr" => rows.push(TableRow {
                before: std::mem::take(&mut pending),
                row: e,
            }),
            other => pending.push(other),
        }
    }
    let trailing = pending;

    let marks: Vec<Vec<Marker>> = rows
        .iter()
        .map(|r| {
            let mut found = vec![];
            collect_markers(&r.row, &mut found);
            found
        })
        .collect();
    let mut blocks = parse_blocks(&marks)?;
    number(&mut blocks, &mut ctx.next_loop);
    // この表の行の中にある入れ子の表のループの番号。行を繰り返しても同じ番号になるよう、行ごとに先に確保する
    let mut cursor = ctx.next_loop;
    let mut row_base = Vec::with_capacity(rows.len());
    for r in &rows {
        row_base.push(cursor);
        cursor += static_loops(&r.row)?;
    }

    let mut out: Vec<Node> = vec![];
    let mut changed = false;
    if rows.is_empty() {
        tbl.children = trailing;
        ctx.next_loop = cursor;
        return Ok(false);
    }
    emit(
        &Emit {
            rows: &rows,
            row_base: &row_base,
            drop_marker_paragraphs: false,
        },
        (0, rows.len() - 1),
        &blocks,
        ctx,
        scope,
        &mut out,
        &mut changed,
    )?;
    out.extend(trailing);
    tbl.children = out;
    ctx.next_loop = cursor;
    Ok(changed)
}

struct TableRow {
    before: Vec<Node>,
    row: Element,
}

struct Emit<'a> {
    rows: &'a [TableRow],
    row_base: &'a [usize],
    /// 印だけの段落を出力しない（本文の段落のループ）
    drop_marker_paragraphs: bool,
}

/// 行 `range`（両端を含む）を出力する。`blocks` の範囲は要素の数だけ繰り返し、それ以外の行はそのまま出す。
fn emit(
    e: &Emit,
    range: (usize, usize),
    blocks: &[Block],
    ctx: &mut Ctx,
    scope: &mut Scope,
    out: &mut Vec<Node>,
    changed: &mut bool,
) -> Result<()> {
    let mut i = range.0;
    while i <= range.1 {
        if let Some(b) = blocks.iter().find(|b| b.first == i) {
            let parent = scope.last().map(|f| f.0);
            let n = ctx
                .src
                .loop_len(b.index, parent, &path_of(scope), &b.source)
                .map_err(|m| expr_error(&format!("#each {}", b.source), m))?;
            *changed = true;
            for item in 0..n {
                scope.push((b.index, item));
                let r = emit(e, (b.first, b.last), &b.children, ctx, scope, out, changed);
                scope.pop();
                r?;
            }
            i = b.last + 1;
        } else {
            let r = &e.rows[i];
            if e.drop_marker_paragraphs && r.row.name == "w:p" && is_marker_only(&r.row) {
                *changed = true;
                i += 1;
                continue;
            }
            out.extend(r.before.iter().cloned());
            let mut row = r.row.clone();
            // 行の中の入れ子の表のループは、何回繰り返しても同じ番号から始める
            ctx.next_loop = e.row_base[i];
            let t = visit(&mut row, ctx, scope)?;
            *changed |= t;
            // 行がすべて消えた表は出力しない
            if !(t && row.name == "w:tbl" && !has_rows(&row)) {
                out.push(Node::Element(row));
            }
            i += 1;
        }
    }
    Ok(())
}

/// 表（入れ子の表を含む）の中のループの数。番号を静的に振るために使う。
fn static_loops(el: &Element) -> Result<usize> {
    let mut n = 0;
    for c in &el.children {
        let Node::Element(ce) = c else { continue };
        if ce.name == "w:tbl" {
            n += table_loops(ce)?;
        } else {
            n += static_loops(ce)?;
        }
    }
    Ok(n)
}

fn table_loops(tbl: &Element) -> Result<usize> {
    let rows: Vec<&Element> = tbl
        .children
        .iter()
        .filter_map(|n| match n {
            Node::Element(e) if e.name == "w:tr" => Some(e),
            _ => None,
        })
        .collect();
    let marks: Vec<Vec<Marker>> = rows
        .iter()
        .map(|r| {
            let mut found = vec![];
            collect_markers(r, &mut found);
            found
        })
        .collect();
    let mut n = count_blocks(&parse_blocks(&marks)?);
    for r in rows {
        n += static_loops(r)?;
    }
    Ok(n)
}

/// 欄の式を値にする。ループの印は、繰り返しの中では空文字（印を消す）、外ではエラー。
fn resolve(ctx: &mut Ctx, scope: &Scope, expr: &str) -> std::result::Result<Value, String> {
    if marker(expr).is_some() {
        return if scope.is_empty() {
            Err(
                "{{#each}} と {{/each}} は、表の行の中か、本文の段落（表の外）に書いてください"
                    .into(),
            )
        } else {
            Ok(Value::String(String::new()))
        };
    }
    match scope.last() {
        None => ctx.src.value(expr),
        Some(&(index, _)) => ctx.src.item_value(index, &path_of(scope), expr),
    }
}

/// 行（`<w:tr>`）の中にあるループの印（入れ子の表の中は、その表の行として別に扱う）。
fn collect_markers(el: &Element, found: &mut Vec<Marker>) {
    if el.name == "w:p" {
        let mut paths = vec![];
        collect_t(el, &mut vec![], &mut paths);
        let text: String = paths.iter().map(|p| element_ref(el, p).text()).collect();
        for m in find(&text) {
            if let Some(mk) = marker(&m.expr) {
                found.push(mk);
            }
        }
    }
    for c in &el.children {
        if let Node::Element(ce) = c {
            if ce.name != "w:tbl" {
                collect_markers(ce, found);
            }
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
