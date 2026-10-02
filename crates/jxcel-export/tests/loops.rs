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

    // 表の外に印を書いた
    let mut doc = jxcel_export::package::Package::read(DOCX).unwrap();
    let xml_text = String::from_utf8(doc.get("word/document.xml").unwrap().to_vec()).unwrap();
    doc.set(
        "word/document.xml",
        xml_text.replace("宛先: ", "宛先: {{#each x}}").into_bytes(),
    );
    let out = doc.write().unwrap();
    let e = render_with(Kind::Docx, &out, &mut Fake::new(items(1))).unwrap_err();
    assert!(e.to_string().contains("表の行の中"), "{e}");
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
