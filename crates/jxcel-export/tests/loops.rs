//! 行ループ（`{{#each 式}}`）・日付・ヘッダー/フッターの差し込みのテスト。
//! テンプレートは tools/make_export_fixtures.py で生成してコミットしてある。

use jxcel_export::package::Package;
use jxcel_export::xml::{self, Element, Node};
use jxcel_export::{plan, render_with, Error, Kind, LoopPlan, Source};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const DOCX: &[u8] = include_bytes!("fixtures/loop.docx");
const XLSX: &[u8] = include_bytes!("fixtures/loop.xlsx"); // 共有文字列（Excel と同じ形）
const XLSX_INLINE: &[u8] = include_bytes!("fixtures/loop-inline.xlsx");

/// 1 つの行ループを持つテンプレート用の取得元。
struct Fake {
    top: BTreeMap<&'static str, Value>,
    items: Vec<Value>,
    unit: i64,
}

impl Fake {
    fn new(items: Vec<Value>) -> Self {
        Self {
            top: BTreeMap::from([
                ("取引先", json!("A社")),
                ("請求番号", json!("INV-7")),
                ("発行日", json!("2026-10-02")),
                ("合計", json!(999)),
            ]),
            items,
            unit: 100,
        }
    }
}

impl Source for Fake {
    fn value(&mut self, expr: &str) -> Result<Value, String> {
        self.top
            .get(expr)
            .cloned()
            .ok_or_else(|| format!("未定義: {expr}"))
    }
    fn loop_len(
        &mut self,
        index: usize,
        parent: Option<usize>,
        path: &[usize],
        source: &str,
    ) -> Result<usize, String> {
        assert_eq!((index, parent, source), (0, None, "明細"));
        assert!(path.is_empty());
        Ok(self.items.len())
    }
    fn item_value(&mut self, index: usize, path: &[usize], expr: &str) -> Result<Value, String> {
        assert_eq!(index, 0);
        let item = path[0];
        let it = &self.items[item];
        match expr {
            "_n" => Ok(json!(item + 1)),
            "取引先" => self.value(expr),
            "数 * 単価" => Ok(json!(it["数"].as_i64().unwrap() * self.unit)),
            e => it.get(e).cloned().ok_or_else(|| format!("未定義: {e}")),
        }
    }
}

fn items(n: usize) -> Vec<Value> {
    (1..=n)
        .map(|i| json!({"品目": format!("品{i}"), "数": i * 10}))
        .collect()
}

fn part(zip: &[u8], name: &str) -> Element {
    let pkg = Package::read(zip).unwrap();
    xml::parse(
        pkg.get(name)
            .unwrap_or_else(|| panic!("{name} がありません")),
    )
    .unwrap()
    .root
}

fn elements<'a>(el: &'a Element, local: &str, out: &mut Vec<&'a Element>) {
    for c in &el.children {
        if let Node::Element(e) = c {
            if e.local() == local {
                out.push(e);
            }
            elements(e, local, out);
        }
    }
}

// ---------------------------------------------------------------- docx

/// 表の各行の、セルごとのテキスト。
fn table_rows(zip: &[u8]) -> Vec<Vec<String>> {
    let root = part(zip, "word/document.xml");
    let mut rows = vec![];
    elements(&root, "tr", &mut rows);
    rows.iter()
        .map(|tr| {
            let mut cells = vec![];
            elements(tr, "tc", &mut cells);
            cells
                .iter()
                .map(|tc| {
                    let mut ts = vec![];
                    elements(tc, "t", &mut ts);
                    ts.iter().map(|t| t.text()).collect::<String>()
                })
                .collect()
        })
        .collect()
}

fn docx(items: Vec<Value>) -> Vec<Vec<String>> {
    let out =
        render_with(Kind::Docx, DOCX, &mut Fake::new(items)).unwrap_or_else(|e| panic!("{e}"));
    table_rows(&out)
}

#[test]
fn docx_plan_lists_loop_placeholders_separately() {
    let p = plan(Kind::Docx, DOCX).unwrap();
    assert_eq!(p.placeholders, ["取引先", "合計"]);
    assert_eq!(
        p.loops,
        [LoopPlan {
            source: "明細".into(),
            parent: None,
            exprs: vec![
                "品目".into(),
                "数 * 単価".into(),
                "_n".into(),
                "取引先".into()
            ],
        }]
    );
}

#[test]
fn docx_row_repeats_once_per_item_with_the_marker_removed() {
    let rows = docx(items(3));
    assert_eq!(rows.len(), 1 + 3 + 2, "{rows:?}");
    assert_eq!(rows[0], ["品名", "数量", "金額"]);
    // 印は、複数のランに分かれていても取り除かれ、残りの欄は要素ごとに入る
    assert_eq!(rows[1], ["品1", "1000", "1/A社"]);
    assert_eq!(rows[2], ["品2", "2000", "2/A社"]);
    assert_eq!(rows[3], ["品3", "3000", "3/A社"]);
    // ループの外の行は、ループの外の値で、そのまま
    assert_eq!(rows[4], ["合計", "", "999"]);
    assert_eq!(rows[5], ["ループの外の行", "", ""]);
}

#[test]
fn docx_zero_items_removes_the_row_and_one_item_keeps_it() {
    let rows = docx(vec![]);
    assert_eq!(rows.len(), 1 + 2, "{rows:?}");
    assert_eq!(rows[1][0], "合計");
    assert_eq!(docx(items(1)).len(), 1 + 1 + 2);
}

#[test]
fn docx_loop_errors_are_reported() {
    // 対象の式が失敗
    struct Bad;
    impl Source for Bad {
        fn value(&mut self, _: &str) -> Result<Value, String> {
            Ok(Value::Null)
        }
        fn loop_len(
            &mut self,
            _: usize,
            _: Option<usize>,
            _: &[usize],
            _: &str,
        ) -> Result<usize, String> {
            Err("明細 is not defined".into())
        }
        fn item_value(&mut self, _: usize, _: &[usize], _: &str) -> Result<Value, String> {
            unreachable!()
        }
    }
    let e = render_with(Kind::Docx, DOCX, &mut Bad).unwrap_err();
    assert!(
        matches!(&e, Error::Expr { expr, message } if expr == "#each 明細" && message.contains("not defined")),
        "{e}"
    );

    // 閉じる印だけがある（対応する {{#each}} がない）
    let mut doc = jxcel_export::package::Package::read(DOCX).unwrap();
    let xml_text = String::from_utf8(doc.get("word/document.xml").unwrap().to_vec()).unwrap();
    doc.set(
        "word/document.xml",
        xml_text.replace("宛先: ", "宛先: {{/each}}").into_bytes(),
    );
    let out = doc.write().unwrap();
    let e = render_with(Kind::Docx, &out, &mut Fake::new(items(1))).unwrap_err();
    assert!(e.to_string().contains("each"), "{e}");
}

// ---------------------------------------------------------------- xlsx

#[derive(Debug, PartialEq)]
struct C {
    t: Option<String>,
    v: String,
    f: Option<String>,
    s: Option<String>,
}

fn shared_strings(zip: &[u8]) -> Vec<String> {
    if Package::read(zip)
        .unwrap()
        .get("xl/sharedStrings.xml")
        .is_none()
    {
        return vec![];
    }
    let root = part(zip, "xl/sharedStrings.xml");
    let mut sis = vec![];
    elements(&root, "si", &mut sis);
    sis.iter()
        .map(|si| {
            let mut ts = vec![];
            elements(si, "t", &mut ts);
            ts.iter().map(|t| t.text()).collect()
        })
        .collect()
}

