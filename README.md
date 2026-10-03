# jxcel

JSON をファイル実体とする、**DB として運用できるスプレッドシート**。

- ファイルは zip 圧縮した JSON。**内蔵 git** が履歴・構造的な差分・復元を提供する（利用者が git を意識する必要はない）。
- File → Sheet → データスキーマ(1:N) → 行。スキーマはネストでき、列には型（文字列・整数・小数・Decimal・真偽・日付・日時・列挙・オブジェクト・配列）がある。
- 計算は独自の式言語ではなく TypeScript。**マクロ**と**計算列**、標準ライブラリ `std`（集計・日付・Decimal・テキスト）を備える。
- xlsx / docx を**テンプレート**にして、行ごとにファイルを書き出せる（行ループ・入れ子ループ対応）。
- 同じネットワークのブラウザから回答を集める **LAN フォーム配信**。

Tauri 2 + Rust + React / TypeScript（グリッドは AG Grid）で作っている。

## インストール

[Releases](https://github.com/HDYS-TTBYS/jexcel/releases) から OS 別のインストーラを入手する（手順と初回起動時の警告は [docs/install.md](docs/install.md)）。

## ソースから動かす

必要なもの: Rust（stable）、Node.js 22、pnpm 10。Linux は `libwebkit2gtk-4.1-dev` など（[docs/development.md](docs/development.md)）。

```
pnpm --dir ui install
pnpm tauri dev                 # アプリを起動
pnpm --dir ui dev              # ブラウザ単体（モックバックエンド）。http://localhost:1420
cargo test --workspace         # Rust のテスト
pnpm --dir ui test             # UI のテスト
```

## ドキュメント

| | |
|---|---|
| [docs/install.md](docs/install.md) | インストールと初回起動 |
| [docs/guide.md](docs/guide.md) | 使い方（データ・履歴・マクロ・計算列・書き出し・フォーム） |
| [docs/file-format.md](docs/file-format.md) | ファイル形式 |
| [docs/development.md](docs/development.md) | 開発・テスト・構成 |
| [docs/releasing.md](docs/releasing.md) | リリース手順（CD） |
| [CLAUDE.md](CLAUDE.md) | 設計上の要点の詳細（実装者向け） |
| [crates/jxcel-export/verification/README.md](crates/jxcel-export/verification/README.md) | Excel・Word・スマートフォンでの確認手順 |

## 状態

書き出した docx / xlsx は LibreOffice・python-docx・openpyxl と OOXML スキーマ検証で確認済み。Excel・Word の実機とスマートフォンのブラウザでの確認は未実施。
