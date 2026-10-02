//! フォーム配信を、アプリを起動せずに試すための例。サンプルの表とフォームを作って配信し、
//! 回答が届くたびに、表の全行を JSON で標準出力に出す。Enter で終了する。
//!
//! ```text
//! cargo run -p jxcel-app --example serve_forms
//! ```

use jxcel_app::forms::FormServer;
use jxcel_app::Session;
use jxcel_core::{Column, DataType};
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
    s.add_form(&sheet, &schema, "来客受付").unwrap();

    let shared = Arc::new(Mutex::new(s));
    let for_print = shared.clone();
    let port = std::env::args()
        .nth(1)
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    let server = FormServer::start(shared, port, move || {
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
