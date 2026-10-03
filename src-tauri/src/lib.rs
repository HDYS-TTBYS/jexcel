//! Tauri のコマンド層。ロジックはすべて `jxcel-app` の `Session` にあり、ここは薄い橋渡しだけ。

use jxcel_app::forms::{FormServer, FormsStatus};
use jxcel_app::{ConvertResult, ExportPreview, ExportResult, RunOutput, Session, Snapshot};
use jxcel_core::diff::Change;
use jxcel_core::Column;
use jxcel_git::CommitInfo;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tauri::{Emitter, State};

// フォームの配信スレッドも同じセッションに回答を書き込むので、共有できる形で持つ。
type Shared = Arc<Mutex<Session>>;
type App<'a> = State<'a, Shared>;
/// 配信中のフォームサーバー（止まっていれば `None`）。
type Forms<'a> = State<'a, Mutex<Option<FormServer>>>;
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
fn current_file(app: App) -> Reply<Snapshot> {
    with(&app, |s| s.current())
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
fn convert_datetime_offset(
    app: App,
    sheet: String,
    schema: String,
    column: String,
    offset: String,
) -> Reply<ConvertResult> {
    with(&app, |s| {
        s.convert_datetime_offset(&sheet, &schema, &column, &offset)
    })
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
fn add_export(app: App, template_path: String, name: Option<String>) -> Reply<Snapshot> {
    with(&app, |s| s.add_export(&template_path, name.as_deref()))
}

#[tauri::command(async)]
fn update_export(
    app: App,
    id: String,
    name: String,
    sheet: String,
    schema: String,
    filename: String,
    filter: Option<String>,
) -> Reply<Snapshot> {
    with(&app, |s| {
        s.update_export(&id, &name, &sheet, &schema, &filename, filter.as_deref())
    })
}

#[tauri::command(async)]
fn replace_export_template(app: App, id: String, template_path: String) -> Reply<Snapshot> {
    with(&app, |s| s.replace_export_template(&id, &template_path))
}

#[tauri::command(async)]
fn delete_export(app: App, id: String) -> Reply<Snapshot> {
    with(&app, |s| s.delete_export(&id))
}

#[tauri::command(async)]
fn export_preview(app: App, id: String, limit: usize) -> Reply<ExportPreview> {
    with(&app, |s| s.export_preview(&id, limit))
}

// 全行を評価してファイルを書くので、時間がかかりうる。async(スレッドプール)で実行する。
#[tauri::command(async)]
fn run_export(app: App, id: String, out_dir: String) -> Reply<ExportResult> {
    with(&app, |s| s.run_export(&id, &out_dir))
}

#[tauri::command(async)]
fn add_form(app: App, sheet: String, schema: String, name: String) -> Reply<Snapshot> {
    with(&app, |s| s.add_form(&sheet, &schema, &name))
}

#[tauri::command(async)]
fn update_form(
    app: App,
    id: String,
    name: String,
    sheet: String,
    schema: String,
    columns: Vec<String>,
    description: String,
) -> Reply<Snapshot> {
    with(&app, |s| {
        s.update_form(&id, &name, &sheet, &schema, columns, &description)
    })
}

/// 送信済みの回答の修正の設定（直せるか・送信から何分まで直せるか。`minutes` が無ければ期限なし）。
#[tauri::command(async)]
fn set_form_edit(app: App, id: String, allowed: bool, minutes: Option<u32>) -> Reply<Snapshot> {
    with(&app, |s| {
        s.set_form_edit(&id, jxcel_core::FormEdit { allowed, minutes })
    })
}

#[tauri::command(async)]
fn delete_form(app: App, id: String) -> Reply<Snapshot> {
    with(&app, |s| s.delete_form(&id))
}

fn forms_lock<'a>(forms: &'a Forms) -> Reply<std::sync::MutexGuard<'a, Option<FormServer>>> {
    forms
        .lock()
        .map_err(|_| "内部状態が壊れています".to_string())
}

fn status_of(server: &Option<FormServer>) -> FormsStatus {
    server
        .as_ref()
        .map(FormServer::status)
        .unwrap_or_else(FormsStatus::stopped)
}

/// フォームの配信を始める。回答が届くたびに UI へ `jxcel://changed` を送る。
#[tauri::command(async)]
fn forms_start(
    window: tauri::WebviewWindow,
    app: App,
    forms: Forms,
    port: u16,
    access_code: Option<String>,
) -> Reply<FormsStatus> {
    let mut server = forms_lock(&forms)?;
    if server.is_none() {
        let session = Arc::clone(&app);
        let started = FormServer::start(session, port, access_code, move || {
            let _ = window.emit("jxcel://changed", ());
        })
        .map_err(|e| e.to_string())?;
        *server = Some(started);
    }
    Ok(status_of(&server))
}

#[tauri::command(async)]
fn forms_stop(forms: Forms) -> Reply<FormsStatus> {
    // 取り出して手放す（待ち受けスレッドの停止を待つので、ロックは先に外す）
    let taken = forms_lock(&forms)?.take();
    drop(taken);
    Ok(FormsStatus::stopped())
}

#[tauri::command(async)]
fn forms_status(forms: Forms) -> Reply<FormsStatus> {
    Ok(status_of(&*forms_lock(&forms)?))
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
        .manage(Shared::new(Mutex::new(Session::new())))
        .manage(Mutex::<Option<FormServer>>::new(None))
        .invoke_handler(tauri::generate_handler![
            new_file,
            open_file,
            save_file,
            current_file,
            add_sheet,
            rename_sheet,
            delete_sheet,
            add_schema,
            add_column,
            update_column,
            delete_column,
            convert_datetime_offset,
            add_row,
            delete_row,
            set_cell,
            add_macro,
            macro_samples,
            update_macro,
            delete_macro,
            run_macro,
            add_export,
            update_export,
            replace_export_template,
            delete_export,
            export_preview,
            run_export,
            add_form,
            update_form,
            set_form_edit,
            delete_form,
            forms_start,
            forms_stop,
            forms_status,
            history_log,
            history_diff,
            restore,
        ])
        .run(tauri::generate_context!())
        .expect("jxcel の起動に失敗しました");
}