fn sheet(zip: &[u8]) -> (BTreeMap<String, C>, Element) {
    let shared = shared_strings(zip);
    let root = part(zip, "xl/worksheets/sheet1.xml");
    let mut cs = vec![];
    elements(&root, "c", &mut cs);
    let mut out = BTreeMap::new();
    for c in cs {
        let mut v = String::new();
        let mut f = None;
        for n in &c.children {
            if let Node::Element(e) = n {
                match e.local() {
                    "v" => v.push_str(&e.text()),
                    "f" => f = Some(e.text()),
                    "is" => {
                        let mut ts = vec![];
                        elements(e, "t", &mut ts);
                        v.extend(ts.iter().map(|t| t.text()));
                    }
                    _ => {}
                }
            }
        }
        // 共有文字列のままのセル（欄のないセル）は、文字列を引いて「文字列のセル」として扱う
        let mut t = c.attr("t").map(String::from);
        if t.as_deref() == Some("s") {
            v = shared[v.parse::<usize>().unwrap()].clone();
            t = Some("inlineStr".into());
        }
        out.insert(
            c.attr("r").unwrap().to_string(),
            C {
                t,
                v,
                f,
                s: c.attr("s").map(String::from),
            },
        );
    }
    (out, root)
}

fn xlsx(template: &[u8], items: Vec<Value>) -> (BTreeMap<String, C>, Element) {
    let out =
        render_with(Kind::Xlsx, template, &mut Fake::new(items)).unwrap_or_else(|e| panic!("{e}"));
    sheet(&out)
}

fn text(c: &BTreeMap<String, C>, r: &str) -> String {
    c.get(r).map_or("<なし>".into(), |c| c.v.clone())
}

fn formula(c: &BTreeMap<String, C>, r: &str) -> String {
    c.get(r)
        .and_then(|c| c.f.clone())
        .unwrap_or_else(|| "<なし>".into())
}

fn check_xlsx_loop(template: &[u8]) {
    let (c, root) = xlsx(template, items(3));

    // ループの外（上）。日付の表示形式のセルは Excel の日付（数値）、そうでないセルは文字列のまま
    assert_eq!(text(&c, "A1"), "請求書 INV-7");
    assert_eq!((c["B2"].t.as_deref(), c["B2"].v.as_str()), (None, "46297"));
    assert_eq!(c["B3"].t.as_deref(), Some("inlineStr"));
    assert_eq!(text(&c, "B3"), "2026-10-02");
    assert_eq!(text(&c, "C3"), "2026-10-02 発行");

    // ループの行（5 行目）が 3 行に: 品目・数・数式・_n が行ごとに入り、印は消える
    for (i, row) in (5..=7).enumerate() {
        let n = i + 1;
        assert_eq!(text(&c, &format!("A{row}")), format!("品{n}"), "A{row}");
        assert_eq!(text(&c, &format!("B{row}")), (n * 10).to_string());
        assert_eq!(c[&format!("B{row}")].t, None, "数値のセル");
        assert_eq!(formula(&c, &format!("C{row}")), format!("B{row}*100"));
        assert_eq!(text(&c, &format!("D{row}")), n.to_string());
    }
    // 後ろの行は 2 行下へ。ループの行を含む範囲は広がり、後ろの行への参照はずれる
    assert_eq!(text(&c, "A8"), "合計");
    assert_eq!(formula(&c, "B8"), "SUM(B5:B7)");
    // 数式セルの古い計算結果は捨てる（LibreOffice などが差し込み前の値を表示しないように）
    assert_eq!((c["B8"].v.as_str(), c["B8"].t.as_deref()), ("", None));
    assert_eq!(formula(&c, "C8"), "SUM(C5:C7)");
    assert_eq!(text(&c, "A9"), "ループの外");
    assert_eq!(formula(&c, "B9"), "B8+1");
    assert_eq!(text(&c, "A10"), "結合セル（ループの外）");
    assert!(!c.contains_key("A11"));
    // 元の位置に古い内容が残っていない
    assert!(c.keys().all(|k| !k.ends_with("11")));

    // 結合セル・範囲の定義もずれる
    let mut merges = vec![];
    elements(&root, "mergeCell", &mut merges);
    assert_eq!(
        merges
            .iter()
            .map(|m| m.attr("ref").unwrap())
            .collect::<Vec<_>>(),
        ["A10:B10"]
    );
    let mut dim = vec![];
    elements(&root, "dimension", &mut dim);
    assert_eq!(dim[0].attr("ref"), Some("A1:D10"));

    // 行番号が連続し、重複しない
    let mut rows = vec![];
    elements(&root, "row", &mut rows);
    let nums: Vec<u32> = rows
        .iter()
        .map(|r| r.attr("r").unwrap().parse().unwrap())
        .collect();
    assert_eq!(nums, (1..=10).collect::<Vec<_>>());

    // ヘッダー・フッター（値の & は && にする）
    let mut hf = vec![];
    elements(&root, "headerFooter", &mut hf);
    let mut odd = vec![];
    elements(hf[0], "oddHeader", &mut odd);
    assert!(odd[0].text().contains("請求書 INV-7"), "{}", odd[0].text());
    assert!(!odd[0].text().contains("{{"));
    let mut foot = vec![];
    elements(hf[0], "oddFooter", &mut foot);
    assert!(foot[0].text().contains("A社"), "{}", foot[0].text());
}

#[test]
fn xlsx_row_repeats_with_shared_strings() {
    check_xlsx_loop(XLSX);
}

#[test]
fn xlsx_row_repeats_with_inline_strings() {
    check_xlsx_loop(XLSX_INLINE);
}

#[test]
fn xlsx_one_item_keeps_the_layout_and_zero_items_leave_an_empty_row() {
    let (c, _) = xlsx(XLSX_INLINE, items(1));
    assert_eq!(text(&c, "A5"), "品1");
    assert_eq!(formula(&c, "B6"), "SUM(B5:B5)");
    assert_eq!(text(&c, "A6"), "合計");

    // 0 件: 行は消さずに空にする（数式の参照が壊れないように）
    let (c, root) = xlsx(XLSX_INLINE, vec![]);
    assert_eq!(text(&c, "A5"), "");
    assert_eq!(text(&c, "B5"), "");
    assert_eq!(text(&c, "D5"), "");
    assert_eq!(formula(&c, "B6"), "SUM(B5:B5)");
    assert_eq!(text(&c, "A6"), "合計");
    let mut rows = vec![];
    elements(&root, "row", &mut rows);
    assert_eq!(rows.len(), 8);
}

#[test]
fn xlsx_plan_lists_loop_placeholders_and_header_footer_fields() {
    let p = plan(Kind::Xlsx, XLSX_INLINE).unwrap();
    assert!(p.placeholders.contains(&"請求番号".to_string()));
    assert!(p.placeholders.contains(&"取引先".to_string())); // フッター
    assert_eq!(p.loops.len(), 1);
    assert_eq!(p.loops[0].source, "明細");
    assert_eq!(p.loops[0].exprs, ["品目", "数", "_n"]);
}

#[test]
fn xlsx_marker_outside_a_row_cell_and_double_markers_are_errors() {
    // 1 行に 2 つの印
    let mut pkg = Package::read(XLSX_INLINE).unwrap();
    let s = String::from_utf8(pkg.get("xl/worksheets/sheet1.xml").unwrap().to_vec()).unwrap();
    pkg.set(
        "xl/worksheets/sheet1.xml",
        s.replace("{{&#25968;}}", "{{#each 別}}{{&#25968;}}")
            .into_bytes(),
    );
    let e = render_with(Kind::Xlsx, &pkg.write().unwrap(), &mut Fake::new(items(1))).unwrap_err();
    assert!(e.to_string().contains("1 つだけ"), "{e}");
}

