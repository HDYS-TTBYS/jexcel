import { describe, expect, it } from "vitest";
import { datetimeToOffset, parseOffset } from "./datetime";

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
