# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

jxcel: JSON をファイル実体とする、DB として運用できるスプレッドシート（Tauri + Rust + TypeScript）。ファイルは zip 圧縮された JSON で、内蔵 git により履歴・構造的差分・復元を提供する。File → Sheet → データスキーマ(1:N) → 行 の階層で、スキーマはネスト可能。計算は独自式言語ではなく TS に統合する方針（マクロと計算列は実装済み）。

MVP の範囲は「コア + 履歴」。その次にマクロ（実行基盤・エディタ）、計算列、標準マクロライブラリ、xlsx・docx テンプレート書き出し、LAN フォーム配信を実装した。

## Commands

Rust（ルートの Cargo workspace: `jxcel-core` / `jxcel-git` / `jxcel-macro` / `jxcel-app`）:

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
pnpm --dir ui e2e            # Playwright の e2e（vite の dev サーバを自動で起動する）
pnpm --dir ui e2e -- guard   # 単一ファイルの絞り込み
```

e2e（`ui/e2e/*.e2e.mjs`）は、ブラウザ単体（モックバックエンド）の UI を実際の Chromium で操作して確かめる。Vitest の対象から外すため `.e2e.mjs` という名前にしてある（`playwright.config.mjs` の `testMatch`）。Playwright 管理外の Chromium を使うときは `PW_CHROMIUM` に実行ファイルのパスを入れる（Playwright は 1.56 に固定していて、ブラウザのリビジョンが合う必要がある）。`form-page.e2e.mjs` は本物の Rust のサーバーを使うので、先に `cargo build -p jxcel-app --example serve_forms` が要る（CI では必須、ローカルで無ければスキップ）。CI の `e2e` ジョブが同じことを行い、失敗時は `playwright-report` を成果物に残す。Tauri の実ウィンドウ（ファイルダイアログや OS 連携）は対象外。

Tauri アプリ（`src-tauri/`。**必ずリポジトリルートから**実行する。CLI は cwd 配下の `tauri.conf.json` を探す。ルートの `tauri` スクリプトは Windows の cmd でも動くよう、`.bin/` ではなく `node …/tauri.js` で呼んでいる）:

```
pnpm tauri dev               # UI の dev サーバを自動起動してアプリを開く
pnpm tauri build --no-bundle # 単一実行ファイルのみ生成
cd src-tauri && cargo check  # Rust 側だけ確認（ui/dist が必要）
```

`src-tauri` は WebKit 等のシステム依存（Linux: `libwebkit2gtk-4.1-dev` など）を要するため、ルートの workspace から `exclude` した独立ルート（独自の `Cargo.lock`）。`cargo test --workspace` はこれを含まない。

## Architecture

構成: `ui/`（React）⇄ Tauri IPC ⇄ `src-tauri`（薄いコマンド層）→ `jxcel-app` → `jxcel-git` / `jxcel-macro` → `jxcel-core`。

- `crates/jxcel-core` — UI/Tauri 非依存。データモデル(`model`)、型と検証(`types`)、永続化(`tree`)、構造差分(`diff`)。
- `crates/jxcel-git` — `jxcel-core` の展開ツリーを内蔵 git（ベアリポジトリ、`git2`）にコミットする `History`と、履歴を zip に内包する `Archive`。

複数ファイルにまたがる設計上の要点:

- **永続化は「展開ツリー」が中心**。`JxcelFile::to_tree()` が `manifest.json` / `sheets/<id>/sheet.json` / `sheets/<id>/<schemaId>.rows.jsonl` のパス→バイト列を作り、zip 化(`to_zip`)も git コミット(`History::commit`)も同じツリーを使う。だから git の差分が行単位で意味を持つ。出力は決定的（キー順固定、行は ID 順、zip タイムスタンプ固定）で、この性質はテストで固定されている。壊さないこと。
- **履歴は zip に内包する**（`jxcel-git/src/archive.rs`）。zip には現在の状態（`manifest.json`, `sheets/**`）と履歴のベアリポジトリ（`history/HEAD`, `history/objects/**`, `history/refs/**`）が同居する。`Archive::open` が `history/` を一時ディレクトリに展開して `History` を開き、`Archive::save` がコミットし、`History::pack` で履歴を 1 つのパック（`history/objects/pack/pack-*.{pack,idx}`）にまとめてから詰め直す。パックは単一スレッドで作り、緩いオブジェクトがなければ何もしないので、内容が同じなら保存結果も同じバイト列になる（この性質を壊さないこと）。zip に詰めるのは `HEAD` / `config` / `objects` / `refs` だけ（`archive.rs` の `KEEP`）で、履歴リポジトリはテンプレートなしで初期化する。`git init` のテンプレート（hooks のサンプル等）は約 20KB あり、マシンごとに違うので入れてはいけない。**Windows 固有の制約**: libgit2 がメモリマップしている古いパックは削除できないので、`History::pack` は削除の前にリポジトリを開き直してハンドルを手放す（`Archive::save` が `&mut self` なのはこのため）。`Archive` のフィールド順（`history` を `dir` より先に宣言）も、一時ディレクトリを消す前にリポジトリを閉じるためのもの。Linux/macOS では再現しないので、変更したら CI の Windows ジョブで確認すること。緩いオブジェクトのままの旧形式を開いて保存すると、履歴を保ったままパックに移行する。`jxcel-core` の `from_zip` は `history/` を無視するので、履歴なしでも現在の状態は読める。履歴のない素の zip も開ける（初回保存で履歴が始まる）。内容が同じなら保存結果のバイト列も同じ。
- **マクロ**（`crates/jxcel-macro`）: ユーザーは `export default function (jx: Jxcel) { ... }` の形の TS モジュールを書く。`oxc` で型を剥がして JS にし（型検査はしない。エディタ側の Monaco が行う）、`rquickjs`（QuickJS）で実行する。実行環境にファイル・ネットワーク・`require` は無く、メモリ 64MB・10 秒の上限がある。**マクロはファイルの JSON コピーに対して動き、書き込みは操作ログ（add/update/remove）として記録される。実行後に Rust がそのログを本体へ再生し、触った行だけ型検証して、1 件でも不正なら全体を捨てる**（途中までの書き込みも残らない）。JS 側の読み書き API は `src/prelude.js`（`jx.sheet(名前).schema(名前)` の `rows/get/add/update/remove`。列は名前で指定、行 ID は `_id`）。**`prelude.js` を変えたら、型定義 `ui/src/jxcelApi.d.ts.txt` とモック（同じ `prelude.js` を `?raw` で再利用している）も確認すること。** マクロは `JxcelFile.macros` に持ち、永続化は `macros/<id>.ts`（ソースをそのままのバイト列で）+ manifest の `macros`（id と名前。無ければキー自体を出さない）。旧形式のファイルも開ける。差分は `MacroAdded/Removed/Renamed/Edited`。
- **標準ライブラリ `std`**（`crates/jxcel-macro/src/std.js`）: マクロにも計算列の式にも、import なしのグローバル `std` として見える（`execute` が prelude の後に読み込む）。集計（`sum/avg/min/max/median/count/round/floor/ceil/clamp/coalesce`）、表の操作（`pluck/sumBy/groupBy/sortBy/lookup/find/where/uniq`）、`std.date`（月末丸めの加減算・曜日・営業日・和暦。日付は `"YYYY-MM-DD"` 文字列で入出力し、`Date` やタイムゾーンに依存しない暦計算）、`std.dec`（Decimal を **BigInt で誤差なく**計算。結果は文字列）、`std.text`（全角半角・かな・正規化・3 桁区切り）。方針: 空(null)は無視するか null を返し、**数値でない値は黙って 0 にせず例外**（計算列では `#ERROR`）。文字列の比較は文字コード順で五十音順ではない。**関数を足したら `ui/src/jxcelApi.d.ts.txt` に JSDoc 付きの型も書くこと**（エディタの補完とホバー説明の元。Rust のテスト `std_matches_its_type_definitions` が、型定義にある関数が実在し、実在する関数が型定義にあることを検査する）。**`std` は QuickJS 付属の `std`（ファイル入出力ができるモジュール）と同名だが別物で、入出力の関数を持たない**（`sandbox_has_no_io` が `std.open/loadFile/getenv/popen` などの不在を検査している）。モックも同じ `std.js` を `?raw` で再利用する。
- **サンプルマクロ**（`crates/jxcel-macro/samples/*.ts`、`samples.rs`）: 先頭の `// @name` / `// @desc` から名前と説明を取り、ソースからはその行を除いて説明をコメントとして先頭に付ける。追加は `samples/` に置いて `samples.rs` の `FILES` に足す（足し忘れはテストが検出）。サンプルは既定の名前（シート1 / データ / 列1 / 列2 / 集計）を前提にした定数で書き、**Rust のテストが全サンプルを実データで実行して結果を検証する**。さらに `ui/src/samples.typecheck.test.ts` が、全サンプルがエディタと同じ厳格な TypeScript の型チェックを通ること（と、間違ったコードが実際に検出されること）を検査する。UI は `Session::macro_samples()`（Tauri コマンド `macro_samples`）から一覧を取る。
- **計算列**（`Column.computed`）: 列ごとに TS の式 `export default function (row, jx) { return ... }` を持つ。**値は保存しない**（元データと式だけが真実なので、git の差分に計算結果が出ない）。`Session::snapshot()` が状態を返すたびに `jxcel_macro::compute` で全計算列を評価し、`Snapshot.computed`（スキーマ ID → 列 ID → 行 ID → `{v}` か `{e}`）として UI に渡す。評価は表ごと・列ごとに列の並び順で行い、**後ろの計算列は前の計算列の値を読める**（前向き参照は null）。`row` は列名でアクセスする現在の行、`jx` は読み取り専用（`add/update/remove` は例外）。失敗は**セル単位**で隔離する（式の構文エラーはその列の全セル、例外や型違いはその行のセルだけが `#ERROR`。他は計算を続ける）。結果は列の型で検査する。計算は 2 秒（`COMPUTE_TIMEOUT`）で打ち切り、全セルをエラーにする（状態を返すたびに走るため、暴走する式で操作を止めない）。計算列のないファイルでは QuickJS を起動しない。計算列のセルは編集できない（`set_cell` が拒否）。通常の列を計算列にしたら保存済みの値を捨てる。マクロからは計算列の値を読めるが（実行開始時点の値）、書き込みは拒否される。永続化は式を `sheets/<sheetId>/computed/<schemaId>.<columnId>.ts`、列定義の JSON には印（`"computed": {}`）だけ。**計算列の列 ID はパスに使うので `check_id` の文字種のみ**。UI のグリッドは、`valueGetter` が計算結果を参照するため、結果が変わったら `refreshCells({force: true})` が必要（無いと、行を追加したときに新しい行の計算セルが空のままになる）。
- **テンプレート書き出し**（`crates/jxcel-export`）: xlsx / docx をテンプレートとしてファイルに埋め込み、行ごとに 1 ファイルを書き出す。テンプレートの実体は `exports/<id>.<ext>` に**元のバイト列のまま**保存し（`JxcelFile.templates`、`#[serde(skip)]` なので UI の JSON にバイト列は載らない）、設定（名前・表・ファイル名パターン・絞り込み式）は manifest の `exports`（無ければキーを出さない）。差分は `Export*` / `TemplateReplaced`。プレースホルダは `{{ 式 }}`: 列名が変数、`_no` は行番号、`std` も使える。式の評価は `jxcel_macro::evaluate_exprs`（`prelude.js` の `evalExprs`。`with(row)` ＋間接 eval。1 行の式の失敗はその行だけのエラー）。docx は Word が `{{x}}` を複数の run に分割するので段落ごとに run を結合して置換する（書式は先頭の run を採用）。xlsx は共有文字列（リッチテキスト含む）とインライン文字列を処理し、式の結果が数値・真偽ならセルを数値/真偽型で書き、`fullCalcOnLoad` を立てる。欄だけのセルの値が日付（`YYYY-MM-DD`）・日時の文字列で、そのセルの**表示形式が日付**（`styles.xml` の `numFmtId`／書式コードを見る。`date1904` も考慮）なら、Excel のシリアル値の数値にする（表示形式が日付でなければ文字列のまま。日時は書かれたままの時刻で、オフセットは無視する。1900 年より前は文字列）。xlsx のヘッダー・フッター（`headerFooter`）の欄も差し込む（値の `&` は `&&`）。数式セルの計算済みの値（`<v>`）は捨てる（LibreOffice は `fullCalcOnLoad` を見ず保存済みの値を信用するため）。書き出しは**既存ファイルを上書きしない**（`write_new_file` が ` (n)` を付ける）。ファイル名はサニタイズし、重複は連番にする。**行ループ**: 表の行（docx は `<w:tr>`、xlsx はシートの `<row>`）のどこかに `{{#each 式}}` と書くと、その行が式（配列）の要素の数だけ繰り返される（`jxcel_export::Source` が `value`・`loop_len`・`item_value` を提供し、ループは出現順の番号で呼ばれる。`plan()` が欄とループの一覧を作る）。行の中では要素がオブジェクトならキーが変数（行の列より優先）、`_item`・`_i`（0 から）・`_n`（1 から）も使える。評価は 1 回の QuickJS 実行にまとめる（`jxcel_macro::evaluate_template`、`prelude.js` の `evalExprs` の `loops`）。配列・オブジェクト型の列の値は、**フィールドの ID ではなく名前**をキーにして渡す（`prelude.js` の `named`。マクロの `rows()` は従来どおり ID のまま）。対象が `null` なら 0 件、配列以外・上限（10000 件）超過はその行のエラー。docx は 0 件で行を消す。xlsx は 0 件でも**空の行を 1 行残す**（数式の参照が壊れないように）。xlsx は後ろの行を下へずらし、セルの番地・数式の参照（ループの行を含む範囲は広がる）・`dimension`・結合セル（ループの行に収まるものは行ごとに複製）・条件付き書式・入力規則・ハイパーリンクの範囲も直す。**未対応**: ループの入れ子、ほかのシートへの参照・共有数式・定義名（印刷範囲）のずらし、段落（表の外）の繰り返し。プレビュー（`export_preview`）は書き込まずに同じ処理を行い、ループの対象と行ごとの件数も返す。`quick-xml` は 0.36 に固定（0.38 は実体参照を別イベントにするので `xml.rs` が崩れる）。フィクスチャは `crates/jxcel-export/tools/make_export_fixtures.py`（python-docx / openpyxl / LibreOffice）で作り直せる。UI は `ExportPanel.tsx`。モックは書き出せない（プレビューまで。テンプレートの中身を読めないので、行ループも分からない）。
- **LAN フォーム配信**（`crates/jxcel-app/src/forms.rs`、ブラウザ側の画面は `form.html`）: `Form`（`JxcelFile.forms`、manifest の `forms`。無ければキーを出さない。差分は `Form*`）が「表のどの列を入力欄にするか」を持ち、`FormServer`（`tiny_http`、`0.0.0.0` で待ち受け、4 スレッド）が同じネットワークのブラウザに配る。回答は開いている `Session` の表に**行として追加**され（`Session::submit_form`。未保存の変更になるだけで、保存は利用者が行う）、`jxcel://changed` イベントで UI が `current_file` を取り直す。**回答の検証は通常の編集と同じ列の型検証**で、ブラウザの値（文字列）は `convert` が列の型に直す（int/float は文字列も受ける、decimal は `1e3` を拒否、日時はブラウザ側で `toISOString()` して UTC にする）。入力欄にできるのは String/Int/Float/Decimal/Bool/Date/DateTime/Enum で計算列とネスト型は不可（`forms::field_kind`。**UI の `ui/src/forms.ts` の `canBeField` と同じ規則なので、変えるなら両方**）。フォームにない列・計算列には書けない。安全の方針: URL は `/f/<32 桁の乱数トークン>` で、トークンは**配信を始めるたびに作り直し、ファイルには保存しない**（知らなければフォームの存在も分からない）。トークンは `FormServer::status()` を呼んだときに作る（UI は配信開始・フォームの増減・回答の到着のたびに呼ぶ）。本文は 64KB まで、HTML 画面はフォームの定義（`/def` の JSON）を DOM API で組み立てる（文字列を HTML に埋め込まない）、CSP と `no-store` を付ける。`FormServer` の `Drop` は停止を指示するだけでスレッドの終了は待たない（本文をゆっくり送る相手で止まらないように。ポートは各スレッドが抜けた時点で閉じる）。Tauri 側は `Arc<Mutex<Session>>` を管理し、配信スレッドと共有する（`Mutex<Option<FormServer>>` が配信の状態）。ブラウザ単体のモックは配信できず、URL の表示と、テスト用の `window.__mockSubmit(フォーム名か ID, {列名か ID: 値})` で回答の到着を再現するだけ。アプリを起動せず試すには `cargo run -p jxcel-app --example serve_forms`（サンプルのフォームを配信し、回答のたびに全行を出力）。未実装: 回答者の認証・回答の編集・複数ファイルの同時配信・回数制限。
- `crates/jxcel-app` — Tauri 非依存の操作ロジック `Session`（開いているファイル、保存先、未保存フラグ、編集、履歴操作）。`src-tauri/src/lib.rs` はこれを呼ぶだけなので、ロジックは `cargo test` で検証できる。編集は「複製に適用→成功したら差し替え」で、失敗した編集は状態を残さない。セル編集と列定義の変更は型検証を通らなければ拒否される。`restore` は履歴の内容を未保存状態として読み込み、保存して初めて新しいコミットになる。`dirty`（未保存の変更あり）と保存先の有無は別の状態: `new_file` は `dirty: false`・保存先なしで始まり、最初の編集で `dirty: true` になる（未編集の新規ファイルを閉じても警告しない。保存は保存先を指定すればできる）。
- UI は `Backend` インターフェース（`ui/src/backend.ts`）越しにバックエンドを呼ぶ。実装は 2 つ: `tauriBackend.ts`（`invoke`）と `mockBackend.ts`（ブラウザ単体用の in-memory。Rust の挙動の簡易再現で、永続化しない）。**コマンドを増減したら `src-tauri/src/lib.rs`、`tauriBackend.ts`、`mockBackend.ts`、`types.ts`（serde 表現の鏡）を揃える。** `DataType` は `{kind: "dateTime", ...}` のように serde の camelCase タグで往復する。
- **未保存警告**: `App.tsx` の `guard()` が「新規」「開く」「ウィンドウを閉じる」の前に、未保存（`snap.dirty`）なら保存して続行/破棄して続行/キャンセルを確認する。閉じる操作は `Backend.onCloseRequested` で横取りし（Tauri は `preventDefault`）、確認後は `closeWindow()`＝`destroy()` で閉じる（`close()` だとハンドラが再発火する）。`destroy` には capability `core:window:allow-destroy` が必要。モックは `window.__requestClose()` / `window.__closed` でブラウザ上から検証できる。`Dialog` は種類とタイトルを `key` にして作り直している（付けないと、確認ダイアログから続けて開く入力ダイアログに前の `useState` の初期値が残り、名前欄が空になる）。
- **マクロエディタ**（`ui/src/components/MacroPanel.tsx`）: Monaco をローカルバンドルで使う（`monacoSetup.ts`。Monaco 0.57 のワーカーは `monaco-editor/editor/editor.worker?worker`、`esm/vs/` は付けない）。`jx` の型は `jxcelApi.d.ts.txt` を extraLib で渡す。入力は 0.5 秒止まってから自動保存するが、**保存・終了確認・パネルを閉じる前には `registerFlush` で保留中の編集を必ず書き出す**（これが無いと、入力直後の保存でその編集が落ちる）。`App.tsx` はその直後の判定のため、最新のスナップショットを `snapRef` にも持つ。ブラウザ単体のモックは TS を変換できないので、型注釈のない JS だけ実行できる。
- グリッドは AG Grid の `readOnlyEdit` で使う。グリッド自身は値を書き換えず、編集要求（`onCellEditRequest`）をバックエンドに送り、検証を通った結果のスナップショットで再描画する。
- 行の並びは `sheet.json` の `rowOrder` に分離している。並べ替えで行本体のファイルが変わらないようにするため。
- 突合はすべて安定 ID（行は `rowId`(ULID)、列は `columnId`）。列の表示名は変更可、ID は不変。`diff` もこの ID で行・列・セル単位の `Change` を返す。
- `DataType::Custom` + `TypeRegistry` は将来の JS 型拡張の差し込み口。未登録のカスタム型は検証エラーになる。
- `Decimal` は精度保持のため文字列で保持する。`Null` は型に関わらず許容し、必須は `Column.required` で判定する。
- `FORMAT_VERSION`（`jxcel-core/src/lib.rs`）は永続化形式を非互換に変えたときに上げる。
