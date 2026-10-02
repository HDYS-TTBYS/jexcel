import type { DataType } from "./types";

/** RFC 3339 の日時を、同じ時刻のまま別のオフセットの表記に直す（Rust の `jxcel_core::types::datetime_to_offset` と同じ規則）。
 *  変換できない文字列（日時でない・うるう秒・範囲外）は null。 */
export function parseOffset(s: string): number | null {
  const t = s.trim();
  if (/^z$/i.test(t)) return 0;
  const m = /^([+-])(\d{2}):(\d{2})$/.exec(t);
  if (!m || Number(m[2]) > 23 || Number(m[3]) > 59) return null;
  return (m[1] === "-" ? -1 : 1) * (Number(m[2]) * 60 + Number(m[3]));
}

const toDays = (y: number, m: number, d: number) => {
  const yy = y - (m <= 2 ? 1 : 0);
  const era = Math.floor(yy / 400);
  const yoe = yy - era * 400;
  const doy = Math.floor((153 * (m + (m > 2 ? -3 : 9)) + 2) / 5) + d - 1;
  return era * 146097 + yoe * 365 + Math.floor(yoe / 4) - Math.floor(yoe / 100) + doy - 719468;
};
const fromDays = (z0: number): [number, number, number] => {
  const z = z0 + 719468;
  const era = Math.floor(z / 146097);
  const doe = z - era * 146097;
  const yoe = Math.floor((doe - Math.floor(doe / 1460) + Math.floor(doe / 36524) - Math.floor(doe / 146096)) / 365);
  const doy = doe - (365 * yoe + Math.floor(yoe / 4) - Math.floor(yoe / 100));
  const mp = Math.floor((5 * doy + 2) / 153);
  const d = doy - Math.floor((153 * mp + 2) / 5) + 1;
  const m = mp + (mp < 10 ? 3 : -9);
  return [yoe + era * 400 + (m <= 2 ? 1 : 0), m, d];
};
const p2 = (n: number) => String(n).padStart(2, "0");

export function datetimeToOffset(s: string, offsetMin: number, zulu: boolean): string | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})[Tt](\d{2}):(\d{2}):(\d{2})(\.\d+)?(Z|z|[+-]\d{2}:\d{2})$/.exec(s);
  if (!m) return null;
  const [y, mo, d, hh, mi, ss] = [1, 2, 3, 4, 5, 6].map((i) => Number(m[i]));
  const from = parseOffset(m[8]);
  if (from === null || mo < 1 || mo > 12 || d < 1 || d > 31 || hh > 23 || mi > 59 || ss > 59) return null;
  // 存在しない日付（2 月 30 日など）は変換しない
  const [cy, cm, cd] = fromDays(toDays(y, mo, d));
  if (cy !== y || cm !== mo || cd !== d) return null;
  const secs = toDays(y, mo, d) * 86400 + hh * 3600 + mi * 60 + ss - from * 60 + offsetMin * 60;
  const day = Math.floor(secs / 86400);
  const r = secs - day * 86400;
  const [ny, nm, nd] = fromDays(day);
  if (ny < 0 || ny > 9999) return null;
  const a = Math.abs(offsetMin);
  const off = offsetMin === 0 && zulu ? "Z" : `${offsetMin < 0 ? "-" : "+"}${p2(Math.floor(a / 60))}:${p2(a % 60)}`;
  return `${String(ny).padStart(4, "0")}-${p2(nm)}-${p2(nd)}T${p2(Math.floor(r / 3600))}:${p2(Math.floor((r % 3600) / 60))}:${p2(r % 60)}${m[7] ?? ""}${off}`;
}

/** 型の中に日時型があるか（オブジェクトのフィールド・配列の要素の中も見る。Rust の `has_datetime` と同じ）。 */
export function hasDatetime(ty: DataType): boolean {
  switch (ty.kind) {
    case "dateTime":
      return true;
    case "object":
      return ty.fields.some((f) => hasDatetime(f.type));
    case "array":
      return hasDatetime(ty.item);
    default:
      return false;
  }
}

export interface ConvertCounts {
  converted: number;
  unchanged: number;
  skipped: number;
}

/** 型に沿って値をたどり、日時の文字列を別のオフセットの表記に直した値を返す（型に合わない値は触らない。Rust の `convert_value` と同じ）。 */
export function convertValue(ty: DataType, v: unknown, offsetMin: number, zulu: boolean, c: ConvertCounts): unknown {
  if (ty.kind === "dateTime" && typeof v === "string") {
    const n = datetimeToOffset(v, offsetMin, zulu);
    if (n === null) c.skipped++;
    else if (n === v) c.unchanged++;
    else {
      c.converted++;
      return n;
    }
    return v;
  }
  if (ty.kind === "object" && v !== null && typeof v === "object" && !Array.isArray(v)) {
    const o = { ...(v as Record<string, unknown>) };
    for (const f of ty.fields) if (f.id in o) o[f.id] = convertValue(f.type, o[f.id], offsetMin, zulu, c);
    return o;
  }
  if (ty.kind === "array" && Array.isArray(v)) return v.map((x) => convertValue(ty.item, x, offsetMin, zulu, c));
  return v;
}

/** オフセットを考慮した「瞬間」（UTC の秒と、小数秒の 9 桁）。日時として読めない文字列は null。 */
export function instantOf(s: string): { sec: number; frac: string } | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})[Tt](\d{2}):(\d{2}):(\d{2})(\.\d+)?(Z|z|[+-]\d{2}:\d{2})$/.exec(s);
  if (!m) return null;
  const [y, mo, d, hh, mi, ss] = [1, 2, 3, 4, 5, 6].map((i) => Number(m[i]));
  const off = parseOffset(m[8]);
  if (off === null || mo < 1 || mo > 12 || d < 1 || d > 31 || hh > 23 || mi > 59 || ss > 60) return null;
  const [cy, cm, cd] = fromDays(toDays(y, mo, d));
  if (cy !== y || cm !== mo || cd !== d) return null;
  return { sec: toDays(y, mo, d) * 86400 + hh * 3600 + mi * 60 + ss - off * 60, frac: ((m[7] ?? ".").slice(1) + "000000000").slice(0, 9) };
}
