// ブラウザ単体（`pnpm dev`）で UI を動かすための in-memory 実装。
// Rust 側の挙動を簡易的に再現するだけで、永続化はしない。

import type { Backend } from "./backend";
import {
  defaultType,
  newId,
  type Change,
  type Column,
  type CommitInfo,
  type DataSchema,
  type DataType,
  type JxcelFile,
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
  return out;
}

export function createMockBackend(): Backend {
  let file: JxcelFile | null = null;
  let path: string | null = null;
  let dirty = false;
  const commits: { info: CommitInfo; file: JxcelFile }[] = [];
  // ブラウザ内の「ディスク」。パス → { file, commits }
  const disk = new Map<string, { file: JxcelFile; commits: typeof commits }>();

  const clone = <T,>(x: T): T => structuredClone(x);
  const snap = (): Snapshot => {
    if (!file) throw "ファイルが開かれていません";
    return { file: clone(file), path, dirty };
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

    newFile: async (name) => {
      const col: Column = { id: newId(), name: "列1", type: defaultType("string") };
      file = {
        name,
        sheets: [
          { id: newId(), name: "シート1", schemas: [{ id: newId(), name: "データ", columns: [col], rows: [{ id: newId(), cells: {} }] }] },
        ],
      };
      path = null;
      dirty = true;
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
    addColumn: async (sheet, schema, column) => edit((f) => void schemaOf(f, sheet, schema).columns.push(column)),
    updateColumn: async (sheet, schema, column) =>
      edit((f) => {
        const s = schemaOf(f, sheet, schema);
        const i = s.columns.findIndex((c) => c.id === column.id);
        if (i < 0) throw "列 が見つかりません";
        s.columns[i] = column;
        for (const r of s.rows) {
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
        if ((value === null || value === undefined) && col.required) throw `${col.name} は必須です`;
        const e = validate(col.type, value);
        if (e) throw e;
        if (value === null || value === undefined) delete r.cells[column];
        else r.cells[column] = value;
      }),

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
  };
}
