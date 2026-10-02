//! 表の全行からファイルを生成する処理のテスト（実際の docx / xlsx テンプレートを使う）。

use jxcel_core::{Column, DataSchema, DataType, JxcelFile, Row, Sheet};
use jxcel_export::package::Package;
use jxcel_export::xml::{self, Element, Node};
use jxcel_export::{export, preview, sanitize_filename, Kind, Spec};
use jxcel_macro::Options;
use serde_json::{json, Value};

const DOCX: &[u8] = include_bytes!("fixtures/invoice.docx");
const XLSX: &[u8] = include_bytes!("fixtures/invoice.xlsx");

/// 請求番号・取引先・数量・単価・区分・備考
type InvoiceRow<'a> = (&'a str, &'a str, Option<i64>, i64, &'a str, Option<&'a str>);

/// 請求の表: 取引先・品名・数量・単価・区分・備考・請求番号・発行日・完了（テンプレートの欄に合わせた列名）
fn invoices(rows: &[InvoiceRow]) -> JxcelFile {
    let cols = [
        "請求番号",
        "取引先",
        "数量",
        "単価",
        "区分",
        "備考",
        "品名",
        "発行日",
        "完了",
    ];
    let types = [
        DataType::String,
        DataType::String,
        DataType::Int,
        DataType::Int,
        DataType::String,
        DataType::String,
        DataType::String,
        DataType::Date,
        DataType::Bool,
    ];
    let mut schema = DataSchema::new(
        "請求",
        cols.iter()
            .zip(types)
            .map(|(c, t)| Column::new(format!("c_{c}"), *c, t))
            .collect(),
    );
    schema.id = "inv".into();
    for (i, (no, client, qty, price, kind, note)) in rows.iter().enumerate() {
        let mut cells = std::collections::BTreeMap::new();
        let mut put = |k: &str, v: Value| {
            cells.insert(format!("c_{k}"), v);
        };
        put("請求番号", json!(no));
        put("取引先", json!(client));
        if let Some(q) = qty {
            put("数量", json!(q));
        }
        put("単価", json!(price));
        put("区分", json!(kind));
        if let Some(n) = note {
            put("備考", json!(n));
        }
        put("品名", json!("ねじ"));
        put("発行日", json!("2024-01-31"));
        put("完了", json!(i % 2 == 0));
        schema.rows.push(Row {
            id: format!("row{i}"),
            cells,
        });
    }
    JxcelFile {
        name: "請求".into(),
        sheets: vec![Sheet {
            id: "sh".into(),
            name: "請求".into(),
            schemas: vec![schema],
        }],
        macros: Default::default(),
        exports: Default::default(),
        forms: Default::default(),
        templates: Default::default(),
    }
}

fn spec<'a>(
    kind: Kind,
    template: &'a [u8],
    filename: &'a str,
    filter: Option<&'a str>,
) -> Spec<'a> {
    Spec {
        kind,
        template,
        sheet: "sh",
        schema: "inv",
        filename,
        filter,
    }
}

fn body_text(zip: &[u8]) -> String {
    let pkg = Package::read(zip).unwrap();
    let root = xml::parse(pkg.get("word/document.xml").unwrap())
        .unwrap()
        .root;
    fn walk(e: &Element, out: &mut String) {
        for c in &e.children {
            if let Node::Element(x) = c {
                if x.name == "w:t" {
                    out.push_str(&x.text());
                } else {
                    walk(x, out);
                }
                if x.name == "w:p" {
                    out.push('\n');
                }
            }
        }
    }
    let mut s = String::new();
    walk(&root, &mut s);
    s
}

fn three() -> JxcelFile {
    invoices(&[
        ("INV-001", "株式会社A", Some(3), 1200, "A", Some("急ぎ")),
        ("INV-002", "B商事", Some(10), 500, "B", Some("通常")),
        ("INV-003", "有限会社C", Some(1), 9800, "A", Some("")),
    ])
}

