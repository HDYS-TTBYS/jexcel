import { describe, expect, it } from "vitest";
import type { Column, DataType } from "./types";
import { convertValue, datetimeToOffset, hasDatetime, parseOffset } from "./datetime";

const c = (s: string, off: string) => datetimeToOffset(s, parseOffset(off)!, /^z$/i.test(off));

describe("datetimeToOffset（Rust 側と同じ結果になる）", () => {
  it("同じ時刻を別のオフセットで表す", () => {
    expect(c("2026-10-02T01:30:00Z", "+09:00")).toBe("2026-10-02T10:30:00+09:00");
    expect(c("2026-10-02T20:00:00Z", "+09:00")).toBe("2026-10-03T05:00:00+09:00");
    expect(c("2024-03-01T00:30:00+09:00", "Z")).toBe("2024-02-29T15:30:00Z");
    expect(c("2024-03-01T00:30:00+09:00", "+00:00")).toBe("2024-02-29T15:30:00+00:00");
    expect(c("2024-01-01T00:00:00.250+09:00", "-05:30")).toBe("2023-12-31T09:30:00.250-05:30");
    expect(c("2026-10-02t01:30:00z", "+09:00")).toBe("2026-10-02T10:30:00+09:00");
  });
  it("変換できないものは null", () => {
    for (const bad of ["2026-10-02", "2026-10-02T23:59:60Z", "2026-02-30T00:00:00Z", "abc", "2026-10-02T10:30:00"]) {
      expect(c(bad, "+09:00"), bad).toBeNull();
    }
  });
  it("オフセットの指定", () => {
    expect([parseOffset("Z"), parseOffset(" -03:30 "), parseOffset("+09:00")]).toEqual([0, -210, 540]);
    for (const bad of ["", "9", "+9:00", "+24:00", "+09:60", "JST", "+0900"]) expect(parseOffset(bad), bad).toBeNull();
  });
});

describe("ネストした列の一括変換（Rust 側の convert_datetime_offsets_inside_nested_columns と同じ結果）", () => {
  const col = (id: string, type: DataType): Column => ({ id, name: id, type });
  const event: DataType = { kind: "object", fields: [col("f_at", { kind: "dateTime" }), col("f_memo", { kind: "string" })] };
  const log: DataType = { kind: "array", item: { kind: "object", fields: [col("f_t", { kind: "dateTime" })] } };
  const times: DataType = { kind: "array", item: { kind: "dateTime" } };

  it("日時を含む型か", () => {
    expect([event, log, times, { kind: "string" } as DataType, { kind: "array", item: { kind: "int" } } as DataType].map(hasDatetime)).toEqual([
      true,
      true,
      true,
      false,
      false,
    ]);
  });

  it("オブジェクト・配列の中の日時だけを変換し、文字列などは触らない", () => {
    const counts = { converted: 0, unchanged: 0, skipped: 0 };
    const run = (t: DataType, v: unknown) => convertValue(t, v, 540, false, counts);
    const ev = { f_at: "2026-10-02T01:30:00Z", f_memo: "2026-10-02T01:30:00Z" };
    expect(run(event, ev)).toEqual({ f_at: "2026-10-02T10:30:00+09:00", f_memo: "2026-10-02T01:30:00Z" });
    expect(ev.f_at).toBe("2026-10-02T01:30:00Z"); // 元の値は書き換えない
    expect(run(log, [{ f_t: "2026-10-02T23:00:00Z" }, { f_t: "2026-10-03T08:00:00+09:00" }, {}])).toEqual([
      { f_t: "2026-10-03T08:00:00+09:00" },
      { f_t: "2026-10-03T08:00:00+09:00" },
      {},
    ]);
    expect(run(times, ["2026-10-02T23:59:60Z", "2026-12-31T15:00:00Z"])).toEqual(["2026-10-02T23:59:60Z", "2027-01-01T00:00:00+09:00"]);
    expect(counts).toEqual({ converted: 3, unchanged: 1, skipped: 1 });
  });
});
