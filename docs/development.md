# 開発

## 構成

```
ui/ (React) ⇄ Tauri IPC ⇄ src-tauri (薄いコマンド層) → jxcel-app → jxcel-git / jxcel-macro → jxcel-core
                                                              └→ jxcel-export (xlsx / docx)
```

| クレート | 役割 |
|---|---|
| `jxcel-core` | データモデル・型検証・永続化(展開ツリー)・構造差分 |
| `jxcel-git` | 内蔵 git の履歴と、zip への同梱 |
| `jxcel-macro` | TS マクロ・計算列・`std`（oxc + QuickJS） |
| `jxcel-export` | xlsx / docx テンプレートの書き出し |
| `jxcel-app` | `Session`（編集・保存・履歴）とフォーム配信 |

`src-tauri` は WebKit 等に依存するため、ワークスペースから外した独立のルート。

## コマンド

```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt
pnpm --dir ui test             # Vitest
pnpm --dir ui e2e              # Playwright（ブラウザ単体）
xvfb-run -a pnpm --dir ui e2e:tauri   # Tauri の実ウィンドウ（Linux。先に pnpm tauri build --debug --no-bundle）
pnpm tauri dev                 # ルートから実行する
pnpm tauri build --no-bundle   # 単一実行ファイル
```

Linux で `src-tauri` を作るには `libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev librsvg2-dev patchelf` が要る。

## 決まり

- コマンドを増減したら `src-tauri/src/lib.rs`・`tauriBackend.ts`・`mockBackend.ts`・`types.ts` を揃える。
- `prelude.js` / `std.js` を変えたら `ui/src/jxcelApi.d.ts.txt` も直す。
- 出力の決定性(同じ内容 → 同じバイト列)を壊さない。
- CI は `.github/workflows/ci.yml`。リリースは [releasing.md](releasing.md)。

設計の細部は [CLAUDE.md](../CLAUDE.md)。
