//! フォーム配信を、アプリを起動せずに試すための例。サンプルの表とフォームを作って配信し、
//! 回答が届くたびに、表の全行を JSON で標準出力に出す。Enter で終了する。
//!
//! ```text
//! cargo run -p jxcel-app --example serve_forms [ポート [合言葉 [no-edit | 修正できる分数 [回答者ごとの合言葉（カンマ区切り）]]]]
//! ```

use jxcel_app::forms::{FormServer, ServeOptions};
use jxcel_app::Session;
use jxcel_core::{Column, DataType, FormEdit};
use std::sync::{Arc, Mutex};

fn main() {
    let mut s = Session::new();
    let snap = s.new_file("例").unwrap();
    let sheet = snap.file.sheets[0].id.clone();
    let snap = s
        .add_schema(
            &sheet,
            "来客",
            vec![
                Column::new("name", "氏名", DataType::String).required(),
                Column::new("people", "人数", DataType::Int),
                Column::new("price", "予算", DataType::Decimal),
                Column::new(
                    "kind",
                    "区分",
                    DataType::Enum {
                        values: vec!["個人".into(), "法人".into()],
                    },
                ),
                Column::new("day", "来訪日", DataType::Date),
                Column::new("at", "来訪時刻", DataType::DateTime),
                Column::new("memo", "了承済み", DataType::Bool),
            ],
        )
        .unwrap();
    let schema = snap.file.sheets[0].schemas.last().unwrap().id.clone();
    let form = s.add_form(&sheet, &schema, "来客受付").unwrap().file.forms[0]
        .id
        .clone();
    // 3 番目の引数が修正の設定（`no-edit` なら修正を受け付けない、数字ならその分数まで。省略は期限なし）
    match std::env::args().nth(3).as_deref() {
        Some("no-edit") => {
            s.set_form_edit(
                &form,
                FormEdit {
                    allowed: false,
                    minutes: None,
                },
            )
            .unwrap();
        }
        Some(m) if m.parse::<u32>().is_ok() => {
            s.set_form_edit(
                &form,
                FormEdit {
                    allowed: true,
                    minutes: m.parse().ok(),
                },
            )
            .unwrap();
        }
        _ => {}
    }

    let shared = Arc::new(Mutex::new(s));
    let for_print = shared.clone();
    let port = std::env::args()
        .nth(1)
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    // 2 番目の引数が合言葉（省略すると合言葉なし）
    let code = std::env::args().nth(2);
    // 4 番目の引数が、回答者ごとの合言葉（カンマ区切り）
    let respondent_codes = std::env::args()
        .nth(4)
        .map(|c| c.split(',').map(String::from).collect())
        .unwrap_or_default();
    let options = ServeOptions {
        access_code: code,
        respondent_codes,
    };
    let server = FormServer::start_with(shared, port, options, move || {
        let snap = for_print.lock().unwrap().current().unwrap();
        let rows: Vec<_> = snap.file.sheets[0]
            .schemas
            .last()
            .unwrap()
            .rows
            .iter()
            .map(|r| &r.cells)
            .collect();
        println!("ROWS {}", serde_json::to_string(&rows).unwrap());
    })
    .unwrap();
    for u in server.status().urls {
        println!(
            "URL {}",
            u.url
                .replacen(&u.url[7..u.url.rfind(':').unwrap()], "127.0.0.1", 1)
        );
    }
    println!("READY");
    let _ = std::io::stdin().read_line(&mut String::new());
}
