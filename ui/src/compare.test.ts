import { describe, expect, it } from "vitest";
import { compareDecimal, compareValues } from "./compare";

const dt = { kind: "dateTime" } as const;

describe("compareValues", () => {
  it("オフセットが違う日時は瞬間で比べる", () => {
    // 文字列では "10:00+09:00" < "02:00Z" だが、瞬間では 01:00Z なので後ではなく前
    expect(compareValues(dt, "2024-01-31T10:00:00+09:00", "2024-01-31T02:00:00Z")).toBe(-1);
    expect(compareValues(dt, "2024-01-31T10:00:00+09:00", "2024-01-31T01:00:00Z")).toBe(0);
    expect(compareValues(dt, "2024-01-31T10:00:00+09:00", "2024-01-30T23:59:59-05:00")).toBe(-1);
    expect(compareValues(dt, "2024-01-31T10:00:00.5+09:00", "2024-01-31T10:00:00.25+09:00")).toBe(1);
    // 並べ替えの結果
    const v = ["2024-01-31T10:00:00+09:00", "2024-01-31T00:30:00Z", "2024-01-31T01:00:00Z", "2024-01-30T23:00:00-05:00"];
    expect([...v].sort((a, b) => compareValues(dt, a, b))).toEqual(["2024-01-31T00:30:00Z", "2024-01-31T10:00:00+09:00", "2024-01-31T01:00:00Z", "2024-01-30T23:00:00-05:00"]);
  });
  it("日時として読めない値は文字列の順にする", () => {
    expect(compareValues(dt, "abc", "abd")).toBeLessThan(0);
    expect(compareValues(dt, "2024-01-31T10:00:00Z", "zzz")).toBeLessThan(0);
  });
  it("数値は数として比べる（文字列の順にしない）", () => {
    expect(compareValues({ kind: "int" }, 10, 9)).toBe(1);
    expect(compareValues({ kind: "float" }, -1.5, 2)).toBe(-1);
  });
  it("10 進数は誤差なく比べる", () => {
    expect(compareDecimal("10", "9")).toBe(1);
    expect(compareDecimal("-10", "-9")).toBe(-1);
    expect(compareDecimal("0.10", "0.1")).toBe(0);
    expect(compareDecimal("-0.0", "0")).toBe(0);
    expect(compareDecimal("12345678901234567890.1", "12345678901234567890.01")).toBe(1);
    expect(compareDecimal("-1", "1")).toBe(-1);
    expect(compareDecimal("007", "7")).toBe(0);
  });
  it("真偽値は false が先", () => {
    expect(compareValues({ kind: "bool" }, false, true)).toBe(-1);
  });
});
