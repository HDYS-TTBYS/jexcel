// ブラウザ単体（`pnpm dev`）で UI を動かすための in-memory 実装。
// Rust 側の挙動を簡易的に再現するだけで、永続化はしない。

import type { Backend } from "./backend";
// Rust 側（crates/jxcel-macro）と同じ実行ライブラリを再利用する。
import prelude from "../../crates/jxcel-macro/src/prelude.js?raw";
import stdLib from "../../crates/jxcel-macro/src/std.js?raw";
import {
  defaultType,
  newId,
  type Change,
  type CellResult,
  type Column,
  type CommitInfo,
  type ComputedValues,
  type ExportPreview,
  type DataSchema,
  type DataType,
  type JxcelFile,
  type MacroSample,
  type RunOutput,
  type Snapshot,
} from "./types";

function validate(type: DataType, v: unknown): string | null {
  if (v === null || v === undefined) return null;
  switch (type.kind) {
    case "string":
      return typeof v === "string" ? null : "文字列ではありません";
    case "int":
      return Number.isInteger(v) ? null : "整数ではありません";
    case "float":
      return typeof v === "number" ? null : "数値ではありません";
    case "decimal":
      return typeof v === "string" && /^-?\d+(\.\d+)?$/.test(v) ? null : "10進数の文字列ではありません";
    case "bool":
      return typeof v === "boolean" ? null : "真偽値ではありません";
    case "date":
      return typeof v === "string" && /^\d{4}-\d{2}-\d{2}$/.test(v) ? null : "YYYY-MM-DD 形式の日付ではありません";
    case "dateTime":
      return typeof v === "string" && /^\d{4}-\d{2}-\d{2}T/.test(v) ? null : "RFC 3339 形式の日時ではありません";
    case "enum":
      return typeof v === "string" && type.values.includes(v) ? null : `${JSON.stringify(type.values)} のいずれかではありません`;
    case "array":
      if (!Array.isArray(v)) return "配列ではありません";
      for (const x of v) {
        const e = validate(type.item, x);
        if (e) return e;
      }
      return null;
    case "object":
      return typeof v === "object" && !Array.isArray(v) ? null : "オブジェクトではありません";
    default:
      return null;
  }
}

function diff(a: JxcelFile, b: JxcelFile): Change[] {
  const out: Change[] = [];
  for (const sb of b.sheets) {
    const sa = a.sheets.find((s) => s.id === sb.id);
    if (!sa) {
      out.push({ kind: "sheetAdded", sheet: sb.id, name: sb.name });
      continue;
    }
    if (sa.name !== sb.name) out.push({ kind: "sheetRenamed", sheet: sb.id, old: sa.name, new: sb.name });
    for (const cb of sb.schemas) {
      const ca = sa.schemas.find((s) => s.id === cb.id);
      if (!ca) {
        out.push({ kind: "schemaAdded", sheet: sb.id, schema: cb.id, name: cb.name });
        continue;
      }
      const base = { sheet: sb.id, schema: cb.id };
      for (const col of cb.columns) {
        const old = ca.columns.find((c) => c.id === col.id);
        if (!old) out.push({ kind: "columnAdded", ...base, column: col.id, name: col.name });
        else if (JSON.stringify(old) !== JSON.stringify(col))
          out.push({ kind: "columnChanged", ...base, column: col.id, old, new: col });
      }
      for (const col of ca.columns)
        if (!cb.columns.some((c) => c.id === col.id)) out.push({ kind: "columnRemoved", ...base, column: col.id, name: col.name });
      for (const row of ca.rows)
        if (!cb.rows.some((r) => r.id === row.id)) out.push({ kind: "rowRemoved", ...base, row: row.id, cells: row });
      for (const row of cb.rows) {
        const old = ca.rows.find((r) => r.id === row.id);
        if (!old) {
          out.push({ kind: "rowAdded", ...base, row: row.id, cells: row });
          continue;
        }
        for (const k of new Set([...Object.keys(old.cells), ...Object.keys(row.cells)])) {
          const [o, n] = [old.cells[k] ?? null, row.cells[k] ?? null];
          if (JSON.stringify(o) !== JSON.stringify(n)) out.push({ kind: "cellChanged", ...base, row: row.id, column: k, old: o, new: n });
        }
      }
    }
    for (const ca of sa.schemas)
      if (!sb.schemas.some((s) => s.id === ca.id)) out.push({ kind: "schemaRemoved", sheet: sb.id, schema: ca.id, name: ca.name });
  }
  for (const sa of a.sheets) if (!b.sheets.some((s) => s.id === sa.id)) out.push({ kind: "sheetRemoved", sheet: sa.id, name: sa.name });
  for (const m of b.macros) {
    const old = a.macros.find((x) => x.id === m.id);
    if (!old) out.push({ kind: "macroAdded", id: m.id, name: m.name });
    else {
      if (old.name !== m.name) out.push({ kind: "macroRenamed", id: m.id, old: old.name, new: m.name });
      if (old.source !== m.source) out.push({ kind: "macroEdited", id: m.id, name: m.name });
    }
  }
  for (const m of a.macros) if (!b.macros.some((x) => x.id === m.id)) out.push({ kind: "macroRemoved", id: m.id, name: m.name });
  for (const e of b.exports) {
    const old = a.exports.find((x) => x.id === e.id);
    if (!old) out.push({ kind: "exportAdded", id: e.id, name: e.name });
    else {
      if (old.name !== e.name) out.push({ kind: "exportRenamed", id: e.id, old: old.name, new: e.name });
      if (old.sheet !== e.sheet || old.schema !== e.schema || old.filename !== e.filename || (old.filter ?? "") !== (e.filter ?? ""))
        out.push({ kind: "exportChanged", id: e.id, name: e.name });
    }
  }
  for (const e of a.exports) if (!b.exports.some((x) => x.id === e.id)) out.push({ kind: "exportRemoved", id: e.id, name: e.name });
  return out;
}

