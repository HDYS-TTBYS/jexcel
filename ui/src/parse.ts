import type { DataType } from "./types";

export type ParseResult = { ok: true; value: unknown } | { ok: false; error: string };

const ok = (value: unknown): ParseResult => ({ ok: true, value });
const err = (error: string): ParseResult => ({ ok: false, error });

/** セルの表示用文字列。 */
export function formatValue(value: unknown): string {
  if (value === null || value === undefined) return "";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  return JSON.stringify(value);
}

/**
 * 入力テキストを型に沿った JSON 値へ変換する。空入力は null（値を消す）。
 * 最終的な検証はバックエンドが行うので、ここは「文字列→値」の変換と早期フィードバックが目的。
 */
export function parseInput(type: DataType, input: string): ParseResult {
  const text = input.trim();
  if (text === "") return ok(null);
  switch (type.kind) {
    case "string":
      return ok(input);
    case "int":
      return /^-?\d+$/.test(text) && Number.isSafeInteger(Number(text)) ? ok(Number(text)) : err("整数を入力してください");
    case "float":
      return Number.isFinite(Number(text)) ? ok(Number(text)) : err("数値を入力してください");
    case "decimal":
      return /^-?\d+(\.\d+)?$/.test(text) ? ok(text) : err("10進数を入力してください（例: 12.50）");
    case "bool":
      if (/^(true|1|はい|yes)$/i.test(text)) return ok(true);
      if (/^(false|0|いいえ|no)$/i.test(text)) return ok(false);
      return err("true / false で入力してください");
    case "date":
      return /^\d{4}-\d{2}-\d{2}$/.test(text) ? ok(text) : err("YYYY-MM-DD 形式で入力してください");
    case "dateTime":
      return /^\d{4}-\d{2}-\d{2}T/.test(text) ? ok(text) : err("RFC 3339 形式（例: 2024-01-02T03:04:05+09:00）で入力してください");
    case "enum":
      return type.values.includes(text) ? ok(text) : err(`${type.values.join(" / ")} から選んでください`);
    case "array":
    case "object":
    case "any":
    case "custom":
      try {
        return ok(JSON.parse(text));
      } catch {
        // ANY は JSON でなければ文字列として扱う
        return type.kind === "any" ? ok(input) : err("JSON として不正です");
      }
  }
}
