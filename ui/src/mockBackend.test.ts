import { describe, expect, it } from "vitest";
import { createMockBackend } from "./mockBackend";
import { describeChange } from "./describe";

describe("mock backend + describeChange", () => {
  it("編集→保存→差分→復元", async () => {
    const b = createMockBackend();
    let s = await b.newFile("台帳");
    expect(s.dirty).toBe(false); // 新規直後は変更なし。編集で変更ありになる
    const sheet = s.file.sheets[0];
    const schema = sheet.schemas[0];
    const col = schema.columns[0];
    const row = schema.rows[0];

    s = await b.updateColumn(sheet.id, schema.id, { ...col, name: "数量", type: { kind: "int" } });
    await expect(b.setCell(sheet.id, schema.id, row.id, col.id, "x")).rejects.toBeTruthy();
    await b.setCell(sheet.id, schema.id, row.id, col.id, 1);
    await b.saveFile("/mock/a.jxcel", "初回");
    s = await b.setCell(sheet.id, schema.id, row.id, col.id, 2);
    expect(s.dirty).toBe(true);
    s = await b.saveFile(null, "数量変更");
    expect(s.dirty).toBe(false);

    const log = await b.historyLog();
    expect(log.map((c) => c.message)).toEqual(["数量変更", "初回"]);
    const changes = await b.historyDiff(log[1].id, log[0].id);
    expect(changes).toHaveLength(1);
    expect(describeChange(changes[0], s.file)).toContain("「数量」: 「1」 → 「2」");

    s = await b.restore(log[1].id);
    expect(s.file.sheets[0].schemas[0].rows[0].cells[col.id]).toBe(1);
    expect(s.dirty).toBe(true);
  });
});
