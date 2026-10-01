import type { Change, Column, CommitInfo, Snapshot } from "./types";

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

  historyLog(): Promise<CommitInfo[]>;
  historyDiff(from: string, to: string): Promise<Change[]>;
  restore(rev: string): Promise<Snapshot>;
}

export const FILE_FILTER = { name: "jxcel", extensions: ["jxcel"] };

export async function createBackend(): Promise<Backend> {
  if ("__TAURI_INTERNALS__" in window) {
    return (await import("./tauriBackend")).tauriBackend;
  }
  return (await import("./mockBackend")).createMockBackend();
}