#[test]
fn rendering_with_loops_is_repeatable() {
    let a = render_with(Kind::Xlsx, XLSX, &mut Fake::new(items(3))).unwrap();
    let b = render_with(Kind::Xlsx, XLSX, &mut Fake::new(items(3))).unwrap();
    assert_eq!(a, b);
    let a = render_with(Kind::Docx, DOCX, &mut Fake::new(items(3))).unwrap();
    let b = render_with(Kind::Docx, DOCX, &mut Fake::new(items(3))).unwrap();
    assert_eq!(a, b);
}

// ---------------------------------------------------------------- 入れ子のループ

const NESTED_DOCX: &[u8] = include_bytes!("fixtures/nested.docx");
const NESTED_TABLE_DOCX: &[u8] = include_bytes!("fixtures/nested-table.docx");
const NESTED_XLSX: &[u8] = include_bytes!("fixtures/nested.xlsx");

/// 明細（品目・数・付属品の名前）を 2 段のループで持つ取得元。外側 = 0（明細）、内側 = 1（付属。外側の要素ごと）。
struct Nest {
    items: Vec<(&'static str, i64, Vec<&'static str>)>,
}

impl Nest {
    fn sample() -> Self {
        Self {
            items: vec![("ねじ", 3, vec!["a", "b"]), ("板", 1, vec![])],
        }
    }
}

impl Source for Nest {
    fn value(&mut self, expr: &str) -> Result<Value, String> {
        match expr {
            "請求番号" => Ok(json!("INV-7")),
            "合計" => Ok(json!(999)),
            e => Err(format!("未定義: {e}")),
        }
    }
    fn loop_len(
        &mut self,
        index: usize,
        parent: Option<usize>,
        path: &[usize],
        source: &str,
    ) -> Result<usize, String> {
        match (index, parent, source) {
            (0, None, "明細") => {
                assert!(path.is_empty());
                Ok(self.items.len())
            }
            (1, Some(0), "付属") => Ok(self.items[path[0]].2.len()),
            other => panic!("想定外のループ: {other:?} {path:?}"),
        }
    }
    fn item_value(&mut self, index: usize, path: &[usize], expr: &str) -> Result<Value, String> {
        let item = &self.items[path[0]];
        match (index, expr) {
            (0, "_n") => Ok(json!(path[0] + 1)),
            (0, "品目") | (1, "品目") => Ok(json!(item.0)),
            (0, "数") => Ok(json!(item.1)),
            (1, "_n") => Ok(json!(path[1] + 1)),
            (1, "名") => Ok(json!(item.2[path[1]])),
            (i, e) => Err(format!("未定義: {i} {e}")),
        }
    }
}

#[test]
fn nested_loop_plan_records_the_parent() {
    for (kind, tpl) in [(Kind::Docx, NESTED_DOCX), (Kind::Xlsx, NESTED_XLSX)] {
        let p = plan(kind, tpl).unwrap();
        assert_eq!(p.loops.len(), 2, "{kind:?}");
        assert_eq!(
            (p.loops[0].source.as_str(), p.loops[0].parent),
            ("明細", None)
        );
        assert_eq!(
            (p.loops[1].source.as_str(), p.loops[1].parent),
            ("付属", Some(0))
        );
        assert!(p.loops[1].exprs.contains(&"名".to_string()));
        assert_eq!(p.chain(1), [0, 1]);
        assert_eq!((p.sibling_pos(0), p.sibling_pos(1)), (0, 0));
    }
}

#[test]
fn docx_nested_block_loops_repeat_groups_of_rows() {
    let out =
        render_with(Kind::Docx, NESTED_DOCX, &mut Nest::sample()).unwrap_or_else(|e| panic!("{e}"));
    let rows = table_rows(&out);
    let strs: Vec<Vec<&str>> = rows
        .iter()
        .map(|r| r.iter().map(String::as_str).collect())
        .collect();
    assert_eq!(
        strs,
        [
            vec!["品名", "付属品"],
            // 1 つ目の明細: 見出し・付属品 2 行・小計
            vec!["1. ねじ", ""],
            vec!["a", "ねじの付属 1"],
            vec!["b", "ねじの付属 2"],
            vec!["小計 3", ""],
            // 2 つ目の明細: 付属品は 0 件なので行が消える
            vec!["2. 板", ""],
            vec!["小計 1", ""],
            vec!["合計", ""],
        ]
    );
    // 要素が 0 件なら、外側の行ごと消える
    struct Empty;
    impl Source for Empty {
        fn value(&mut self, _: &str) -> Result<Value, String> {
            Ok(Value::Null)
        }
        fn loop_len(
            &mut self,
            i: usize,
            _: Option<usize>,
            _: &[usize],
            _: &str,
        ) -> Result<usize, String> {
            assert_eq!(i, 0, "外側が 0 件なら、内側のループは聞かれない");
            Ok(0)
        }
        fn item_value(&mut self, _: usize, _: &[usize], _: &str) -> Result<Value, String> {
            unreachable!()
        }
    }
    let out = render_with(Kind::Docx, NESTED_DOCX, &mut Empty).unwrap();
    assert_eq!(table_rows(&out).len(), 2); // 見出しの行と合計
}

#[test]
fn docx_loops_inside_a_nested_table_are_numbered_once() {
    let out = render_with(Kind::Docx, NESTED_TABLE_DOCX, &mut Nest::sample())
        .unwrap_or_else(|e| panic!("{e}"));
    let rows = table_rows(&out);
    // 外側の行（ねじ・板）と、その中の表の行（付属品）。外側の行の文字には、中の表の文字も含まれる
    let texts: Vec<String> = rows.iter().map(|r| r.concat()).collect();
    assert_eq!(texts[0], "品名付属品");
    assert!(
        texts[1].starts_with("ねじ") && texts[1].ends_with("ねじ:aねじ:b"),
        "{texts:?}"
    );
    assert_eq!(&texts[2..4], ["ねじ:a", "ねじ:b"]);
    assert!(texts[4].starts_with("板"), "{texts:?}");
    // 計画: 外側が 0、中の表のループが 1（外側の要素ごとに同じ番号）
    let p = plan(Kind::Docx, NESTED_TABLE_DOCX).unwrap();
    assert_eq!(
        (p.loops[1].parent, p.loops[1].source.as_str()),
        (Some(0), "付属")
    );
}

#[test]
fn nested_loop_markers_must_be_balanced() {
    fn with_text(old: &str, new: &str) -> Vec<u8> {
        let mut doc = Package::read(NESTED_DOCX).unwrap();
        let x = String::from_utf8(doc.get("word/document.xml").unwrap().to_vec()).unwrap();
        assert!(x.contains(old), "{old}");
        doc.set("word/document.xml", x.replace(old, new).into_bytes());
        doc.write().unwrap()
    }
    // 外側を閉じ忘れた
    let e = render_with(
        Kind::Docx,
        &with_text("{{/each}}小計", "小計"),
        &mut Nest::sample(),
    )
    .unwrap_err();
    assert!(e.to_string().contains("閉じて"), "{e}");
    // 余分な {{/each}}
    let e = render_with(
        Kind::Docx,
        &with_text("合計", "{{/each}}合計"),
        &mut Nest::sample(),
    )
    .unwrap_err();
    assert!(e.to_string().contains("対応する"), "{e}");
}

