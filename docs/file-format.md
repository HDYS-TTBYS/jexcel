# ファイル形式

`.jxcel` は zip。中身は決定的に出力される（同じ内容なら同じバイト列）。

```
manifest.json                                   形式バージョン・シート一覧・マクロ・書き出し・フォームの定義
sheets/<sheetId>/sheet.json                     シート名・スキーマ定義・行の並び(rowOrder)
sheets/<sheetId>/<schemaId>.rows.jsonl          1 行 1 JSON。行は ID(ULID) 順
sheets/<sheetId>/computed/<schemaId>.<columnId>.ts   計算列の式
macros/<id>.ts                                  マクロのソース
exports/<id>.xlsx|docx                          書き出しテンプレート（元のバイト列のまま）
history/HEAD, config, objects/**, refs/**       内蔵 git の履歴（ベアリポジトリ）
```

- 行・列・シートは安定 ID で突合する。行の並びは `rowOrder` に分けてあり、並べ替えで行本体は変わらない。
- 計算列の値は保存しない。Decimal は精度のため文字列で持つ。`null` は型に関わらず許容し、必須は列の設定で判定する。
- 履歴は 1 つのパックにまとめて詰める。`history/` を無視すれば現在の状態だけ読める。履歴のない素の zip も開ける（初回保存で履歴が始まる）。
- 永続化形式を非互換に変えたら `FORMAT_VERSION`（`crates/jxcel-core/src/lib.rs`）を上げる。旧形式のファイルは開ける。

詳細な設計の理由は [CLAUDE.md](../CLAUDE.md) を参照。
