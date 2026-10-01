//! Tauri のコマンド層。ロジックはすべて `jxcel-app` の `Session` にあり、ここは薄い橋渡しだけ。

use jxcel_app::{RunOutput, Session, Snapshot};
use jxcel_core::diff::Change;
use jxcel_core::Column;
use jxcel_git::CommitInfo;
use serde_json::Value;
use std::sync::Mutex;
use tauri::State;

type App<'a> = State<'a, Mutex<Session>>;
type Reply<T> = Result<T, String>;

/// セッションをロックして操作を実行し、エラーを UI 向けの文字列にする。
fn with<T>(app: &App, f: impl FnOnce(&mut Session) -> jxcel_app::Result<T>) -> Reply<T> {
    let mut session = app
        .lock()
        .map_err(|_| "内部状態が壊れています".to_string())?;
    f(&mut session).map_err(|e| e.to_string())
}

// ファイル入出力や git を含むので、メインスレッドを塞がないよう async(スレッドプール)で実行する。

#[tauri::command(async)]
fn new_file(app: App, name: String) -> Reply<Snapshot> {
    with(&app, |s| s.new_file(&name))
}

#[tauri::command(async)]
fn open_file(app: App, path: String) -> Reply<Snapshot> {
    with(&app, |s| s.open(&path))
}

#[tauri::command(async)]
fn save_file(app: App, path: Option<String>, message: String) -> Reply<Snapshot> {
    with(&app, |s| s.save(path.as_deref(), &message))
}

#[tauri::command(async)]
fn add_sheet(app: App, name: String) -> Reply<Snapshot> {
    with(&app, |s| s.add_sheet(&name))
}

#[tauri::command(async)]
fn rename_sheet(app: App, sheet: String, name: String) -> Reply<Snapshot> {
    with(&app, |s| s.rename_sheet(&sheet, &name))
}

#[tauri::command(async)]
fn delete_sheet(app: App, sheet: String) -> Reply<Snapshot> {
    with(&app, |s| s.delete_sheet(&sheet))
}

#[tauri::command(async)]
fn add_schema(app: App, sheet: String, name: String, columns: Vec<Column>) -> Reply<Snapshot> {
    with(&app, |s| s.add_schema(&sheet, &name, columns))
}

#[tauri::command(async)]
fn add_column(app: App, sheet: String, schema: String, column: Column) -> Reply<Snapshot> {
    with(&app, |s| s.add_column(&sheet, &schema, column))
}

#[tauri::command(async)]
fn update_column(app: App, sheet: String, schema: String, column: Column) -> Reply<Snapshot> {
    with(&app, |s| s.update_column(&sheet, &schema, column))
}

#[tauri::command(async)]
fn delete_column(app: App, sheet: String, schema: String, column: String) -> Reply<Snapshot> {
    with(&app, |s| s.delete_column(&sheet, &schema, &column))
}

#[tauri::command(async)]
fn add_row(app: App, sheet: String, schema: String) -> Reply<Snapshot> {
    with(&app, |s| s.add_row(&sheet, &schema))
}

#[tauri::command(async)]
fn delete_row(app: App, sheet: String, schema: String, row: String) -> Reply<Snapshot> {
    with(&app, |s| s.delete_row(&sheet, &schema, &row))
}

#[tauri::command(async)]
fn set_cell(
    app: App,
    sheet: String,
    schema: String,
    row: String,
    column: String,
    value: Value,
) -> Reply<Snapshot> {
    with(&app, |s| s.set_cell(&sheet, &schema, &row, &column, value))
}

#[tauri::command(async)]
fn add_macro(app: App, name: String, source: Option<String>) -> Reply<Snapshot> {
    with(&app, |s| s.add_macro(&name, source.as_deref()))
}

#[tauri::command]
fn macro_samples() -> Vec<jxcel_macro::samples::Sample> {
    Session::macro_samples()
}

#[tauri::command(async)]
fn update_macro(app: App, id: String, name: String, source: String) -> Reply<Snapshot> {
    with(&app, |s| s.update_macro(&id, &name, &source))
}

#[tauri::command(async)]
fn delete_macro(app: App, id: String) -> Reply<Snapshot> {
    with(&app, |s| s.delete_macro(&id))
}

// マクロは最大 10 秒かかりうるので async(スレッドプール)で実行する。
#[tauri::command(async)]
fn run_macro(app: App, id: String, source: Option<String>) -> Reply<RunOutput> {
    with(&app, |s| s.run_macro(&id, source.as_deref()))
}

#[tauri::command(async)]
fn history_log(app: App) -> Reply<Vec<CommitInfo>> {
    with(&app, |s| s.history_log())
}

#[tauri::command(async)]
fn history_diff(app: App, from: String, to: String) -> Reply<Vec<Change>> {
    with(&app, |s| s.history_diff(&from, &to))
}

#[tauri::command(async)]
fn restore(app: App, rev: String) -> Reply<Snapshot> {
    with(&app, |s| s.restore(&rev))
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Mutex::new(Session::new()))
        .invoke_handler(tauri::generate_handler![
            new_file,
            open_file,
            save_file,
            add_sheet,
            rename_sheet,
            delete_sheet,
            add_schema,
            add_column,
            update_column,
            delete_column,
            add_row,
            delete_row,
            set_cell,
            add_macro,
            macro_samples,
            update_macro,
            delete_macro,
            run_macro,
            history_log,
            history_diff,
            restore,
        ])
        .run(tauri::generate_context!())
        .expect("jxcel の起動に失敗しました");
}