#[test]
fn xlsx_nested_loops_shift_rows_formulas_and_merges() {
    let out =
        render_with(Kind::Xlsx, NESTED_XLSX, &mut Nest::sample()).unwrap_or_else(|e| panic!("{e}"));
    let (c, root) = sheet(&out);
    // 1 つ目の明細（行 3〜6）: 見出し・付属品 2 行・小計
    assert_eq!(text(&c, "A1"), "請求書 INV-7");
    assert_eq!(
        [text(&c, "A3"), text(&c, "B3"), formula(&c, "C3")],
        ["ねじ", "3", "B3*100"]
    );
    assert_eq!(
        [text(&c, "A4"), text(&c, "B4"), formula(&c, "C4")],
        ["a", "ねじ", "C3"]
    );
    assert_eq!(
        [text(&c, "A5"), text(&c, "B5"), formula(&c, "C5")],
        ["b", "ねじ", "C3"]
    ); // 同じ回の見出しの行を指す
    assert_eq!(text(&c, "A6"), "小計");
    assert_eq!(
        (formula(&c, "B6"), formula(&c, "C6")),
        ("SUM(B3:B5)".into(), "SUM(C3:C5)".into())
    );
    // 2 つ目の明細（行 7〜9）: 付属品は 0 件 → 空の行を 1 行残す
    assert_eq!(
        [text(&c, "A7"), text(&c, "B7"), formula(&c, "C7")],
        ["板", "1", "B7*100"]
    );
    assert_eq!([text(&c, "A8"), text(&c, "B8")], ["", ""]);
    assert_eq!(formula(&c, "C8"), "C7");
    assert_eq!(text(&c, "A9"), "小計");
    assert_eq!(
        (formula(&c, "B9"), formula(&c, "C9")),
        ("SUM(B7:B8)".into(), "SUM(C7:C8)".into())
    );
    // ループの後ろ: 範囲は最後のコピーまで広がり、1 つの参照は最初のコピーを指す
    assert_eq!(text(&c, "A10"), "総合計");
    assert_eq!(formula(&c, "B10"), "SUM(B3:B9)");
    assert_eq!(formula(&c, "C10"), "C6+1");
    // 内側の繰り返しごとに結合セルが作られる
    let mut merges = vec![];
    elements(&root, "mergeCell", &mut merges);
    let refs: Vec<&str> = merges.iter().map(|m| m.attr("ref").unwrap()).collect();
    assert_eq!(refs, ["C4:D4", "C5:D5", "C8:D8"]);
}

#[test]
fn xlsx_defined_names_follow_the_shifted_rows() {
    let out =
        render_with(Kind::Xlsx, NESTED_XLSX, &mut Nest::sample()).unwrap_or_else(|e| panic!("{e}"));
    let wb = part(&out, "xl/workbook.xml");
    let mut names = vec![];
    elements(&wb, "definedName", &mut names);
    let got: BTreeMap<String, String> = names
        .iter()
        .map(|n| (n.attr("name").unwrap().to_string(), n.text()))
        .collect();
    // 範囲の終わりは最後のコピーまで広がり、1 つの参照は最初のコピーを指す
    assert_eq!(got["_xlnm.Print_Area"], "'明細'!$A$1:$D$10"); // 引用符つきのシート名
    assert_eq!(got["合計セル"], "明細!$B$10"); // 6 行目（合計）は 10 行目へ
    assert_eq!(got["グループ"], "明細!$A$3:$C$9");
    // ループより前の行だけの範囲（印刷タイトル）と、ほかのシートへの参照は変わらない
    assert_eq!(got["_xlnm.Print_Titles"], "'明細'!$1:$2");
    assert_eq!(got["別シート"], "Sheet9!$A$6");

    // ループのないテンプレートの定義名は触らない
    let out = render_with(Kind::Xlsx, XLSX, &mut Fake::new(items(1))).unwrap();
    let before = part(XLSX, "xl/workbook.xml");
    let after = part(&out, "xl/workbook.xml");
    let (mut a, mut b) = (vec![], vec![]);
    elements(&before, "definedName", &mut a);
    elements(&after, "definedName", &mut b);
    assert_eq!(
        a.iter().map(|n| n.text()).collect::<Vec<_>>(),
        b.iter().map(|n| n.text()).collect::<Vec<_>>()
    );
}

/// 入れ子のテンプレートに、手を入れたもの: 明細シートに自分を名前つきで指す参照と共有数式、
/// 別のシート（集計）に、明細を指す数式（共有数式を含む）を足す。
fn nested_xlsx_with_other_references() -> Vec<u8> {
    let mut pkg = Package::read(NESTED_XLSX).unwrap();
    let edit = |pkg: &mut Package, name: &str, f: &dyn Fn(String) -> String| {
        let text = String::from_utf8(pkg.get(name).unwrap().to_vec()).unwrap();
        pkg.set(name, f(text).into_bytes());
    };
    edit(&mut pkg, "xl/worksheets/sheet1.xml", &|t| {
        t
            // 自分を名前つきで指す参照（3 行目 = 見出しの行。同じ回の見出しを指す）と、ループの外の行を指す参照
            .replace(
                "<f>B3*100</f><v></v></c></row>",
                "<f>B3*100</f><v></v></c><c r=\"D3\"><f>明細!B3+1</f></c><c r=\"E3\"><f t=\"shared\" ref=\"E3:E4\" si=\"0\">B3+1</f></c></row>",
            )
            .replace(
                "<f>C3</f><v></v></c></row>",
                "<f>C3</f><v></v></c><c r=\"E4\"><f t=\"shared\" si=\"0\"/></c></row>",
            )
            // ループの外の共有数式
            .replace(
                "<f>C5+1</f><v></v></c></row>",
                "<f>C5+1</f><v></v></c><c r=\"D6\"><f t=\"shared\" ref=\"D6:E6\" si=\"1\">B6*2</f></c><c r=\"E6\"><f t=\"shared\" si=\"1\"/></c><c r=\"F6\"><f>'明細'!$B$6+Sheet2!A1</f></c></row>",
            )
    });
    pkg.set(
        "xl/worksheets/sheet2.xml",
        concat!(
            "<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>",
            "<row r=\"1\"><c r=\"A1\"><f>明細!B6</f></c><c r=\"B1\"><f>SUM(明細!C3:C5)</f></c><c r=\"C1\"><f>'明細'!$A$1&amp;\"明細!B6\"</f></c></row>",
            "<row r=\"3\"><c r=\"A3\"><f t=\"shared\" ref=\"A3:A4\" si=\"0\">明細!B5</f></c></row>",
            "<row r=\"4\"><c r=\"A4\"><f t=\"shared\" si=\"0\"/></c></row>",
            "</sheetData></worksheet>"
        )
        .as_bytes()
        .to_vec(),
    );
    edit(&mut pkg, "xl/workbook.xml", &|t| {
        t.replace(
            "</sheets>",
            "<sheet xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" name=\"Sheet2\" sheetId=\"2\" state=\"visible\" r:id=\"rId9\"/></sheets>",
        )
    });
    edit(&mut pkg, "xl/_rels/workbook.xml.rels", &|t| {
        t.replace(
            "</Relationships>",
            "<Relationship Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"/xl/worksheets/sheet2.xml\" Id=\"rId9\"/></Relationships>",
        )
    });
    edit(&mut pkg, "[Content_Types].xml", &|t| {
        t.replace(
            "</Types>",
            "<Override PartName=\"/xl/worksheets/sheet2.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/></Types>",
        )
    });
    pkg.write().unwrap()
}

fn formulas_of(zip: &[u8], name: &str) -> BTreeMap<String, String> {
    let root = part(zip, name);
    let mut cs = vec![];
    elements(&root, "c", &mut cs);
    cs.iter()
        .filter_map(|c| {
            let mut f = vec![];
            elements(c, "f", &mut f);
            let f = f.first()?;
            assert!(
                f.attr("t").is_none(),
                "共有数式が残っている: {:?}",
                c.attr("r")
            );
            Some((c.attr("r").unwrap().to_string(), f.text()))
        })
        .collect()
}

