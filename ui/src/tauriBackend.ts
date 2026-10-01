import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open, save } from "@tauri-apps/plugin-dialog";
import { FILE_FILTER, TEMPLATE_FILTER, type Backend } from "./backend";

// Rust 側コマンドの引数名は snake_case だが、Tauri が camelCase のキーで受け付ける。
export const tauriBackend: Backend = {
  pickOpenPath: async () => {
    const p = await open({ multiple: false, filters: [FILE_FILTER] });
    return typeof p === "string" ? p : null;
  },
  pickTemplatePath: async () => {
    const p = await open({ multiple: false, filters: [TEMPLATE_FILTER] });
    return typeof p === "string" ? p : null;
  },
  pickFolder: async () => {
    const p = await open({ directory: true, multiple: false });
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

  addMacro: (name, source) => invoke("add_macro", { name, source: source ?? null }),
  macroSamples: () => invoke("macro_samples"),
  updateMacro: (id, name, source) => invoke("update_macro", { id, name, source }),
  deleteMacro: (id) => invoke("delete_macro", { id }),
  runMacro: (id, source) => invoke("run_macro", { id, source }),

  addExport: (templatePath, name) => invoke("add_export", { templatePath, name: name ?? null }),
  updateExport: (id, name, sheet, schema, filename, filter) => invoke("update_export", { id, name, sheet, schema, filename, filter }),
  replaceExportTemplate: (id, templatePath) => invoke("replace_export_template", { id, templatePath }),
  deleteExport: (id) => invoke("delete_export", { id }),
  exportPreview: (id, limit) => invoke("export_preview", { id, limit }),
  runExport: (id, outDir) => invoke("run_export", { id, outDir }),

  historyLog: () => invoke("history_log"),
  historyDiff: (from, to) => invoke("history_diff", { from, to }),
  restore: (rev) => invoke("restore", { rev }),

  onCloseRequested: (handler) =>
    getCurrentWindow().onCloseRequested((event) => {
      event.preventDefault();
      handler();
    }),
  // close() だと上の handler が再び呼ばれるので、確認済みの終了は destroy() で行う
  closeWindow: () => getCurrentWindow().destroy(),
};
