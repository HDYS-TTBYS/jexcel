# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

jxcel: JSON をファイル実体とする、DB として運用できるスプレッドシート（Tauri + Rust + TypeScript）。ファイルは zip 圧縮された JSON で、内蔵 git により履歴・構造的差分・復元を提供する。File → Sheet → データスキーマ(1:N) → 行 の階層で、スキーマはネスト可能。計算は独自式言語ではなく TS マクロに統合する方針（未実装）。

MVP の範囲は「コア + 履歴」。マクロ/LSP、xlsx・docx テンプレート書き出し、LAN フォーム配信は後続マイルストーン。

## Commands

```
cargo test --workspace                 # 全テスト
cargo test -p jxcel-core diff::        # 単一モジュール/テストの絞り込み（名前の部分一致）
cargo clippy --workspace --all-targets
cargo fmt
```

`src-tauri/` と `ui/` は未作成。`src-tauri` は WebKit 等のシステム依存を要するため、ルートの Cargo workspace から `exclude` している（独立したルートとしてビルドする）。

## Architecture

- `crates/jxcel-core` — UI/Tauri 非依存。データモデル(`model`)、型と検証(`types`)、永続化(`tree`)、構造差分(`diff`)。
- `crates/jxcel-git` — `jxcel-core` の展開ツリーを内蔵 git（ベアリポジトリ、`git2`）にコミットする `History`と、履歴を zip に内包する `Archive`。

複数ファイルにまたがる設計上の要点:

- **永続化は「展開ツリー」が中心**。`JxcelFile::to_tree()` が `manifest.json` / `sheets/<id>/sheet.json` / `sheets/<id>/<schemaId>.rows.jsonl` のパス→バイト列を作り、zip 化(`to_zip`)も git コミット(`History::commit`)も同じツリーを使う。だから git の差分が行単位で意味を持つ。出力は決定的（キー順固定、行は ID 順、zip タイムスタンプ固定）で、この性質はテストで固定されている。壊さないこと。
- **履歴は zip に内包する**（`jxcel-git/src/archive.rs`）。zip には現在の状態（`manifest.json`, `sheets/**`）と履歴のベアリポジトリ（`history/HEAD`, `history/objects/**`, `history/refs/**`）が同居する。`Archive::open` が `history/` を一時ディレクトリに展開して `History` を開き、`Archive::save` がコミットしてから詰め直す。`jxcel-core` の `from_zip` は `history/` を無視するので、履歴なしでも現在の状態は読める。履歴のない素の zip も開ける（初回保存で履歴が始まる）。内容が同じなら保存結果のバイト列も同じ。
- 行の並びは `sheet.json` の `rowOrder` に分離している。並べ替えで行本体のファイルが変わらないようにするため。
- 突合はすべて安定 ID（行は `rowId`(ULID)、列は `columnId`）。列の表示名は変更可、ID は不変。`diff` もこの ID で行・列・セル単位の `Change` を返す。
- `DataType::Custom` + `TypeRegistry` は将来の JS 型拡張の差し込み口。未登録のカスタム型は検証エラーになる。
- `Decimal` は精度保持のため文字列で保持する。`Null` は型に関わらず許容し、必須は `Column.required` で判定する。
- `FORMAT_VERSION`（`jxcel-core/src/lib.rs`）は永続化形式を非互換に変えたときに上げる。
