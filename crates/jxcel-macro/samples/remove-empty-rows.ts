// @name 空の行を削除
// @desc すべての列が空（または空白だけ）の行を削除します。
// 対象の表（自分のファイルに合わせて書き換えてください）
const SHEET = "シート1";
const SCHEMA = "データ";

export default function (jx: Jxcel) {
  const schema = jx.sheet(SHEET).schema(SCHEMA);
  // 計算列の値は元データから決まるので、判定には使わない
  const columns = schema.columns.filter((c) => !c.computed).map((c) => c.name);
  let removed = 0;
  for (const row of schema.rows()) {
    if (columns.every((c) => std.text.isBlank(row[c]))) {
      schema.remove(row._id);
      removed++;
    }
  }
  jx.log(`${removed} 行を削除しました`);
  return removed;
}
