import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open, save } from "@tauri-apps/plugin-dialog";
import { FILE_FILTER, TEMPLATE_FILTER, type Backend } from "./backend";

/**
 * e2e 用: OS のファイルダイアログは WebDriver から操作できないので、`window.__jxcelDialog` に
 * 関数（種類 → パスか null）が入っていれば、ダイアログの代わりにそれを使う。
 * Tauri の IPC（`__TAURI_INTERNALS__`）は凍結されていて、外から差し替えられないため。
 */
type DialogHook = (kind: "open" | "template" | "folder" | "save") => string | null;
const hook = (): DialogHook | undefined => (window as unknown as { __jxcelDialog?: DialogHook }).__jxcelDialog;

// Rust 側コマンドの引数名は snake_case だが、Tauri が camelCase のキーで受け付ける。
export const tauriBackend: Backend = {
  pickOpenPath: async () => {
    const h = hook();
    if (h) return h("open");
    const p = await open({ multiple: false, filters: [FILE_FILTER] });
    return typeof p === "string" ? p : null;
  },
  pickTemplatePath: async () => {
    const h = hook();
    if (h) return h("template");
    const p = await open({ multiple: false, filters: [TEMPLATE_FILTER] });
    return typeof p === "string" ? p : null;
  },
  pickFolder: async () => {
    const h = hook();
    if (h) return h("folder");
    const p = await open({ directory: true, multiple: false });
    return typeof p === "string" ? p : null;
  },
  pickSavePath: async (defaultName) => {
    const h = hook();
    if (h) return h("save");
    return save({ defaultPath: defaultName, filters: [FILE_FILTER] });
  },

  newFile: (name) => invoke("new_file", { name }),
  openFile: (path) => invoke("open_file", { path }),
  saveFile: (path, message) => invoke("save_file", { path, message }),

  current: () => invoke("current_file"),
  addSheet: (name) => invoke("add_sheet", { name }),
  renameSheet: (sheet, name) => invoke("rename_sheet", { sheet, name }),
  deleteSheet: (sheet) => invoke("delete_sheet", { sheet }),
  addSchema: (sheet, name, columns) => invoke("add_schema", { sheet, name, columns }),
  addColumn: (sheet, schema, column) => invoke("add_column", { sheet, schema, column }),
  updateColumn: (sheet, schema, column) => invoke("update_column", { sheet, schema, column }),
  deleteColumn: (sheet, schema, column) => invoke("delete_column", { sheet, schema, column }),
  convertDatetimeOffset: (sheet, schema, column, offset) => invoke("convert_datetime_offset", { sheet, schema, column, offset }),
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

  addForm: (sheet, schema, name) => invoke("add_form", { sheet, schema, name }),
  updateForm: (id, name, sheet, schema, columns, description) => invoke("update_form", { id, name, sheet, schema, columns, description }),
  setFormEdit: (id, allowed, minutes) => invoke("set_form_edit", { id, allowed, minutes }),
  deleteForm: (id) => invoke("delete_form", { id }),
  formsStart: (port, accessCode, respondentCodes) =>
    invoke("forms_start", { port, accessCode: accessCode || null, respondentCodes: respondentCodes?.length ? respondentCodes : null }),
  formsStop: () => invoke("forms_stop"),
  formsStatus: () => invoke("forms_status"),
  onFormsChanged: (handler) => listen("jxcel://changed", () => handler()),

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
