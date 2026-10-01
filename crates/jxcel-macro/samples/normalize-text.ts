// @name 表記ゆれの整理
// @desc 文字列の列すべてで、全角英数を半角にし、半角カナを全角にし、前後・連続する空白を整えます。
// 対象の表（自分のファイルに合わせて書き換えてください）
const SHEET = "シート1";
const SCHEMA = "データ";

export default function (jx: Jxcel) {
  const schema = jx.sheet(SHEET).schema(SCHEMA);
  // 計算列は書き込めないので除く
  const textColumns = schema.columns.filter((c) => c.type === "string" && !c.computed).map((c) => c.name);

  let changed = 0;
  for (const row of schema.rows()) {
    const patch: Record<string, unknown> = {};
    for (const col of textColumns) {
      const before = row[col];
      if (typeof before !== "string") continue;
      const after = std.text.normalize(before);
      if (after !== before) patch[col] = after;
    }
    if (Object.keys(patch).length > 0) {
      schema.update(row._id, patch);
      changed += Object.keys(patch).length;
    }
  }
  jx.log(`${changed} 件のセルを整理しました`);
  return changed;
}
