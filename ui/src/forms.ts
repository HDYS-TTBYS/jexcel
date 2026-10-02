import type { Column } from "./types";

/** フォームの入力欄にできる列か。計算列とネスト型（オブジェクト・配列）・any・カスタム型は不可。
 *  Rust 側の `jxcel_app::forms::field_kind` と同じ規則。 */
export function canBeField(c: Column): boolean {
  if (c.computed) return false;
  return ["string", "int", "float", "decimal", "bool", "date", "dateTime", "enum"].includes(c.type.kind);
}