#[test]
fn xlsx_formulas_follow_rows_through_qualified_references_other_sheets_and_shared_formulas() {
    let template = nested_xlsx_with_other_references();
    let out =
        render_with(Kind::Xlsx, &template, &mut Nest::sample()).unwrap_or_else(|e| panic!("{e}"));

    // 明細シート。1 つ目の明細（行 3〜6）・2 つ目（行 7〜9）・総合計（行 10）
    let f = formulas_of(&out, "xl/worksheets/sheet1.xml");
    // 自分を名前つきで指す参照も、同じ回の見出しの行を指す
    assert_eq!(f["D3"], "明細!B3+1");
    assert_eq!(f["D7"], "明細!B7+1");
    // 共有数式は 1 つずつの数式になり、それぞれの回の行を指す（E4 は元の `B4+1`）
    assert_eq!(f["E3"], "B3+1");
    assert_eq!(f["E4"], "B4+1");
    assert_eq!(f["E5"], "B5+1");
    assert_eq!(f["E7"], "B7+1");
    assert_eq!(f["E8"], "B8+1");
    // ループの外の共有数式は、ずれた行に合わせる（D6:E6 → D10:E10。E は元の `C6*2`）
    assert_eq!(f["D10"], "B10*2");
    assert_eq!(f["E10"], "C10*2");
    // 名前つき（引用符あり）でループの外の行を指す参照と、ほかのシートへの参照
    assert_eq!(f["F10"], "'明細'!$B$10+Sheet2!A1");

    // 別のシート: 明細への名前つきの参照がずれる（1 つの参照は最初のコピー、範囲は最後のコピーまで）
    let g = formulas_of(&out, "xl/worksheets/sheet2.xml");
    assert_eq!(g["A1"], "明細!B10");
    assert_eq!(g["B1"], "SUM(明細!C3:C9)");
    assert_eq!(g["C1"], "'明細'!$A$1&\"明細!B6\""); // 文字列リテラルの中は触らない
                                                    // 共有数式: 元は A3 = 明細!B5、A4 = 明細!B6。それぞれの行へ
    assert_eq!(g["A3"], "明細!B6");
    assert_eq!(g["A4"], "明細!B10");
}

#[test]
fn xlsx_references_in_sheets_without_loops_are_left_alone() {
    // ずれたシートを指さない数式しかないシートは、バイト列も変わらない
    let template = nested_xlsx_with_other_references();
    let mut pkg = Package::read(&template).unwrap();
    pkg.set(
        "xl/worksheets/sheet2.xml",
        br#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><f t="shared" ref="A1:A2" si="0">B1+Sheet9!A1</f></c></row><row r="2"><c r="A2"><f t="shared" si="0"/></c></row></sheetData></worksheet>"#.to_vec(),
    );
    let before = pkg.get("xl/worksheets/sheet2.xml").unwrap().to_vec();
    let out = render_with(Kind::Xlsx, &pkg.write().unwrap(), &mut Nest::sample()).unwrap();
    assert_eq!(
        Package::read(&out)
            .unwrap()
            .get("xl/worksheets/sheet2.xml")
            .unwrap(),
        &before[..]
    );
}

// ---------------------------------------------------------------- 段落のループ

const PARAGRAPHS_DOCX: &[u8] = include_bytes!("fixtures/paragraphs.docx");

