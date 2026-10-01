// Rust 側 (jxcel-core / jxcel-app) の serde 表現と対応させる。

export type DataType =
  | { kind: "string" }
  | { kind: "int" }
  | { kind: "float" }
  | { kind: "decimal" }
  | { kind: "bool" }
  | { kind: "date" }
  | { kind: "dateTime" }
  | { kind: "enum"; values: string[] }
  | { kind: "object"; fields: Column[] }
  | { kind: "array"; item: DataType }
  | { kind: "any" }
  | { kind: "custom"; name: string };

export interface Column {
  id: string;
  name: string;
  type: DataType;
  required?: boolean;
}

export interface Row {
  id: string;
  cells: Record<string, unknown>;
}

export interface DataSchema {
  id: string;
  name: string;
  columns: Column[];
  rows: Row[];
}

export interface Sheet {
  id: string;
  name: string;
  schemas: DataSchema[];
}

export interface JxcelFile {
  name: string;
  sheets: Sheet[];
}

export interface Snapshot {
  file: JxcelFile;
  path: string | null;
  dirty: boolean;
}

export interface CommitInfo {
  id: string;
  message: string;
  time: number;
}

export type Change =
  | { kind: "fileRenamed"; old: string; new: string }
  | { kind: "sheetAdded"; sheet: string; name: string }
  | { kind: "sheetRemoved"; sheet: string; name: string }
  | { kind: "sheetRenamed"; sheet: string; old: string; new: string }
  | { kind: "schemaAdded"; sheet: string; schema: string; name: string }
  | { kind: "schemaRemoved"; sheet: string; schema: string; name: string }
  | { kind: "schemaRenamed"; sheet: string; schema: string; old: string; new: string }
  | { kind: "columnAdded"; sheet: string; schema: string; column: string; name: string }
  | { kind: "columnRemoved"; sheet: string; schema: string; column: string; name: string }
  | { kind: "columnChanged"; sheet: string; schema: string; column: string; old: Column; new: Column }
  | { kind: "rowAdded"; sheet: string; schema: string; row: string; cells: Row }
  | { kind: "rowRemoved"; sheet: string; schema: string; row: string; cells: Row }
  | { kind: "cellChanged"; sheet: string; schema: string; row: string; column: string; old: unknown; new: unknown }
  | { kind: "rowsReordered"; sheet: string; schema: string };

export const SCALAR_KINDS = ["string", "int", "float", "decimal", "bool", "date", "dateTime", "enum", "any"] as const;
export const ALL_KINDS = [...SCALAR_KINDS, "array", "object"] as const;
export type Kind = (typeof ALL_KINDS)[number];

export const KIND_LABEL: Record<string, string> = {
  string: "文字列",
  int: "整数",
  float: "小数(浮動)",
  decimal: "10進数",
  bool: "真偽値",
  date: "日付",
  dateTime: "日時",
  enum: "選択肢",
  any: "ANY",
  array: "配列",
  object: "オブジェクト",
  custom: "カスタム",
};

/** 衝突しない ID（ULID の代わりに UUID を使う。サーバ側は文字種のみ検査する）。 */
export const newId = () => crypto.randomUUID().replace(/-/g, "");

export function defaultType(kind: Kind): DataType {
  switch (kind) {
    case "enum":
      return { kind: "enum", values: [] };
    case "array":
      return { kind: "array", item: { kind: "string" } };
    case "object":
      return { kind: "object", fields: [] };
    default:
      return { kind };
  }
}
