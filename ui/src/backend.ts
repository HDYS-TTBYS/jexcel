import type { Change, Column, CommitInfo, MacroSample, RunOutput, Snapshot } from "./types";

/** UI が必要とする操作。Tauri 実装とブラウザ単体用のモック実装がある。 */
export interface Backend {
  pickOpenPath(): Promise<string | null>;
  pickSavePath(defaultName: string): Promise<string | null>;

  newFile(name: string): Promise<Snapshot>;
  openFile(path: string): Promise<Snapshot>;
  saveFile(path: string | null, message: string): Promise<Snapshot>;

  addSheet(name: string): Promise<Snapshot>;
  renameSheet(sheet: string, name: string): Promise<Snapshot>;
  deleteSheet(sheet: string): Promise<Snapshot>;
  addSchema(sheet: string, name: string, columns: Column[]): Promise<Snapshot>;
  addColumn(sheet: string, schema: string, column: Column): Promise<Snapshot>;
  updateColumn(sheet: string, schema: string, column: Column): Promise<Snapshot>;
  deleteColumn(sheet: string, schema: string, column: string): Promise<Snapshot>;
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

  historyLog(): Promise<CommitInfo[]>;
  historyDiff(from: string, to: string): Promise<Change[]>;
  restore(rev: string): Promise<Snapshot>;

  /** ウィンドウを閉じる操作を横取りする。`handler` が呼ばれたら、閉じるかどうかは UI が決める。 */
  onCloseRequested(handler: () => void): Promise<() => void>;
  /** 確認なしでウィンドウを閉じる。 */
  closeWindow(): Promise<void>;
}

export const FILE_FILTER = { name: "jxcel", extensions: ["jxcel"] };

export async function createBackend(): Promise<Backend> {
  if ("__TAURI_INTERNALS__" in window) {
    return (await import("./tauriBackend")).tauriBackend;
  }
  return (await import("./mockBackend")).createMockBackend();
}