/// 本文の直下のブロックを、段落は文字列・表は `[行1|行2]` にして文書順に並べる。
fn body_blocks(zip: &[u8]) -> Vec<String> {
    let root = part(zip, "word/document.xml");
    let mut bodies = vec![];
    elements(&root, "body", &mut bodies);
    let text_of = |e: &Element| {
        let mut ts = vec![];
        elements(e, "t", &mut ts);
        ts.iter().map(|t| t.text()).collect::<String>()
    };
    bodies[0]
        .children
        .iter()
        .filter_map(|n| match n {
            Node::Element(e) if e.local() == "p" => Some(text_of(e)),
            Node::Element(e) if e.local() == "tbl" => {
                let mut rows = vec![];
                elements(e, "tr", &mut rows);
                Some(format!(
                    "[{}]",
                    rows.iter()
                        .map(|r| text_of(r))
                        .collect::<Vec<_>>()
                        .join("|")
                ))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn docx_paragraph_loops_repeat_blocks_between_the_markers() {
    let out = render_with(Kind::Docx, PARAGRAPHS_DOCX, &mut Nest::sample())
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        body_blocks(&out),
        [
            "請求書 INV-7",
            // 印だけの段落は出力しない。1 つ目の明細: 段落・付属品の表（2 行）
            "1. ねじ（3 個）",
            "[- a|- b]",
            // 2 つ目の明細: 付属品は 0 件なので表の行がすべて消え、表ごとなくなる
            "2. 板（1 個）",
            "以上 INV-7",
        ]
    );
}

#[test]
fn docx_paragraph_loops_with_no_items_remove_the_block() {
    struct Empty;
    impl Source for Empty {
        fn value(&mut self, e: &str) -> Result<Value, String> {
            Ok(json!(e))
        }
        fn loop_len(
            &mut self,
            i: usize,
            _: Option<usize>,
            _: &[usize],
            _: &str,
        ) -> Result<usize, String> {
            assert_eq!(i, 0, "外側が 0 件なら、内側のループは聞かれない");
            Ok(0)
        }
        fn item_value(&mut self, _: usize, _: &[usize], _: &str) -> Result<Value, String> {
            unreachable!()
        }
    }
    let out = render_with(Kind::Docx, PARAGRAPHS_DOCX, &mut Empty).unwrap();
    assert_eq!(body_blocks(&out), ["請求書 請求番号", "以上 請求番号"]);
}

#[test]
fn docx_paragraph_loops_are_planned_with_their_numbers() {
    let p = plan(Kind::Docx, PARAGRAPHS_DOCX).unwrap();
    let sources: Vec<(&str, Option<usize>)> = p
        .loops
        .iter()
        .map(|l| (l.source.as_str(), l.parent))
        .collect();
    assert_eq!(sources, [("明細", None), ("付属", Some(0))]);
    assert!(p.loops[0].exprs.contains(&"品目".to_string()));
}

// ---------------------------------------------------------------- セルの中の段落のループ

const CELL_PARAGRAPHS_DOCX: &[u8] = include_bytes!("fixtures/cell-paragraphs.docx");

/// 表の各セルの、段落ごとのテキスト（行 → セル → 段落）。
fn cell_paragraphs(zip: &[u8]) -> Vec<Vec<Vec<String>>> {
    let root = part(zip, "word/document.xml");
    let mut rows = vec![];
    elements(&root, "tr", &mut rows);
    rows.iter()
        .map(|tr| {
            let mut cells = vec![];
            elements(tr, "tc", &mut cells);
            cells
                .iter()
                .map(|tc| {
                    tc.children
                        .iter()
                        .filter_map(|n| match n {
                            Node::Element(p) if p.local() == "p" => {
                                let mut ts = vec![];
                                elements(p, "t", &mut ts);
                                Some(ts.iter().map(|t| t.text()).collect::<String>())
                            }
                            _ => None,
                        })
                        .collect()
                })
                .collect()
        })
        .collect()
}

#[test]
fn docx_paragraph_loops_work_inside_a_table_cell() {
    let out = render_with(Kind::Docx, CELL_PARAGRAPHS_DOCX, &mut Nest::sample())
        .unwrap_or_else(|e| panic!("{e}"));
    let rows = cell_paragraphs(&out);
    // 行は増えず、セルの中の段落だけが繰り返される。印だけの段落は出力しない
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[1][0],
        ["1. ねじ", "- a", "- b", "2. 板"],
        "{:?}",
        rows[1]
    );
    assert_eq!(rows[1][1], ["INV-7"]);
    // セルを空にしても、セルは段落で終わる（Word の決まり）
    struct Empty;
    impl Source for Empty {
        fn value(&mut self, e: &str) -> Result<Value, String> {
            Ok(json!(e))
        }
        fn loop_len(
            &mut self,
            _: usize,
            _: Option<usize>,
            _: &[usize],
            _: &str,
        ) -> Result<usize, String> {
            Ok(0)
        }
        fn item_value(&mut self, _: usize, _: &[usize], _: &str) -> Result<Value, String> {
            unreachable!()
        }
    }
    let out = render_with(Kind::Docx, CELL_PARAGRAPHS_DOCX, &mut Empty).unwrap();
    assert_eq!(cell_paragraphs(&out)[1][0], [""]);
}

#[test]
fn docx_cell_paragraph_loops_are_planned_and_row_markers_inside_are_rejected() {
    let p = plan(Kind::Docx, CELL_PARAGRAPHS_DOCX).unwrap();
    let sources: Vec<(&str, Option<usize>)> = p
        .loops
        .iter()
        .map(|l| (l.source.as_str(), l.parent))
        .collect();
    assert_eq!(sources, [("明細", None), ("付属", Some(0))]);

    // セルの段落のループの中に、同じ段落で閉じる行のループの印を書くとエラー
    let mut pkg = Package::read(CELL_PARAGRAPHS_DOCX).unwrap();
    let xml_text = String::from_utf8(pkg.get("word/document.xml").unwrap().to_vec()).unwrap();
    pkg.set(
        "word/document.xml",
        xml_text
            .replace("- {{名}}", "{{#each 付属}}{{名}}{{/each}}")
            .into_bytes(),
    );
    let e = render_with(Kind::Docx, &pkg.write().unwrap(), &mut Nest::sample()).unwrap_err();
    assert!(e.to_string().contains("セルの段落のループの中"), "{e}");
}

#[test]
fn xlsx_rules_charts_and_row_ranges_follow_the_shifted_rows() {
    let mut pkg = Package::read(&nested_xlsx_with_other_references()).unwrap();
    let edit = |pkg: &mut Package, name: &str, f: &dyn Fn(String) -> String| {
        let text = String::from_utf8(pkg.get(name).unwrap().to_vec()).unwrap();
        pkg.set(name, f(text).into_bytes());
    };
    edit(&mut pkg, "xl/worksheets/sheet1.xml", &|t| {
        t.replace(
            "<mergeCells",
            concat!(
                // 条件付き書式・入力規則の式（このシートの行 6 = 総合計の行を指す）
                "<conditionalFormatting sqref=\"B3:B6\"><cfRule type=\"expression\" priority=\"1\"><formula>$B$6&gt;5</formula></cfRule></conditionalFormatting>",
                "<dataValidations count=\"1\"><dataValidation type=\"custom\" sqref=\"A1\"><formula1>B6&gt;0</formula1></dataValidation></dataValidations>",
                "<mergeCells"
            ),
        )
        // 行だけの範囲を使う数式
        .replace("<f>C5+1</f>", "<f>SUM(3:5)+C5+1</f>")
    });
    // 別シート: 条件付き書式の式と、行だけの範囲
    edit(&mut pkg, "xl/worksheets/sheet2.xml", &|t| {
        t.replace(
            "</sheetData>",
            "</sheetData><conditionalFormatting sqref=\"A1\"><cfRule type=\"expression\" priority=\"1\"><formula>明細!$B$6&gt;1</formula></cfRule></conditionalFormatting>",
        )
        .replace("<row r=\"3\">", "<row r=\"2\"><c r=\"A2\"><f>SUM(明細!3:5)</f></c></row><row r=\"3\">")
    });
    // グラフの系列
    pkg.set(
        "xl/charts/chart1.xml",
        concat!(
            "<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"><c:chart><c:plotArea><c:barChart><c:ser>",
            "<c:cat><c:strRef><c:f>明細!$A$3:$A$5</c:f></c:strRef></c:cat>",
            "<c:val><c:numRef><c:f>'明細'!$B$6</c:f></c:numRef></c:val>",
            "</c:ser></c:barChart></c:plotArea></c:chart></c:chartSpace>"
        )
        .as_bytes()
        .to_vec(),
    );
    // 図形・グラフの置き場所（行は 0 から数える）
    pkg.set(
        "xl/worksheets/_rels/sheet1.xml.rels",
        concat!(
            "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
            "<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing\" Target=\"../drawings/drawing1.xml\"/>",
            "</Relationships>"
        )
        .as_bytes()
        .to_vec(),
    );
    pkg.set(
        "xl/drawings/drawing1.xml",
        concat!(
            "<xdr:wsDr xmlns:xdr=\"http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing\">",
            // 総合計の行（6 行目 = 0 から数えて 5）の下の図: 最初のコピーの位置へ
            "<xdr:oneCellAnchor><xdr:from><xdr:col>0</xdr:col><xdr:row>5</xdr:row></xdr:from></xdr:oneCellAnchor>",
            // ループの範囲（3〜5 行目）に掛かる図: 終わりは最後のコピーまで伸びる
            "<xdr:twoCellAnchor><xdr:from><xdr:col>0</xdr:col><xdr:row>2</xdr:row></xdr:from><xdr:to><xdr:col>2</xdr:col><xdr:row>4</xdr:row></xdr:to></xdr:twoCellAnchor>",
            // ループより前（1 行目）は動かない
            "<xdr:oneCellAnchor><xdr:from><xdr:col>0</xdr:col><xdr:row>0</xdr:row></xdr:from></xdr:oneCellAnchor>",
            // 絶対位置（EMU。行の高さは既定の 15pt = 190500）の図。1 つ目: 3〜5 行目に掛かる（y は 3 行目の上端）
            "<xdr:absoluteAnchor><xdr:pos x=\"0\" y=\"381000\"/><xdr:ext cx=\"100\" cy=\"571500\"/></xdr:absoluteAnchor>",
            // 2 つ目: 総合計の行（6 行目）の中ほどから 1 行分
            "<xdr:absoluteAnchor><xdr:pos x=\"0\" y=\"965200\"/><xdr:ext cx=\"100\" cy=\"190500\"/></xdr:absoluteAnchor>",
            // 3 つ目: ループより前（1 行目）は動かない
            "<xdr:absoluteAnchor><xdr:pos x=\"0\" y=\"0\"/><xdr:ext cx=\"100\" cy=\"190500\"/></xdr:absoluteAnchor>",
            "</xdr:wsDr>"
        )
        .as_bytes()
        .to_vec(),
    );
    let out = render_with(Kind::Xlsx, &pkg.write().unwrap(), &mut Nest::sample())
        .unwrap_or_else(|e| panic!("{e}"));

    let texts = |name: &str, local: &str| -> Vec<String> {
        let root = part(&out, name);
        let mut els = vec![];
        elements(&root, local, &mut els);
        els.iter().map(|e| e.text()).collect()
    };
    // このシート: 範囲は最後のコピーまで、1 つの参照は最初のコピー
    assert_eq!(texts("xl/worksheets/sheet1.xml", "formula"), ["$B$10>5"]);
    assert_eq!(texts("xl/worksheets/sheet1.xml", "formula1"), ["B10>0"]);
    let f = formulas_of(&out, "xl/worksheets/sheet1.xml");
    assert_eq!(f["C10"], "SUM(3:9)+C6+1"); // 行だけの範囲も最後のコピーまで
                                           // 別シート
    assert_eq!(
        texts("xl/worksheets/sheet2.xml", "formula"),
        ["明細!$B$10>1"]
    );
    assert_eq!(
        formulas_of(&out, "xl/worksheets/sheet2.xml")["A2"],
        "SUM(明細!3:9)"
    );
    // 図形・グラフの置き場所（行は 0 から数える。2 つ目の twoCellAnchor の終わりは最後のコピーまで）
    assert_eq!(
        texts("xl/drawings/drawing1.xml", "row"),
        ["9", "2", "8", "0"]
    );
    let drawing = part(&out, "xl/drawings/drawing1.xml");
    let (mut pos, mut ext) = (vec![], vec![]);
    elements(&drawing, "pos", &mut pos);
    elements(&drawing, "ext", &mut ext);
    let y: Vec<&str> = pos.iter().map(|p| p.attr("y").unwrap()).collect();
    let cy: Vec<&str> = ext.iter().map(|x| x.attr("cy").unwrap()).collect();
    // 1 つ目は 3〜9 行目（7 行分 = 1333500）に伸びる。2 つ目は最初のコピー（10 行目）の同じ位置へ。3 つ目は動かない
    assert_eq!(y, ["381000", "1727200", "0"]);
    assert_eq!(cy, ["1333500", "190500", "190500"]);
    // グラフ
    assert_eq!(
        texts("xl/charts/chart1.xml", "f"),
        ["明細!$A$3:$A$9", "'明細'!$B$10"]
    );
}

#[test]
fn xlsx_chart_caches_are_rebuilt_from_the_filled_cells() {
    let mut pkg = Package::read(&nested_xlsx_with_other_references()).unwrap();
    pkg.set(
        "xl/charts/chart1.xml",
        concat!(
            "<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"><c:chart><c:plotArea><c:barChart><c:ser>",
            "<c:cat><c:strRef><c:f>明細!$A$3:$A$5</c:f><c:strCache><c:ptCount val=\"3\"/><c:pt idx=\"0\"><c:v>古い</c:v></c:pt></c:strCache></c:strRef></c:cat>",
            "<c:val><c:numRef><c:f>明細!$B$3:$B$5</c:f><c:numCache><c:formatCode>General</c:formatCode><c:ptCount val=\"3\"/><c:pt idx=\"0\"><c:v>999</c:v></c:pt></c:numCache></c:numRef></c:val>",
            // 2 次元の範囲は触らない
            "<c:val><c:numRef><c:f>明細!$A$3:$B$5</c:f><c:numCache><c:ptCount val=\"1\"/><c:pt idx=\"0\"><c:v>7</c:v></c:pt></c:numCache></c:numRef></c:val>",
            "</c:ser></c:barChart></c:plotArea></c:chart></c:chartSpace>"
        )
        .as_bytes()
        .to_vec(),
    );
    let out = render_with(Kind::Xlsx, &pkg.write().unwrap(), &mut Nest::sample())
        .unwrap_or_else(|e| panic!("{e}"));
    let root = part(&out, "xl/charts/chart1.xml");
    let mut caches = vec![];
    for local in ["strCache", "numCache"] {
        elements(&root, local, &mut caches);
    }
    // 各キャッシュの (ptCount, [(idx, 値)])
    let points = |c: &Element| -> (String, Vec<(String, String)>) {
        let mut count = vec![];
        elements(c, "ptCount", &mut count);
        let mut pts = vec![];
        elements(c, "pt", &mut pts);
        (
            count[0].attr("val").unwrap().to_string(),
            pts.iter()
                .map(|p| {
                    let mut v = vec![];
                    elements(p, "v", &mut v);
                    (p.attr("idx").unwrap().to_string(), v[0].text())
                })
                .collect(),
        )
    };
    let texts_of = |e: &Element, local: &str| -> Vec<String> {
        let mut els = vec![];
        elements(e, local, &mut els);
        els.iter().map(|x| x.text()).collect()
    };
    let owned = |v: &[(&str, &str)]| -> Vec<(String, String)> {
        v.iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    };
    // 見出し・小計の行も含めた 3〜9 行目の文字列（空のセルは点なし）
    assert_eq!(
        points(caches[0]),
        (
            "7".to_string(),
            owned(&[
                ("0", "ねじ"),
                ("1", "a"),
                ("2", "b"),
                ("3", "小計"),
                ("4", "板"),
                ("6", "小計")
            ])
        )
    );
    // 数値は数値のセルだけ。formatCode は残る
    assert_eq!(
        points(caches[1]),
        ("7".to_string(), owned(&[("0", "3"), ("4", "1")]))
    );
    assert!(texts_of(caches[1], "formatCode") == ["General"]);
    // 2 次元の範囲のキャッシュは触らない
    assert_eq!(points(caches[2]), ("1".to_string(), owned(&[("0", "7")])));
}

#[test]
fn xlsx_three_d_references_are_shifted_or_rejected() {
    let template = nested_xlsx_with_other_references();
    let with_formula = |f: &str| {
        let mut pkg = Package::read(&template).unwrap();
        let text =
            String::from_utf8(pkg.get("xl/worksheets/sheet2.xml").unwrap().to_vec()).unwrap();
        pkg.set(
            "xl/worksheets/sheet2.xml",
            text.replace(
                "</sheetData>",
                &format!("<row r=\"9\"><c r=\"A9\"><f>{f}</f></c></row></sheetData>"),
            )
            .into_bytes(),
        );
        render_with(Kind::Xlsx, &pkg.write().unwrap(), &mut Nest::sample())
    };
    // 範囲（明細〜Sheet2）の中で、動いたのは明細だけ。ループより前の行（1 行目）はどのシートでも同じなので、そのまま
    let out = with_formula("SUM(明細:Sheet2!B1)").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        formulas_of(&out, "xl/worksheets/sheet2.xml")["A9"],
        "SUM(明細:Sheet2!B1)"
    );
    // ずれる行を指すと、シートごとに結果が違うのでエラー（黙って壊さない）
    let e = with_formula("SUM(明細:Sheet2!B6)").unwrap_err();
    assert!(e.to_string().contains("3D 参照"), "{e}");
    // 範囲が動いたシートだけ（明細〜明細）なら、普通の参照と同じにずらす
    let out = with_formula("SUM(明細:明細!B6)").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        formulas_of(&out, "xl/worksheets/sheet2.xml")["A9"],
        "SUM(明細:明細!B10)"
    );
}