#[test]
fn one_file_per_row_with_row_specific_content_and_names() {
    let r = export(
        &three(),
        &spec(Kind::Docx, DOCX, "{{請求番号}}_{{取引先}}", None),
        &Options::default(),
    )
    .unwrap();
    assert!(r.errors.is_empty() && r.skipped == 0, "{:?}", r.errors);
    let names: Vec<&str> = r.files.iter().map(|f| f.filename.as_str()).collect();
    assert_eq!(
        names,
        [
            "INV-001_株式会社A.docx",
            "INV-002_B商事.docx",
            "INV-003_有限会社C.docx"
        ]
    );
    assert_eq!(
        r.files.iter().map(|f| f.row_no).collect::<Vec<_>>(),
        [1, 2, 3]
    );

    // 行ごとに中身が違う（宛先・合計・判定・ヘッダー）
    let t0 = body_text(&r.files[0].bytes);
    let t1 = body_text(&r.files[1].bytes);
    assert!(
        t0.contains("宛先: 株式会社A 御中") && t0.contains("合計 3600 円") && t0.contains("判定 ○"),
        "{t0}"
    );
    assert!(
        t1.contains("宛先: B商事 御中") && t1.contains("合計 5000 円") && t1.contains("判定 ×"),
        "{t1}"
    );
    let pkg = Package::read(&r.files[2].bytes).unwrap();
    assert!(String::from_utf8_lossy(pkg.get("word/header1.xml").unwrap()).contains("INV-003"));
    // 3 行目の備考は空文字なので「備考: 」だけ
    assert!(
        body_text(&r.files[2].bytes).contains("備考: \n"),
        "{}",
        body_text(&r.files[2].bytes)
    );
}

#[test]
fn filter_selects_rows_and_reports_skipped() {
    let r = export(
        &three(),
        &spec(Kind::Docx, DOCX, "{{請求番号}}", Some("区分 === 'A'")),
        &Options::default(),
    )
    .unwrap();
    assert_eq!(
        r.files
            .iter()
            .map(|f| f.filename.as_str())
            .collect::<Vec<_>>(),
        ["INV-001.docx", "INV-003.docx"]
    );
    assert_eq!(r.skipped, 1);
    // 行番号は元の表での位置（絞り込んでも変わらない）
    assert_eq!(r.files.iter().map(|f| f.row_no).collect::<Vec<_>>(), [1, 3]);
    // 全部除外
    let none = export(
        &three(),
        &spec(Kind::Docx, DOCX, "x", Some("false")),
        &Options::default(),
    )
    .unwrap();
    assert!(none.files.is_empty() && none.skipped == 3);
}

#[test]
fn a_failing_row_is_reported_and_other_rows_are_still_written() {
    // 2 行目は数量が空 → 「数量 * 単価」は 0 だが、備考が空(null) の行は備考.length で例外になる式を使う
    let f = invoices(&[
        ("INV-001", "A", Some(3), 100, "A", Some("メモ")),
        ("INV-002", "B", Some(2), 100, "A", None),
        ("INV-003", "C", Some(1), 100, "A", Some("メモ")),
    ]);
    let tpl = DOCX;
    // 絞り込みに例外を起こす式を使うと、その行だけ失敗する
    let r = export(
        &f,
        &spec(Kind::Docx, tpl, "{{請求番号}}", Some("備考.length > 0")),
        &Options::default(),
    )
    .unwrap();
    assert_eq!(r.files.len(), 2);
    assert_eq!(r.errors.len(), 1);
    assert_eq!(
        (r.errors[0].row_no, r.errors[0].row_id.as_str()),
        (2, "row1")
    );
    assert!(
        r.errors[0].message.contains("絞り込み条件"),
        "{}",
        r.errors[0].message
    );

    // ファイル名の式が失敗した場合
    let r = export(
        &f,
        &spec(Kind::Docx, tpl, "{{備考.length}}-{{請求番号}}", None),
        &Options::default(),
    )
    .unwrap();
    assert_eq!(r.errors.len(), 1);
    assert!(
        r.errors[0].message.contains("ファイル名"),
        "{}",
        r.errors[0].message
    );
    assert_eq!(r.files.len(), 2);
}

#[test]
fn duplicate_and_unsafe_names_are_made_unique_and_safe() {
    let f = invoices(&[
        ("A/B:C", "同じ", Some(1), 1, "A", Some("")),
        ("A/B:C", "同じ", Some(1), 1, "A", Some("")),
        ("a/b:c", "同じ", Some(1), 1, "A", Some("")),
        ("", "", Some(1), 1, "A", Some("")),
    ]);
    let r = export(
        &f,
        &spec(Kind::Docx, DOCX, "{{請求番号}}", None),
        &Options::default(),
    )
    .unwrap();
    let names: Vec<&str> = r.files.iter().map(|f| f.filename.as_str()).collect();
    // 使えない文字は _ に。同名（大文字小文字違いも）は番号を付ける。空は行番号
    assert_eq!(
        names,
        [
            "A_B_C.docx",
            "A_B_C (2).docx",
            "a_b_c (3).docx",
            "row4.docx"
        ]
    );
    // 拡張子を自分で書いても重複しない
    let r = export(
        &f,
        &spec(Kind::Docx, DOCX, "{{取引先}}.DOCX", None),
        &Options::default(),
    )
    .unwrap();
    assert_eq!(r.files[0].filename, "同じ.docx");
    assert_eq!(r.files[1].filename, "同じ (2).docx");
    // パターンが空なら行番号
    let r = export(
        &three(),
        &spec(Kind::Docx, DOCX, "", None),
        &Options::default(),
    )
    .unwrap();
    assert_eq!(
        r.files
            .iter()
            .map(|f| f.filename.as_str())
            .collect::<Vec<_>>(),
        ["1.docx", "2.docx", "3.docx"]
    );
}

