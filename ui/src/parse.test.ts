import { describe, expect, it } from "vitest";
import { formatValue, parseInput } from "./parse";

const p = (t: Parameters<typeof parseInput>[0], s: string) => parseInput(t, s);

describe("parseInput", () => {
  it("空入力は null", () => {
    expect(p({ kind: "int" }, "  ")).toEqual({ ok: true, value: null });
  });
  it("数値型", () => {
    expect(p({ kind: "int" }, "12")).toEqual({ ok: true, value: 12 });
    expect(p({ kind: "int" }, "1.5").ok).toBe(false);
    expect(p({ kind: "float" }, "1.5")).toEqual({ ok: true, value: 1.5 });
    expect(p({ kind: "float" }, "abc").ok).toBe(false);
  });
  it("decimal は精度保持のため文字列", () => {
    expect(p({ kind: "decimal" }, "12.50")).toEqual({ ok: true, value: "12.50" });
    expect(p({ kind: "decimal" }, "12.").ok).toBe(false);
  });
  it("bool / date / enum", () => {
    expect(p({ kind: "bool" }, "TRUE")).toEqual({ ok: true, value: true });
    expect(p({ kind: "bool" }, "no")).toEqual({ ok: true, value: false });
    expect(p({ kind: "bool" }, "maybe").ok).toBe(false);
    expect(p({ kind: "date" }, "2024-01-02").ok).toBe(true);
    expect(p({ kind: "date" }, "2024/01/02").ok).toBe(false);
    expect(p({ kind: "enum", values: ["a", "b"] }, "a").ok).toBe(true);
    expect(p({ kind: "enum", values: ["a", "b"] }, "c").ok).toBe(false);
  });
  it("配列/オブジェクトは JSON、ANY は JSON でなければ文字列", () => {
    expect(p({ kind: "array", item: { kind: "int" } }, "[1,2]")).toEqual({ ok: true, value: [1, 2] });
    expect(p({ kind: "array", item: { kind: "int" } }, "[1,").ok).toBe(false);
    expect(p({ kind: "any" }, '{"a":1}')).toEqual({ ok: true, value: { a: 1 } });
    expect(p({ kind: "any" }, "hello")).toEqual({ ok: true, value: "hello" });
  });
  it("string は前後の空白を保持", () => {
    expect(p({ kind: "string" }, " x ")).toEqual({ ok: true, value: " x " });
  });
});

describe("formatValue", () => {
  it("型ごとに文字列化", () => {
    expect(formatValue(null)).toBe("");
    expect(formatValue(3)).toBe("3");
    expect(formatValue(false)).toBe("false");
    expect(formatValue({ a: [1] })).toBe('{"a":[1]}');
  });
});
