// @name 集計表を作る
// @desc ある列の値ごとに、別の列の合計と件数を求めて、集計用の表に書き出します（実行のたびに作り直し）。
// 元の表と、集計の軸にする列・合計する列。集計先の表には「区分」「合計」「件数」の列を作っておいてください。
const SHEET = "シート1";
const SOURCE = "データ";
const GROUP_BY = "列1";
const SUM = "列2";
const SUMMARY = "集計"; // 集計先の表（同じシート内。列: 区分 / 合計 / 件数）

export default function (jx: Jxcel) {
  const sheet = jx.sheet(SHEET);
  const rows = sheet.schema(SOURCE).rows();
  const out = sheet.schema(SUMMARY);

  // いったん空にしてから書き出す
  for (const old of out.rows()) out.remove(old._id);

  const groups = std.groupBy(rows, GROUP_BY);
  for (const g of std.sortBy(groups.map((g) => ({ 区分: g.key, 合計: std.sumBy(g.rows, SUM), 件数: g.rows.length })), "区分")) {
    out.add(g);
  }
  jx.log(`${groups.length} 区分を集計しました`);
  return groups.length;
}
