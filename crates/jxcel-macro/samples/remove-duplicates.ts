// @name 重複行の削除
// @desc 指定した列の組が同じ行のうち、最初の 1 行だけを残して削除します。
// 対象の表と、重複の判定に使う列（自分のファイルに合わせて書き換えてください）
const SHEET = "シート1";
const SCHEMA = "データ";
const KEYS = ["列1"];

export default function (jx: Jxcel) {
  const schema = jx.sheet(SHEET).schema(SCHEMA);
  const seen = new Set<string>();
  let removed = 0;
  for (const row of schema.rows()) {
    const key = JSON.stringify(KEYS.map((k) => row[k]));
    if (seen.has(key)) {
      schema.remove(row._id);
      removed++;
    } else {
      seen.add(key);
    }
  }
  jx.log(`${removed} 行を削除しました`);
  return removed;
}