/// `x14` の拡張領域（条件付き書式・入力規則・スパークライン）を持つ `<extLst>`。`f` はその中の式。
fn x14_ext(f: &str, sqref: &str) -> String {
    format!(
        concat!(
            "<extLst><ext uri=\"{{78C0D931-6437-407d-A8EE-F0AAD7539E65}}\" ",
            "xmlns:x14=\"http://schemas.microsoft.com/office/spreadsheetml/2009/9/main\" ",
            "xmlns:xm=\"http://schemas.microsoft.com/office/excel/2006/main\">",
            "<x14:conditionalFormattings><x14:conditionalFormatting>",
            "<x14:cfRule type=\"expression\" priority=\"2\"><xm:f>{f}</xm:f></x14:cfRule>",
            "<xm:sqref>{sqref}</xm:sqref></x14:conditionalFormatting></x14:conditionalFormattings>",
            "<x14:sparklineGroups><x14:sparklineGroup><x14:sparklines><x14:sparkline>",
            "<xm:f>{f}</xm:f><xm:sqref>A6</xm:sqref></x14:sparkline></x14:sparklines></x14:sparklineGroup></x14:sparklineGroups>",
            "</ext></extLst>"
        ),
        f = f,
        sqref = sqref
    )
}

#[test]
fn xlsx_extension_formulas_and_ranges_follow_the_shifted_rows() {
    let mut pkg = Package::read(&nested_xlsx_with_other_references()).unwrap();
    let edit = |pkg: &mut Package, name: &str, f: &dyn Fn(String) -> String| {
        let text = String::from_utf8(pkg.get(name).unwrap().to_vec()).unwrap();
        pkg.set(name, f(text).into_bytes());
    };
    // このシート: 自分を指す式（名前なしと、名前つき）と、範囲
    edit(&mut pkg, "xl/worksheets/sheet1.xml", &|t| {
        t.replace(
            "</worksheet>",
            &format!("{}</worksheet>", x14_ext("$B$6&gt;5+明細!$B$6", "B3:B6")),
        )
    });
    // 別のシート: 明細を指す式
    edit(&mut pkg, "xl/worksheets/sheet2.xml", &|t| {
        t.replace(
            "</worksheet>",
            &format!("{}</worksheet>", x14_ext("明細!$B$6&gt;1", "A1")),
        )
    });
    let out = render_with(Kind::Xlsx, &pkg.write().unwrap(), &mut Nest::sample())
        .unwrap_or_else(|e| panic!("{e}"));

    fn xm(el: &Element, local: &str, out: &mut Vec<String>) {
        for c in &el.children {
            if let Node::Element(e) = c {
                if e.name == format!("xm:{local}") {
                    out.push(e.text());
                }
                xm(e, local, out);
            }
        }
    }
    let get = |name: &str, local: &str| {
        let mut v = vec![];
        xm(&part(&out, name), local, &mut v);
        v
    };
    // このシート（総合計の行 6 → 10。範囲は最後のコピーまで）。名前つきの自分への参照も同じ
    assert_eq!(
        get("xl/worksheets/sheet1.xml", "f"),
        ["$B$10>5+明細!$B$10", "$B$10>5+明細!$B$10"]
    );
    assert_eq!(get("xl/worksheets/sheet1.xml", "sqref"), ["B3:B10", "A10"]);
    // 別のシートの拡張領域は、ほかのシートへの参照だけがずれる（範囲はそのシート自身のもの）
    assert_eq!(
        get("xl/worksheets/sheet2.xml", "f"),
        ["明細!$B$10>1", "明細!$B$10>1"]
    );
    assert_eq!(get("xl/worksheets/sheet2.xml", "sqref"), ["A1", "A6"]);
}

