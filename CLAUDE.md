# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

jxcel: JSON をファイル実体とする、DB として運用できるスプレッドシート（Tauri + Rust + TypeScript）。ファイルは zip 圧縮された JSON で、内蔵 git により履歴・構造的差分・復元を提供する。File → Sheet → データスキーマ(1:N) → 行 の階層で、スキーマはネスト可能。計算は独自式言語ではなく TS マクロに統合する方針（未実装）。

MVP の範囲は「コア + 履歴」。マクロ/LSP、xlsx・docx テンプレート書き出し、LAN フォーム配信は後続マイルストーン。

## Commands

Rust（ルートの Cargo workspace: `jxcel-core` / `jxcel-git` / `jxcel-app`）:

```
cargo test --workspace                 # 全テスト
cargo test -p jxcel-core diff::        # 単一モジュール/テストの絞り込み（名前の部分一致）
cargo clippy --workspace --all-targets
cargo fmt
```

UI（`ui/`: Vite + React + TS + AG Grid。`pnpm --dir ui <cmd>` またはルートの `pnpm ui <cmd>`）:

```
pnpm --dir ui install
pnpm --dir ui dev            # ブラウザ単体で起動（モックバックエンド。http://localhost:1420）
pnpm --dir ui test           # Vitest
pnpm --dir ui test -- parse  # 単一ファイルの絞り込み
pnpm --dir ui build          # tsc + vite build（src-tauri が ui/dist を埋め込む）
```

Tauri アプリ（`src-tauri/`。**必ずリポジトリルートから**実行する。CLI は cwd 配下の `tauri.conf.json` を探す）:

```
pnpm tauri dev               # UI の dev サーバを自動起動してアプリを開く
pnpm tauri build --no-bundle # 単一実行ファイルのみ生成
cd src-tauri && cargo check  # Rust 側だけ確認（ui/dist が必要）
```

`src-tauri` は WebKit 等のシステム依存（Linux: `libwebkit2gtk-4.1-dev` など）を要するため、ルートの workspace から `exclude` した独立ルート（独自の `Cargo.lock`）。`cargo test --workspace` はこれを含まない。

## Architecture

構成: `ui/`（React）⇄ Tauri IPC ⇄ `src-tauri`（薄いコマンド層）→ `jxcel-app` → `jxcel-git` → `jxcel-core`。

- `crates/jxcel-core` — UI/Tauri 非依存。データモデル(`model`)、型と検証(`types`)、永続化(`tree`)、構造差分(`diff`)。
- `crates/jxcel-git` — `jxcel-core` の展開ツリーを内蔵 git（ベアリポジトリ、`git2`）にコミットする `History`と、履歴を zip に内包する `Archive`。

複数ファイルにまたがる設計上の要点:

- **永続化は「展開ツリー」が中心**。`JxcelFile::to_tree()` が `manifest.json` / `sheets/<id>/sheet.json` / `sheets/<id>/<schemaId>.rows.jsonl` のパス→バイト列を作り、zip 化(`to_zip`)も git コミット(`History::commit`)も同じツリーを使う。だから git の差分が行単位で意味を持つ。出力は決定的（キー順固定、行は ID 順、zip タイムスタンプ固定）で、この性質はテストで固定されている。壊さないこと。
- **履歴は zip に内包する**（`jxcel-git/src/archive.rs`）。zip には現在の状態（`manifest.json`, `sheets/**`）と履歴のベアリポジトリ（`history/HEAD`, `history/objects/**`, `history/refs/**`）が同居する。`Archive::open` が `history/` を一時ディレクトリに展開して `History` を開き、`Archive::save` がコミットしてから詰め直す。`jxcel-core` の `from_zip` は `history/` を無視するので、履歴なしでも現在の状態は読める。履歴のない素の zip も開ける（初回保存で履歴が始まる）。内容が同じなら保存結果のバイト列も同じ。
- `crates/jxcel-app` — Tauri 非依存の操作ロジック `Session`（開いているファイル、保存先、未保存フラグ、編集、履歴操作）。`src-tauri/src/lib.rs` はこれを呼ぶだけなので、ロジックは `cargo test` で検証できる。編集は「複製に適用→成功したら差し替え」で、失敗した編集は状態を残さない。セル編集と列定義の変更は型検証を通らなければ拒否される。`restore` は履歴の内容を未保存状態として読み込み、保存して初めて新しいコミットになる。
- UI は `Backend` インターフェース（`ui/src/backend.ts`）越しにバックエンドを呼ぶ。実装は 2 つ: `tauriBackend.ts`（`invoke`）と `mockBackend.ts`（ブラウザ単体用の in-memory。Rust の挙動の簡易再現で、永続化しない）。**コマンドを増減したら `src-tauri/src/lib.rs`、`tauriBackend.ts`、`mockBackend.ts`、`types.ts`（serde 表現の鏡）を揃える。** `DataType` は `{kind: "dateTime", ...}` のように serde の camelCase タグで往復する。
- **未保存警告**: `App.tsx` の `guard()` が「新規」「開く」「ウィンドウを閉じる」の前に、未保存（`snap.dirty`）なら保存して続行/破棄して続行/キャンセルを確認する。閉じる操作は `Backend.onCloseRequested` で横取りし（Tauri は `preventDefault`）、確認後は `closeWindow()`＝`destroy()` で閉じる（`close()` だとハンドラが再発火する）。`destroy` には capability `core:window:allow-destroy` が必要。モックは `window.__requestClose()` / `window.__closed` でブラウザ上から検証できる。
- グリッドは AG Grid の `readOnlyEdit` で使う。グリッド自身は値を書き換えず、編集要求（`onCellEditRequest`）をバックエンドに送り、検証を通った結果のスナップショットで再描画する。
- 行の並びは `sheet.json` の `rowOrder` に分離している。並べ替えで行本体のファイルが変わらないようにするため。
- 突合はすべて安定 ID（行は `rowId`(ULID)、列は `columnId`）。列の表示名は変更可、ID は不変。`diff` もこの ID で行・列・セル単位の `Change` を返す。
- `DataType::Custom` + `TypeRegistry` は将来の JS 型拡張の差し込み口。未登録のカスタム型は検証エラーになる。
- `Decimal` は精度保持のため文字列で保持する。`Null` は型に関わらず許容し、必須は `Column.required` で判定する。
- `FORMAT_VERSION`（`jxcel-core/src/lib.rs`）は永続化形式を非互換に変えたときに上げる。
