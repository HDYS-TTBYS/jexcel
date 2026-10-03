# インストール

[Releases](https://github.com/HDYS-TTBYS/jexcel/releases) の最新版から、使う OS の成果物を入手する。

| OS | 成果物 |
|---|---|
| Windows | `jxcel_*_x64-setup.exe`（NSIS）または `*.msi` |
| macOS | `jxcel_*.dmg` |
| Linux | `*.AppImage`（`chmod +x` して実行）または `*.deb` |

## 初回起動の警告

現状は**コード署名をしていない**ため、OS が警告を出す。

- Windows: SmartScreen に「WindowsによってPCが保護されました」と出たら「詳細情報」→「実行」。
- macOS: 「開発元を検証できません」と出たら、システム設定 → プライバシーとセキュリティ → 「このまま開く」。または `xattr -dr com.apple.quarantine /Applications/jxcel.app`。
- Linux: 警告なし。AppImage は FUSE が必要な環境がある。

## フォーム配信とファイアウォール

LAN フォーム配信は、使うときだけ `0.0.0.0` で待ち受ける。OS のファイアウォールの確認が出たら、プライベートネットワークでの受信を許可する。