#[test]
fn xlsx_pivot_tables_follow_the_shifted_rows_and_refresh_on_load() {
    let mut pkg = Package::read(&nested_xlsx_with_other_references()).unwrap();
    // 明細シートに置かれたピボットテーブル（総合計の行 6 の下 = 7 行目から）。元データはループを含む範囲
    pkg.set(
        "xl/worksheets/_rels/sheet1.xml.rels",
        concat!(
            "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
            "<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/pivotTable\" Target=\"../pivotTables/pivotTable1.xml\"/>",
            "</Relationships>"
        )
        .as_bytes()
        .to_vec(),
    );
    pkg.set(
        "xl/pivotTables/pivotTable1.xml",
        br#"<pivotTableDefinition xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" name="P" cacheId="1"><location ref="F7:H9" firstHeaderRow="1" firstDataRow="1" firstDataCol="1"/></pivotTableDefinition>"#.to_vec(),
    );
    let cache = |sheet: &str| {
        format!(
            r#"<pivotCacheDefinition xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><cacheSource type="worksheet"><worksheetSource ref="A2:C5" sheet="{sheet}"/></cacheSource></pivotCacheDefinition>"#
        )
        .into_bytes()
    };
    pkg.set("xl/pivotCache/pivotCacheDefinition1.xml", cache("明細"));
    pkg.set("xl/pivotCache/pivotCacheDefinition2.xml", cache("Sheet2"));
    let out = render_with(Kind::Xlsx, &pkg.write().unwrap(), &mut Nest::sample())
        .unwrap_or_else(|e| panic!("{e}"));

    // 置き場所は下へずれる（6 行目の総合計が 10 行目になるので、7〜9 行目は 11〜13 行目）
    let loc = part(&out, "xl/pivotTables/pivotTable1.xml");
    let mut l = vec![];
    elements(&loc, "location", &mut l);
    assert_eq!(l[0].attr("ref"), Some("F11:H13"));
    // 元データの範囲はループを含むので最後のコピーまで広がり、開くときに更新させる
    let c1 = part(&out, "xl/pivotCache/pivotCacheDefinition1.xml");
    let mut w = vec![];
    elements(&c1, "worksheetSource", &mut w);
    assert_eq!(w[0].attr("ref"), Some("A2:C9"));
    assert_eq!(c1.attr("refreshOnLoad"), Some("1"));
    // 動いていないシートが元データのキャッシュは触らない
    let c2 = part(&out, "xl/pivotCache/pivotCacheDefinition2.xml");
    let mut w = vec![];
    elements(&c2, "worksheetSource", &mut w);
    assert_eq!(w[0].attr("ref"), Some("A2:C5"));
    assert_eq!(c2.attr("refreshOnLoad"), None);
}

#[test]
fn xlsx_pivot_caches_refresh_when_a_sheet_without_loops_is_filled_in() {
    let mut pkg = Package::read(&nested_xlsx_with_other_references()).unwrap();
    // Sheet2（ループなし）に差し込みの欄を足す。これを元データにするキャッシュと、表名が元データのキャッシュ
    let text = String::from_utf8(pkg.get("xl/worksheets/sheet2.xml").unwrap().to_vec()).unwrap();
    pkg.set(
        "xl/worksheets/sheet2.xml",
        text.replace(
            "<row r=\"3\">",
            "<row r=\"2\"><c r=\"A2\" t=\"inlineStr\"><is><t>{{請求番号}}</t></is></c></row><row r=\"3\">",
        )
        .into_bytes(),
    );
    let cache = |source: &str| {
        format!(
            r#"<pivotCacheDefinition xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><cacheSource type="worksheet">{source}</cacheSource></pivotCacheDefinition>"#
        )
        .into_bytes()
    };
    pkg.set(
        "xl/pivotCache/pivotCacheDefinition1.xml",
        cache(r#"<worksheetSource ref="A1:A9" sheet="Sheet2"/>"#),
    );
    pkg.set(
        "xl/pivotCache/pivotCacheDefinition2.xml",
        cache(r#"<worksheetSource name="売上表"/>"#),
    );
    // 差し込みのないシートが元データ
    pkg.set(
        "xl/pivotCache/pivotCacheDefinition3.xml",
        cache(r#"<worksheetSource ref="A1:A9" sheet="Sheet9"/>"#),
    );
    let out = render_with(Kind::Xlsx, &pkg.write().unwrap(), &mut Nest::sample())
        .unwrap_or_else(|e| panic!("{e}"));
    let refresh = |n: u32| {
        part(&out, &format!("xl/pivotCache/pivotCacheDefinition{n}.xml"))
            .attr("refreshOnLoad")
            .map(String::from)
    };
    // 範囲は動かさず（Sheet2 は行がずれない）、開くときに更新させる
    let c1 = part(&out, "xl/pivotCache/pivotCacheDefinition1.xml");
    let mut w = vec![];
    elements(&c1, "worksheetSource", &mut w);
    assert_eq!(w[0].attr("ref"), Some("A1:A9"));
    assert_eq!(refresh(1).as_deref(), Some("1"));
    assert_eq!(refresh(2).as_deref(), Some("1")); // 表名が元データ（どこかを書き換えている）
    assert_eq!(refresh(3), None); // 書き換えていないシートが元データ
}
