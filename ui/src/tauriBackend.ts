import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { FILE_FILTER, type Backend } from "./backend";

// Rust 側コマンドの引数名は snake_case だが、Tauri が camelCase のキーで受け付ける。
export const tauriBackend: Backend = {
  pickOpenPath: async () => {
    const p = await open({ multiple: false, filters: [FILE_FILTER] });
    return typeof p === "string" ? p : null;
  },
  pickSavePath: (defaultName) => save({ defaultPath: defaultName, filters: [FILE_FILTER] }),

  newFile: (name) => invoke("new_file", { name }),
  openFile: (path) => invoke("open_file", { path }),
  saveFile: (path, message) => invoke("save_file", { path, message }),

  addSheet: (name) => invoke("add_sheet", { name }),
  renameSheet: (sheet, name) => invoke("rename_sheet", { sheet, name }),
  deleteSheet: (sheet) => invoke("delete_sheet", { sheet }),
  addSchema: (sheet, name, columns) => invoke("add_schema", { sheet, name, columns }),
  addColumn: (sheet, schema, column) => invoke("add_column", { sheet, schema, column }),
  updateColumn: (sheet, schema, column) => invoke("update_column", { sheet, schema, column }),
  deleteColumn: (sheet, schema, column) => invoke("delete_column", { sheet, schema, column }),
  addRow: (sheet, schema) => invoke("add_row", { sheet, schema }),
  deleteRow: (sheet, schema, row) => invoke("delete_row", { sheet, schema, row }),
  setCell: (sheet, schema, row, column, value) => invoke("set_cell", { sheet, schema, row, column, value }),

  historyLog: () => invoke("history_log"),
  historyDiff: (from, to) => invoke("history_diff", { from, to }),
  restore: (rev) => invoke("restore", { rev }),
};