#[test]
fn sanitize_handles_reserved_and_long_names() {
    assert_eq!(sanitize_filename(" a<b>c?.. "), "a_b_c_");
    assert_eq!(sanitize_filename("CON"), "_CON");
    assert_eq!(sanitize_filename("com1.txt"), "_com1.txt");
    assert_eq!(sanitize_filename("COMX"), "COMX");
    assert_eq!(sanitize_filename(&"あ".repeat(300)).chars().count(), 100);
    assert_eq!(sanitize_filename("...  "), "");
    assert_eq!(sanitize_filename("tab\there"), "tab_here");
}

#[test]
fn xlsx_rows_become_numeric_cells() {
    let r = export(
        &three(),
        &spec(Kind::Xlsx, XLSX, "{{請求番号}}", None),
        &Options::default(),
    )
    .unwrap();
    assert_eq!(r.files.len(), 3, "{:?}", r.errors);
    let sheet = |zip: &[u8]| {
        let pkg = Package::read(zip).unwrap();
        String::from_utf8(pkg.get("xl/worksheets/sheet1.xml").unwrap().to_vec()).unwrap()
    };
    // 3 行目: 数量 1 × 単価 9800
    let s = sheet(&r.files[2].bytes);
    assert!(s.contains("<v>9800</v>"), "{s}");
    assert!(sheet(&r.files[0].bytes).contains("<v>3600</v>"));
}

#[test]
fn expressions_can_use_computed_columns_and_std() {
    let mut f = three();
    f.sheets[0].schemas[0].columns.push(
        Column::new("c_金額", "金額", DataType::Int)
            .computed("export default (row: any) => row.数量 * row.単価"),
    );
    // ファイル名の式に、計算列（金額）・std・行番号（_no）を使う
    let r = export(
        &f,
        &spec(
            Kind::Docx,
            DOCX,
            "{{ 金額 }}_{{ std.text.zeroPad(_no, 3) }}",
            None,
        ),
        &Options::default(),
    )
    .unwrap();
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let names: Vec<&str> = r.files.iter().map(|f| f.filename.as_str()).collect();
    assert_eq!(names, ["3600_001.docx", "5000_002.docx", "9800_003.docx"]);
    // 絞り込みにも計算列が使える
    let r = export(
        &f,
        &spec(Kind::Docx, DOCX, "{{請求番号}}", Some("金額 >= 5000")),
        &Options::default(),
    )
    .unwrap();
    assert_eq!(r.files.len(), 2);
}

#[test]
fn preview_lists_placeholders_and_first_rows() {
    let p = preview(
        &three(),
        &spec(Kind::Docx, DOCX, "{{請求番号}}", Some("区分 === 'A'")),
        2,
        &Options::default(),
    )
    .unwrap();
    assert!(
        p.placeholders.iter().any(|x| x == "取引先")
            && p.placeholders.iter().any(|x| x == "数量 * 単価")
    );
    assert_eq!(p.total_rows, 3);
    assert_eq!(p.rows.len(), 2); // limit
    let idx = |name: &str| p.placeholders.iter().position(|x| x == name).unwrap();
    assert_eq!(p.rows[0].values[idx("取引先")], Ok(json!("株式会社A")));
    assert_eq!(p.rows[1].values[idx("数量 * 単価")], Ok(json!(5000)));
    assert_eq!(p.rows[0].filename, Ok("INV-001.docx".to_string()));
    assert!(!p.rows[0].excluded && p.rows[1].excluded); // 2 行目は区分 B なので除外
                                                        // 式の誤りはその欄・その行のエラーとして見える（プレビューでは失敗しない）
    let bad = invoices(&[("X", "A", None, 1, "A", None)]);
    let p = preview(
        &bad,
        &spec(Kind::Docx, DOCX, "{{備考.length}}", None),
        1,
        &Options::default(),
    )
    .unwrap();
    assert!(p.rows[0].filename.is_err());
}

#[test]
fn unknown_target_table_is_an_error() {
    let mut s = spec(Kind::Docx, DOCX, "x", None);
    s.schema = "nope";
    assert!(export(&three(), &s, &Options::default()).is_err());
}
