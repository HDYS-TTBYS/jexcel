// @name 空欄を上の行の値で埋める
// @desc 指定した列で、空のセルを直前の空でないセルの値で埋めます（結合セルを解いたデータの整理に）。
// 対象の表と列（自分のファイルに合わせて書き換えてください）
const SHEET = "シート1";
const SCHEMA = "データ";
const COLUMN = "列1";

export default function (jx: Jxcel) {
  const schema = jx.sheet(SHEET).schema(SCHEMA);
  let last: unknown = null;
  let filled = 0;
  for (const row of schema.rows()) {
    if (std.text.isBlank(row[COLUMN])) {
      if (last !== null) {
        schema.update(row._id, { [COLUMN]: last });
        filled++;
      }
    } else {
      last = row[COLUMN];
    }
  }
  jx.log(`${filled} 件のセルを埋めました`);
  return filled;
}