type Op =
  | { op: "add"; sheet: string; schema: string; row: string; cells: Record<string, unknown> }
  | { op: "update"; sheet: string; schema: string; row: string; cells: Record<string, unknown> }
  | { op: "remove"; sheet: string; schema: string; row: string };

/**
 * ブラウザ単体用のマクロ実行。TypeScript の変換はできないので、型注釈のない JS だけ動く
 * （本物の実行は Rust 側の QuickJS）。`export default` を式にして呼ぶ。
 */
function runMacroInBrowser(source: string, file: JxcelFile): { ops: Op[]; logs: string[]; result: unknown } {
  // prelude は globalThis.console を差し替えるので、本物の globalThis を隠して実行する
  const fakeGlobal = loadRuntime();
  const state = (fakeGlobal.__makeState as (d: JxcelFile) => { jx: unknown; finish: (r: unknown) => string })(structuredClone(file));
  const body = source.replace(/export\s+default\s+/, "const __main = ") + "\n;return __main(jx);";
  const result = new Function("jx", "console", "std", body)(state.jx, (fakeGlobal.console as object) ?? console, fakeGlobal.std);
  return JSON.parse(state.finish(result));
}

/** prelude と std を、本物の globalThis を隠した環境に読み込む。マクロ・計算式に std を渡すために使う。 */
function loadRuntime(): Record<string, unknown> {
  (window as unknown as { __newId?: () => string }).__newId = newId;
  const fake: Record<string, unknown> = {};
  new Function("globalThis", prelude)(fake);
  new Function("globalThis", stdLib)(fake);
  return fake;
}

/**
 * ブラウザ単体用の計算列の評価。Rust 側と同じ prelude.js の computeAll を使う。
 * TypeScript は変換できないので、型注釈のない JS の式だけ動く。
 */
