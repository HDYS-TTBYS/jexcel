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
  /** 計算列。値は保存されず、式から計算される。 */
  computed?: { source?: string };
}

/** 計算列の 1 セルの結果。値か、そのセルだけのエラー。 */
export type CellResult = { v: unknown } | { e: string };

/** スキーマ ID → 列 ID → 行 ID → 結果 */
export type ComputedValues = Record<string, Record<string, Record<string, CellResult>>>;

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

export interface Macro {
  id: string;
  name: string;
  /** ユーザーが書いたままの TypeScript */
  source: string;
}

export type TemplateKind = "xlsx" | "docx";

/** 書き出し（テンプレートへの差し込み）の設定。テンプレート本体はバックエンドが持つ。 */
export interface Export {
  id: string;
  name: string;
  kind: TemplateKind;
  /** 取り込んだテンプレートの元のファイル名 */
  templateName: string;
  /** 対象の表（シート ID・スキーマ ID） */
  sheet: string;
  schema: string;
  /** 出力ファイル名（`{{ 式 }}` が使える） */
  filename: string;
  /** 空でなければ、この式が真になる行だけを書き出す */
  filter?: string;
}

/** 入力フォームの設定。LAN 内のブラウザから、表の行として入力できるようにする。 */
export interface Form {
  id: string;
  name: string;
  /** 回答の追加先の表（シート ID・スキーマ ID） */
  sheet: string;
  schema: string;
  /** 入力欄にする列の ID（表示順） */
  columns: string[];
  description?: string;
}

/** 配信中のフォームの URL */
export interface FormUrl {
  formId: string;
  name: string;
  url: string;
  /** 配信を始めてから受け付けた回答の数 */
  submitted: number;
}

export interface FormsStatus {
  running: boolean;
  port: number | null;
  urls: FormUrl[];
}

export interface JxcelFile {
  name: string;
  sheets: Sheet[];
  macros: Macro[];
  exports: Export[];
  forms: Form[];
}

export interface ExportPreviewRow {
  rowNo: number;
  /** 絞り込み条件で除外される行か */
  excluded: boolean;
  filename: CellResult;
  /** `placeholders` と同じ順の、欄ごとの値かエラー */
  values: CellResult[];
}

/** テンプレートの行ループ（`{{#each 式}}`） */
export interface ExportPreviewLoop {
  /** 繰り返す配列の式 */
  source: string;
  /** 入れ子のループなら、外側のループの番号（`loops` の添字） */
  parent?: number | null;
  /** ループの中の差し込み欄の式 */
  exprs: string[];
  /** プレビューした行ごとの繰り返しの回数（要素数）か、対象の式のエラー（入れ子のループは外側の要素すべての合計） */
  counts: CellResult[];
}

export interface ExportPreview {
  placeholders: string[];
  loops: ExportPreviewLoop[];
  totalRows: number;
  rows: ExportPreviewRow[];
}

export interface ExportResult {
  outDir: string;
  written: { rowNo: number; filename: string }[];
  skipped: number;
  errors: { rowNo: number; message: string }[];
}

/** 日時列のオフセット一括変換の結果 */
export interface ConvertResult {
  snapshot: Snapshot;
  /** 値が変わったセルの数 */
  converted: number;
  /** すでにそのオフセットで、変わらなかったセルの数 */
  unchanged: number;
  /** 変換できず、そのままにしたセルの数 */
  skipped: number;
}

/** 同梱のサンプルマクロ（実体は crates/jxcel-macro/samples/*.ts） */
export interface MacroSample {
  id: string;
  name: string;
  description: string;
  source: string;
}

export interface RunOutput {
  snapshot: Snapshot;
  logs: string[];
  result: unknown;
  /** 書き込み操作の数（0 ならファイルは変わっていない） */
  ops: number;
}

export interface Snapshot {
  file: JxcelFile;
  /** 計算列の値（状態を返すたびに計算される。保存はされない） */
  computed: ComputedValues;
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
  | { kind: "macroAdded"; id: string; name: string }
  | { kind: "macroRemoved"; id: string; name: string }
  | { kind: "macroRenamed"; id: string; old: string; new: string }
  | { kind: "macroEdited"; id: string; name: string }
  | { kind: "exportAdded"; id: string; name: string }
  | { kind: "exportRemoved"; id: string; name: string }
  | { kind: "exportRenamed"; id: string; old: string; new: string }
  | { kind: "exportChanged"; id: string; name: string }
  | { kind: "templateReplaced"; id: string; name: string }
  | { kind: "formAdded"; id: string; name: string }
  | { kind: "formRemoved"; id: string; name: string }
  | { kind: "formChanged"; id: string; name: string }
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
/** 計算列の式の雛形 */
export const FORMULA_TEMPLATE = `// row: この行（列名でアクセス）。jx: 他の表を引くための読み取り専用 API。
export default function (row: JxcelRow, jx: JxcelReadonly) {
  return null;
}
`;

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
