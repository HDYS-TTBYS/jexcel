import { instantOf } from "./datetime";
import type { DataType } from "./types";

const collator = new Intl.Collator("ja");

/** 10 進数の文字列（"-12.50" など）を、誤差なく比べる。 */
export function compareDecimal(a: string, b: string): number {
  const split = (s: string) => {
    const neg = s.startsWith("-");
    const [i, f = ""] = (neg ? s.slice(1) : s).split(".");
    return { neg, i: i.replace(/^0+(?=\d)/, ""), f: f.replace(/0+$/, "") };
  };
  const [x, y] = [split(a), split(b)];
  const zero = (v: { i: string; f: string }) => v.i === "0" && v.f === "";
  const sx = zero(x) ? 0 : x.neg ? -1 : 1;
  const sy = zero(y) ? 0 : y.neg ? -1 : 1;
  if (sx !== sy) return sx < sy ? -1 : 1;
  if (sx === 0) return 0;
  let c = x.i.length !== y.i.length ? (x.i.length < y.i.length ? -1 : 1) : x.i < y.i ? -1 : x.i > y.i ? 1 : 0;
  if (c === 0) {
    const n = Math.max(x.f.length, y.f.length);
    const [fx, fy] = [x.f.padEnd(n, "0"), y.f.padEnd(n, "0")];
    c = fx < fy ? -1 : fx > fy ? 1 : 0;
  }
  return c * sx;
}

/** 空でない 2 つの値を、列の型に合わせて比べる（グリッドの並べ替え用）。
 *  数値は数として、10 進数は誤差なく、オフセット付きの日時は文字列ではなく瞬間で比べる。 */
export function compareValues(type: DataType, a: unknown, b: unknown): number {
  switch (type.kind) {
    case "int":
    case "float":
      if (typeof a === "number" && typeof b === "number") return a < b ? -1 : a > b ? 1 : 0;
      break;
    case "decimal":
      if (typeof a === "string" && typeof b === "string" && /^-?\d+(\.\d+)?$/.test(a) && /^-?\d+(\.\d+)?$/.test(b)) return compareDecimal(a, b);
      break;
    case "bool":
      if (typeof a === "boolean" && typeof b === "boolean") return a === b ? 0 : a ? 1 : -1;
      break;
    case "dateTime": {
      const [x, y] = [typeof a === "string" ? instantOf(a) : null, typeof b === "string" ? instantOf(b) : null];
      if (x && y) return x.sec !== y.sec ? (x.sec < y.sec ? -1 : 1) : x.frac < y.frac ? -1 : x.frac > y.frac ? 1 : 0;
      break;
    }
  }
  const text = (v: unknown) => (typeof v === "string" ? v : JSON.stringify(v));
  return collator.compare(text(a), text(b));
}
