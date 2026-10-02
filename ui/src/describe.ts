import type { Change, JxcelFile } from "./types";
import { formatValue } from "./parse";

/** 差分の 1 件を人が読める日本語にする。列名は現在のファイルから引き、なければ ID を出す。 */
export function describeChange(c: Change, file: JxcelFile): string {
  const colName = (sheet: string, schema: string, column: string) =>
    file.sheets.find((s) => s.id === sheet)?.schemas.find((s) => s.id === schema)?.columns.find((x) => x.id === column)
      ?.name ?? column;
  const show = (v: unknown) => (v === null || v === undefined ? "（空）" : `「${formatValue(v)}」`);

  switch (c.kind) {
    case "fileRenamed":
      return `ファイル名: ${c.old} → ${c.new}`;
    case "sheetAdded":
      return `シート「${c.name}」を追加`;
    case "sheetRemoved":
      return `シート「${c.name}」を削除`;
    case "sheetRenamed":
      return `シート名: ${c.old} → ${c.new}`;
    case "schemaAdded":
      return `スキーマ「${c.name}」を追加`;
    case "schemaRemoved":
      return `スキーマ「${c.name}」を削除`;
    case "schemaRenamed":
      return `スキーマ名: ${c.old} → ${c.new}`;
    case "columnAdded":
      return `列「${c.name}」を追加`;
    case "columnRemoved":
      return `列「${c.name}」を削除`;
    case "columnChanged": {
      const [o, n] = [c.old.computed, c.new.computed];
      if (!o && n) return `列「${c.new.name}」を計算列にした`;
      if (o && !n) return `列「${c.new.name}」を通常の列に戻した`;
      if (o && n && o.source !== n.source) return `列「${c.new.name}」の計算式を変更`;
      return `列「${c.new.name}」の定義を変更`;
    }
    case "rowAdded":
      return `行を追加 ${summarizeRow(c.cells.cells)}`;
    case "rowRemoved":
      return `行を削除 ${summarizeRow(c.cells.cells)}`;
    case "cellChanged":
      return `行 ${c.row.slice(-6)} の「${colName(c.sheet, c.schema, c.column)}」: ${show(c.old)} → ${show(c.new)}`;
    case "macroAdded":
      return `マクロ「${c.name}」を追加`;
    case "macroRemoved":
      return `マクロ「${c.name}」を削除`;
    case "macroRenamed":
      return `マクロ名: ${c.old} → ${c.new}`;
    case "macroEdited":
      return `マクロ「${c.name}」のコードを変更`;
    case "exportAdded":
      return `書き出し「${c.name}」を追加`;
    case "exportRemoved":
      return `書き出し「${c.name}」を削除`;
    case "exportRenamed":
      return `書き出し名: ${c.old} → ${c.new}`;
    case "exportChanged":
      return `書き出し「${c.name}」の設定を変更`;
    case "templateReplaced":
      return `書き出し「${c.name}」のテンプレートを差し替え`;
    case "formAdded":
      return `フォーム「${c.name}」を追加`;
    case "formRemoved":
      return `フォーム「${c.name}」を削除`;
    case "formChanged":
      return `フォーム「${c.name}」の設定を変更`;
    case "rowsReordered":
      return "行の並び順を変更";
  }
}

function summarizeRow(cells: Record<string, unknown>): string {
  const text = Object.values(cells).map(formatValue).filter(Boolean).join(", ");
  return text ? `(${text.length > 40 ? text.slice(0, 40) + "…" : text})` : "(空の行)";
}