function computeInBrowser(file: JxcelFile): ComputedValues {
  if (!file.sheets.some((s) => s.schemas.some((c) => c.columns.some((x) => x.computed)))) return {};
  const runtime = loadRuntime();
  const specs: { sheet: string; schema: string; column: string; error?: string }[] = [];
  const fns: (Function | undefined)[] = [];
  for (const sh of file.sheets)
    for (const sc of sh.schemas)
      for (const col of sc.columns) {
        if (!col.computed) continue;
        const spec: (typeof specs)[number] = { sheet: sh.id, schema: sc.id, column: col.id };
        const src = col.computed.source ?? "";
        if (!src.trim()) spec.error = "式が空です";
        else {
          try {
            const f = new Function("std", src.replace(/export\s+default\s+/, "const __f = ") + "\n;return __f;")(runtime.std);
            if (typeof f !== "function") throw new Error("export default function (row, jx) { return ... } の形で書いてください");
            fns[specs.length] = f;
          } catch (e) {
            spec.error = e instanceof Error ? e.message : String(e);
          }
        }
        specs.push(spec);
      }
  if (specs.length === 0) return {};

  const fake = runtime;
  fake.__fns = fns;
  const state = (fake.__makeState as (d: JxcelFile) => { computeAll: (s: unknown) => void; finish: (r: unknown) => string })(
    structuredClone(file),
  );
  state.computeAll(specs);
  const out: { schema: string; column: string; row: string; v?: unknown; e?: string }[] = JSON.parse(state.finish(null)).computed;

  const result: ComputedValues = {};
  for (const c of out) {
    const col = file.sheets.flatMap((s) => s.schemas).find((s) => s.id === c.schema)?.columns.find((x) => x.id === c.column);
    let cell: CellResult;
    if (c.e !== undefined) cell = { e: c.e };
    else {
      const bad = col ? validate(col.type, c.v ?? null) : null;
      cell = bad ? { e: `${col?.name}: ${bad}` } : { v: c.v ?? null };
    }
    ((result[c.schema] ??= {})[c.column] ??= {})[c.row] = cell;
  }
  return result;
}

// サンプルマクロ（実体は Rust 側と共通の crates/jxcel-macro/samples/*.ts）。@name / @desc の行は取り除く
const sampleFiles = import.meta.glob("../../crates/jxcel-macro/samples/*.ts", { query: "?raw", import: "default", eager: true }) as Record<string, string>;
function mockSamples(): MacroSample[] {
  return Object.entries(sampleFiles).map(([path, src]) => {
    const id = path.split("/").pop()!.replace(/\.ts$/, "");
    let name = id;
    let description = "";
    const body: string[] = [];
    for (const line of src.split("\n")) {
      if (line.startsWith("// @name ")) name = line.slice(9).trim();
      else if (line.startsWith("// @desc ")) description = line.slice(9).trim();
      else body.push(line);
    }
    return { id, name, description, source: `// ${description}\n${body.join("\n").replace(/^\n+/, "")}\n` };
  });
}

const PLACEHOLDER_RE = /\{\{(.+?)\}\}/g;
const placeholdersOf = (text: string) => [...text.matchAll(PLACEHOLDER_RE)].map((m) => m[1].trim()).filter(Boolean);

/** 差し込み欄の式を全行で評価する（Rust 側と同じ prelude.js の evalExprs）。 */
function evalExprsInBrowser(file: JxcelFile, sheet: string, schemaId: string, exprs: string[]): { v?: unknown; e?: string }[][] {
  const fake = loadRuntime();
  const state = (fake.__makeState as (d: JxcelFile) => { evalExprs: (s: unknown) => void; finish: (r: unknown) => string })(structuredClone(file));
  state.evalExprs({ sheet, schemaId, exprs });
  return JSON.parse(state.finish(null)).exprs;
}

