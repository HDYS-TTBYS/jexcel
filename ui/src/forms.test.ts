import { describe, expect, it } from "vitest";
import { canBeField } from "./forms";
import type { Column } from "./types";

const col = (type: Column["type"], extra: Partial<Column> = {}): Column => ({ id: "c", name: "c", type, ...extra });

describe("canBeField", () => {
  it("accepts scalar columns", () => {
    for (const kind of ["string", "int", "float", "decimal", "bool", "date", "dateTime"] as const) {
      expect(canBeField(col({ kind }))).toBe(true);
    }
    expect(canBeField(col({ kind: "enum", values: ["a"] }))).toBe(true);
  });
  it("rejects computed and nested columns", () => {
    expect(canBeField(col({ kind: "int" }, { computed: {} }))).toBe(false);
    expect(canBeField(col({ kind: "object", fields: [] }))).toBe(false);
    expect(canBeField(col({ kind: "array", item: { kind: "int" } }))).toBe(false);
    expect(canBeField(col({ kind: "any" }))).toBe(false);
    expect(canBeField(col({ kind: "custom", name: "x" }))).toBe(false);
  });
});
