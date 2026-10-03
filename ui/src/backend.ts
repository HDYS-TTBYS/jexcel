import type { Change, Column, CommitInfo, ConvertResult, ExportPreview, ExportResult, FormsStatus, MacroSample, RunOutput, Snapshot } from "./types";

/** UI が必要とする操作。Tauri 実装とブラウザ単体用のモック実装がある。 */
export interface Backend {
  pickOpenPath(): Promise<string | null>;
  pickSavePath(defaultName: string): Promise<string | null>;
  /** 書き出しのテンプレート（xlsx / docx）を選ぶ */
  pickTemplatePath(): Promise<string | null>;
  /** 書き出し先のフォルダを選ぶ */
  pickFolder(): Promise<string | null>;

  newFile(name: string): Promise<Snapshot>;
  openFile(path: string): Promise<Snapshot>;
  saveFile(path: string | null, message: string): Promise<Snapshot>;

  /** 開いているファイルの現在の状態（配信中に回答が届いたあとの再取得などに使う）。 */
  current(): Promise<Snapshot>;
  addSheet(name: string): Promise<Snapshot>;
  renameSheet(sheet: string, name: string): Promise<Snapshot>;
  deleteSheet(sheet: string): Promise<Snapshot>;
  addSchema(sheet: string, name: string, columns: Column[]): Promise<Snapshot>;
  addColumn(sheet: string, schema: string, column: Column): Promise<Snapshot>;
  updateColumn(sheet: string, schema: string, column: Column): Promise<Snapshot>;
  deleteColumn(sheet: string, schema: string, column: string): Promise<Snapshot>;
  /** 日時型の列の値を、同じ時刻のまま別のオフセット（+09:00 など）の表記に直す。 */
  convertDatetimeOffset(sheet: string, schema: string, column: string, offset: string): Promise<ConvertResult>;
  addRow(sheet: string, schema: string): Promise<Snapshot>;
  deleteRow(sheet: string, schema: string, row: string): Promise<Snapshot>;
  setCell(sheet: string, schema: string, row: string, column: string, value: unknown): Promise<Snapshot>;

  /** `source` を渡すとその内容（サンプルなど）で、無ければ雛形で追加する */
  addMacro(name: string, source?: string): Promise<Snapshot>;
  macroSamples(): Promise<MacroSample[]>;
  updateMacro(id: string, name: string, source: string): Promise<Snapshot>;
  deleteMacro(id: string): Promise<Snapshot>;
  /** `source` を渡すと保存前のエディタの内容で実行する。失敗したときは何も変更しない。 */
  runMacro(id: string, source: string | null): Promise<RunOutput>;

  /** テンプレートを取り込んで書き出しを追加する（対象は先頭の表） */
  addExport(templatePath: string, name?: string): Promise<Snapshot>;
  updateExport(id: string, name: string, sheet: string, schema: string, filename: string, filter: string | null): Promise<Snapshot>;
  replaceExportTemplate(id: string, templatePath: string): Promise<Snapshot>;
  deleteExport(id: string): Promise<Snapshot>;
  /** 書き出す前の確認。テンプレートの欄と、先頭 limit 行の値・出力ファイル名 */
  exportPreview(id: string, limit: number): Promise<ExportPreview>;
  /** 全行を書き出す。既存のファイルは上書きしない */
  runExport(id: string, outDir: string): Promise<ExportResult>;

  addForm(sheet: string, schema: string, name: string): Promise<Snapshot>;
  updateForm(id: string, name: string, sheet: string, schema: string, columns: string[], description: string): Promise<Snapshot>;
  /** 送信済みの回答の修正の設定。`minutes` が null なら期限なし */
  setFormEdit(id: string, allowed: boolean, minutes: number | null): Promise<Snapshot>;
  deleteForm(id: string): Promise<Snapshot>;
  /** LAN へのフォーム配信。回答は開いているファイルに行として追加される（未保存の変更になる）。 */
  /** accessCode を渡すと、回答者に合言葉を求める（ASCII の 4〜64 文字。ファイルには保存しない） */
  formsStart(port: number, accessCode?: string): Promise<FormsStatus>;
  formsStop(): Promise<FormsStatus>;
  formsStatus(): Promise<FormsStatus>;
  /** 配信中に回答が届いて表が変わったときに呼ばれる。 */
  onFormsChanged(handler: () => void): Promise<() => void>;

  historyLog(): Promise<CommitInfo[]>;
  historyDiff(from: string, to: string): Promise<Change[]>;
  restore(rev: string): Promise<Snapshot>;

  /** ウィンドウを閉じる操作を横取りする。`handler` が呼ばれたら、閉じるかどうかは UI が決める。 */
  onCloseRequested(handler: () => void): Promise<() => void>;
  /** 確認なしでウィンドウを閉じる。 */
  closeWindow(): Promise<void>;
}

export const FILE_FILTER = { name: "jxcel", extensions: ["jxcel"] };
export const TEMPLATE_FILTER = { name: "Word / Excel", extensions: ["xlsx", "docx"] };

export async function createBackend(): Promise<Backend> {
  if ("__TAURI_INTERNALS__" in window) {
    return (await import("./tauriBackend")).tauriBackend;
  }
  return (await import("./mockBackend")).createMockBackend();
}