export function createMockBackend(): Backend {
  let file: JxcelFile | null = null;
  let path: string | null = null;
  let dirty = false;
  const commits: { info: CommitInfo; file: JxcelFile }[] = [];
  // ブラウザ内の「ディスク」。パス → { file, commits }
  const disk = new Map<string, { file: JxcelFile; commits: typeof commits }>();

  // ブラウザ単体にはテンプレートの中身を読む手段がないので、差し込み欄は「対象の表の全列」として見せる
  const mockPlaceholders = new Map<string, string[]>();
  const clone = <T,>(x: T): T => structuredClone(x);
  const snap = (): Snapshot => {
    if (!file) throw "ファイルが開かれていません";
    return { file: clone(file), computed: computeInBrowser(file), path, dirty };
  };
  const edit = (f: (file: JxcelFile) => void): Snapshot => {
    if (!file) throw "ファイルが開かれていません";
    const next = clone(file);
    f(next);
    file = next;
    dirty = true;
    return snap();
  };
  const schemaOf = (f: JxcelFile, sheet: string, schema: string): DataSchema => {
    const s = f.sheets.find((x) => x.id === sheet)?.schemas.find((x) => x.id === schema);
    if (!s) throw "スキーマ が見つかりません";
    return s;
  };

  return {
    pickOpenPath: async () => {
      const first = [...disk.keys()][0];
      return first ?? null;
    },
    pickSavePath: async (name) => `/mock/${name}`,
    pickTemplatePath: async () => "/mock/請求書.docx",
    pickFolder: async () => "/mock/out",

    newFile: async (name) => {
      const col: Column = { id: newId(), name: "列1", type: defaultType("string") };
      file = {
        name,
        sheets: [
          { id: newId(), name: "シート1", schemas: [{ id: newId(), name: "データ", columns: [col], rows: [{ id: newId(), cells: {} }] }] },
        ],
        macros: [],
        exports: [],
      };
      path = null;
      dirty = false;
      commits.length = 0;
      return snap();
    },
    openFile: async (p) => {
      const d = disk.get(p);
      if (!d) throw `${p} を開けません`;
      file = clone(d.file);
      path = p;
      dirty = false;
      commits.length = 0;
      commits.push(...clone(d.commits));
      return snap();
    },
    saveFile: async (p, message) => {
      if (!file) throw "ファイルが開かれていません";
      if (p) path = p;
      if (!path) throw "保存先が未指定です";
      const last = commits[commits.length - 1];
      if (!last || JSON.stringify(last.file) !== JSON.stringify(file)) {
        commits.push({ info: { id: newId(), message: message.trim() || "保存", time: Math.floor(Date.now() / 1000) }, file: clone(file) });
      }
      disk.set(path, { file: clone(file), commits: clone(commits) });
      dirty = false;
      return snap();
    },

    addSheet: async (name) => edit((f) => void f.sheets.push({ id: newId(), name, schemas: [] })),
    renameSheet: async (sheet, name) =>
      edit((f) => {
        const s = f.sheets.find((x) => x.id === sheet);
        if (!s) throw "シート が見つかりません";
        s.name = name;
      }),
    deleteSheet: async (sheet) => edit((f) => void (f.sheets = f.sheets.filter((s) => s.id !== sheet))),
    addSchema: async (sheet, name, columns) =>
      edit((f) => {
        const s = f.sheets.find((x) => x.id === sheet);
        if (!s) throw "シート が見つかりません";
        s.schemas.push({ id: newId(), name, columns, rows: [] });
      }),
    addColumn: async (sheet, schema, column) =>
      edit((f) => {
        const s = schemaOf(f, sheet, schema);
        if (column.computed) for (const r of s.rows) delete r.cells[column.id]; // 計算列は値を保存しない
        s.columns.push(column);
      }),
    updateColumn: async (sheet, schema, column) =>
      edit((f) => {
        const s = schemaOf(f, sheet, schema);
        const i = s.columns.findIndex((c) => c.id === column.id);
        if (i < 0) throw "列 が見つかりません";
        s.columns[i] = column;
        for (const r of s.rows) {
          if (column.computed) {
            delete r.cells[column.id]; // 通常の列を計算列にしたら、保存していた値は捨てる
            continue;
          }
          const e = validate(column.type, r.cells[column.id]);
          if (e) throw `既存の値が新しい定義に合いません: ${e}`;
        }
      }),
    deleteColumn: async (sheet, schema, column) =>
      edit((f) => {
        const s = schemaOf(f, sheet, schema);
        s.columns = s.columns.filter((c) => c.id !== column);
        for (const r of s.rows) delete r.cells[column];
      }),
    addRow: async (sheet, schema) => edit((f) => void schemaOf(f, sheet, schema).rows.push({ id: newId(), cells: {} })),
    deleteRow: async (sheet, schema, row) =>
      edit((f) => {
        const s = schemaOf(f, sheet, schema);
        s.rows = s.rows.filter((r) => r.id !== row);
      }),
    setCell: async (sheet, schema, row, column, value) =>
      edit((f) => {
        const s = schemaOf(f, sheet, schema);
        const col = s.columns.find((c) => c.id === column);
        const r = s.rows.find((x) => x.id === row);
        if (!col || !r) throw "対象が見つかりません";
        if (col.computed) throw `「${col.name}」は計算列なので編集できません`;
        if ((value === null || value === undefined) && col.required) throw `${col.name} は必須です`;
        const e = validate(col.type, value);
        if (e) throw e;
        if (value === null || value === undefined) delete r.cells[column];
        else r.cells[column] = value;
      }),

    addMacro: async (name, source) =>
      edit((f) => {
        f.macros.push({
          id: newId(),
          name,
          source:
            source ??
            'export default function (jx) {\n  const rows = jx.sheet("シート1").schema("データ").rows();\n  jx.log(rows.length + " 行");\n}\n',
        });
      }),
    macroSamples: async () => mockSamples(),
    updateMacro: async (id, name, source) => {
      const cur = file?.macros.find((m) => m.id === id);
      if (cur && cur.name === name && cur.source === source) return snap(); // 変更なしなら未保存にしない
      return edit((f) => {
        const m = f.macros.find((x) => x.id === id);
        if (!m) throw "マクロ が見つかりません";
        m.name = name;
        m.source = source;
      });
    },
    deleteMacro: async (id) =>
      edit((f) => {
        if (!f.macros.some((m) => m.id === id)) throw "マクロ が見つかりません";
        f.macros = f.macros.filter((m) => m.id !== id);
      }),
    runMacro: async (id, source): Promise<RunOutput> => {
      if (!file) throw "ファイルが開かれていません";
      const m = file.macros.find((x) => x.id === id);
      if (!m) throw "マクロ が見つかりません";
      let out;
      try {
        out = runMacroInBrowser(source ?? m.source, file);
      } catch (e) {
        throw `マクロ: 実行エラー: ${e instanceof Error ? e.message : String(e)}（ブラウザ単体では型注釈のない JS だけ実行できます）`;
      }
      if (out.ops.length > 0) {
        const next = clone(file);
        for (const op of out.ops) {
          const s = schemaOf(next, op.sheet, op.schema);
          if (op.op === "add") s.rows.push({ id: op.row, cells: Object.fromEntries(Object.entries(op.cells).filter(([, v]) => v !== null)) });
          else if (op.op === "update") {
            const r = s.rows.find((x) => x.id === op.row);
            if (!r) throw "行 が見つかりません";
            for (const [k, v] of Object.entries(op.cells)) {
              if (v === null) delete r.cells[k];
              else r.cells[k] = v;
            }
          } else s.rows = s.rows.filter((x) => x.id !== op.row);
        }
        file = next;
        dirty = true;
      }
      return { snapshot: snap(), logs: out.logs, result: out.result ?? null, ops: out.ops.length };
    },

    addExport: async (templatePath, name) => {
      const kind = /\.xlsx$/i.test(templatePath) ? "xlsx" : /\.docx$/i.test(templatePath) ? "docx" : null;
      if (!kind) throw "テンプレートは .xlsx か .docx のファイルを選んでください";
      return edit((f) => {
        const sheet = f.sheets[0];
        const schema = sheet?.schemas[0];
        if (!schema) throw "書き出しの対象にする表（スキーマ）がありません";
        const id = newId();
        const fileName = templatePath.split("/").pop()!;
        f.exports.push({
          id,
          name: name ?? fileName.replace(/\.[^.]+$/, ""),
          kind,
          templateName: fileName,
          sheet: sheet.id,
          schema: schema.id,
          filename: schema.columns[0] ? `{{${schema.columns[0].name}}}` : "{{_no}}",
        });
        mockPlaceholders.set(id, schema.columns.map((c) => c.name));
      });
    },
    updateExport: async (id, name, sheet, schema, filename, filter) =>
      edit((f) => {
        if (!f.sheets.some((s) => s.id === sheet && s.schemas.some((c) => c.id === schema))) throw "書き出しの対象の表 が見つかりません";
        const e = f.exports.find((x) => x.id === id);
        if (!e) throw "書き出し が見つかりません";
        Object.assign(e, { name, sheet, schema, filename, filter: filter?.trim() ? filter.trim() : undefined });
      }),
    replaceExportTemplate: async (id, templatePath) =>
      edit((f) => {
        const e = f.exports.find((x) => x.id === id);
        if (!e) throw "書き出し が見つかりません";
        if (!new RegExp(`\\.${e.kind}$`, "i").test(templatePath)) throw `この書き出しのテンプレートは .${e.kind} です。同じ種類のファイルを選んでください`;
        e.templateName = templatePath.split("/").pop()!;
      }),
    deleteExport: async (id) =>
      edit((f) => {
        if (!f.exports.some((e) => e.id === id)) throw "書き出し が見つかりません";
        f.exports = f.exports.filter((e) => e.id !== id);
        mockPlaceholders.delete(id);
      }),
    exportPreview: async (id, limit): Promise<ExportPreview> => {
      if (!file) throw "ファイルが開かれていません";
      const e = file.exports.find((x) => x.id === id);
      if (!e) throw "書き出し が見つかりません";
      const placeholders = mockPlaceholders.get(id) ?? [];
      const nameExprs = placeholdersOf(e.filename || "{{_no}}");
      const filter = e.filter?.trim();
      const exprs = [...new Set([...placeholders, ...nameExprs, ...(filter ? [filter] : [])])];
      const matrix = evalExprsInBrowser(file, e.sheet, e.schema, exprs);
      const cell = (r: { v?: unknown; e?: string }): CellResult => (r.e !== undefined ? { e: r.e } : { v: r.v ?? null });
      const used = new Set<string>();
      return {
        placeholders,
        totalRows: matrix.length,
        rows: matrix.slice(0, limit).map((row, i) => {
          const at = (x: string) => row[exprs.indexOf(x)];
          const bad = nameExprs.map(at).find((r) => r.e !== undefined);
          let filename: CellResult;
          if (bad) filename = { e: `ファイル名の「${nameExprs[nameExprs.map(at).indexOf(bad)]}」: ${bad.e}` };
          else {
            const stem = (e.filename || "{{_no}}").replace(PLACEHOLDER_RE, (_m, x: string) => String(at(x.trim()).v ?? "")).replace(/[\\/:*?"<>|]/g, "_").trim() || `row${i + 1}`;
            let name = `${stem}.${e.kind}`;
            for (let n = 2; used.has(name.toLowerCase()); n++) name = `${stem} (${n}).${e.kind}`;
            used.add(name.toLowerCase());
            filename = { v: name };
          }
          const f = filter ? at(filter) : undefined;
          return { rowNo: i + 1, excluded: !!f && f.e === undefined && !f.v, filename, values: placeholders.map((p) => cell(at(p))) };
        }),
      };
    },
    runExport: async () => {
      throw "ブラウザ単体ではファイルを書き出せません（デスクトップ版で実行してください）";
    },

    historyLog: async () => [...commits].reverse().map((c) => clone(c.info)),
    historyDiff: async (from, to) => {
      const [a, b] = [commits.find((c) => c.info.id === from), commits.find((c) => c.info.id === to)];
      if (!a || !b) throw "リビジョンが見つかりません";
      return diff(a.file, b.file);
    },
    restore: async (rev) => {
      const c = commits.find((x) => x.info.id === rev);
      if (!c) throw "リビジョンが見つかりません";
      file = clone(c.file);
      dirty = true;
      return snap();
    },

    // ブラウザには「閉じる」を横取りする手段がないので、テスト用に window から呼べるようにする
    onCloseRequested: async (handler) => {
      const w = window as unknown as { __requestClose?: () => void };
      w.__requestClose = handler;
      // StrictMode の再マウントで後から登録された分を消さないよう、自分の分だけ解除する
      return () => {
        if (w.__requestClose === handler) delete w.__requestClose;
      };
    },
    closeWindow: async () => {
      (window as unknown as { __closed?: boolean }).__closed = true;
    },
  };
}
