# リリース手順（CD）

`.github/workflows/release.yml` が、`v` で始まるタグの push を契機に、3 OS のインストーラを作って **GitHub Releases に下書き**として載せる。

1. バージョンを上げる（3 か所を同じ値にする）: ルート `Cargo.toml` の `[workspace.package]`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`。`cargo check` で `Cargo.lock` も更新する。
2. コミットして push し、CI が通ることを確かめる。
3. タグを打って push する。
   ```
   git tag v0.2.0
   git push origin v0.2.0
   ```
4. ワークフローが走る。順に、(a) タグと 2 つのバージョンの一致を確認、(b) 3 OS で `cargo test --workspace`、(c) 3 OS でインストーラを作成して Releases の下書きに添付。
5. Releases の下書きを開き、変更点を書いて、**公開**する。

タグなしで「Run workflow」から手動実行すると、Releases には載せず、成果物（Artifacts）として 7 日間残す。インストーラのビルド確認に使える。

## 成果物

| OS | 形式 |
|---|---|
| Windows | NSIS（`.exe`）、MSI |
| macOS | `.dmg` |
| Linux | `.deb`、AppImage |

## 補足

- コード署名・公証・自動更新はしていない。入れるなら、`tauri-action` に署名用のシークレットを渡し、`tauri.conf.json` に updater を足す。
- 失敗したら、同じタグでやり直すのではなく、下書きを削除し、タグも消して打ち直す（`git push --delete origin v0.2.0`）。
