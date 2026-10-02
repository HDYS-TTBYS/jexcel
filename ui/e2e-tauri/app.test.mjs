// Tauri の実ウィンドウ（WebKitGTK）を WebDriver（tauri-driver + WebKitWebDriver）で操作する。
// ブラウザ単体の e2e（../e2e）が見られない、本物の IPC・Rust のセッション・フォーム配信のスレッド・
// Tauri のイベントを通して確かめる。OS のファイルダイアログだけは自動操作できないので、
// `tauriBackend.ts` のテスト用フック（window.__jxcelDialog）で、決めたパスを返す（IPC の口は凍結されていて差し替えられない）。
//
// 実行: pnpm --dir ui e2e:tauri（先に `pnpm tauri build --debug --no-bundle` が要る。DISPLAY が無ければ xvfb-run で起動する）
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dirname, "../..");
const APP = process.env.JXCEL_BIN ?? join(root, "src-tauri/target/debug/jxcel");
const PORT = 4444;
const WD = `http://127.0.0.1:${PORT}`;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

let driver, sid, tmp;

async function wd(method, path, body) {
  const res = await fetch(`${WD}${path}`, { method, headers: { "content-type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body) });
  const j = await res.json();
  if (!res.ok) throw new Error(`${method} ${path}: ${JSON.stringify(j.value)}`);
  return j.value;
}
const exec = (script, args = []) => wd("POST", `/session/${sid}/execute/sync`, { script, args });
const execAsync = (script, args = []) => wd("POST", `/session/${sid}/execute/async`, { script, args });

/** 条件が真になるまで待つ（ページ内で評価する式） */
async function until(expr, what, ms = 10000) {
  const t = Date.now();
  let last;
  while (Date.now() - t < ms) {
    last = await exec(`return (${expr});`);
    if (last) return last;
    await sleep(100);
  }
  const body = await exec(`return document.body.innerText.slice(0, 600);`).catch(() => "");
  throw new Error(`待ち切れませんでした: ${what}（最後の値: ${JSON.stringify(last)}）\n画面: ${body}`);
}

const q = (sel) => JSON.stringify(sel);
/** 表示テキストでボタンを探して押す。`within` はその中だけを探す CSS セレクタ */
async function click(text, within = "body") {
  const ok = await exec(
    `const root = document.querySelector(${q(within)}); if (!root) return false;
     const b = [...root.querySelectorAll("button")].find((b) => (b.textContent.trim() === ${q(text)} || b.textContent.trim() === ${q(text)} + " *") && !b.disabled);
     if (!b) return false; b.click(); return true;`,
  );
  if (!ok) throw new Error(`ボタンが見つかりません: ${text}（${within}）`);
}
const text = (sel) => exec(`const e = document.querySelector(${q(sel)}); return e ? e.innerText : null;`);
const rows = () => exec(`return new Set([...document.querySelectorAll(".ag-row[row-index]")].map((r) => r.getAttribute("row-index"))).size;`);

/** Rust のコマンドを実 IPC で呼ぶ */
async function invoke(cmd, args = {}) {
  const r = await execAsync(
    `const [cmd, args, done] = arguments;
     window.__TAURI_INTERNALS__.invoke(cmd, args).then((v) => done({ ok: v }), (e) => done({ err: String(e) }));`,
    [cmd, args],
  );
  if ("err" in r) throw new Error(`${cmd}: ${r.err}`);
  return r.ok;
}

/** OS のダイアログを差し替える（`tauriBackend.ts` の `window.__jxcelDialog`。種類ごとに返すパス） */
const stubDialog = (paths) =>
  exec(
    `window.__dialogCalls = []; const paths = ${JSON.stringify(paths)};
     window.__jxcelDialog = (kind) => (window.__dialogCalls.push(kind), paths[kind] ?? null);`,
  );

before(async () => {
  assert.ok(existsSync(APP), `アプリがありません: ${APP}（pnpm tauri build --debug --no-bundle）`);
  tmp = mkdtempSync(join(tmpdir(), "jxcel-tauri-"));
  driver = spawn("tauri-driver", ["--port", String(PORT), "--native-driver", process.env.WEBKIT_DRIVER ?? "/usr/bin/WebKitWebDriver"], { stdio: "inherit", env: { ...process.env, GSETTINGS_BACKEND: "memory" } });
  for (let i = 0; i < 100; i++) {
    try {
      await fetch(`${WD}/status`);
      break;
    } catch {
      await sleep(100);
    }
  }
  const s = await fetch(`${WD}/session`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ capabilities: { alwaysMatch: { "tauri:options": { application: APP } } } }),
  }).then((r) => r.json());
  if (!s.value?.sessionId) throw new Error("セッションを開始できません: " + JSON.stringify(s));
  sid = s.value.sessionId;
});

after(async () => {
  try {
    await wd("DELETE", `/session/${sid}`);
  } catch {}
  driver?.kill();
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("実ウィンドウ: 新規・IPC・保存・開く・履歴・フォーム配信・未保存警告", async () => {
  // Tauri の IPC が本物であること（モックではない）
  await until(`"__TAURI_INTERNALS__" in window`, "Tauri の IPC");
  await until(`[...document.querySelectorAll("button")].some((b) => b.textContent.trim() === "新規")`, "ツールバー");

  // 新規作成（名前の入力ダイアログ）
  await click("新規");
  await until(`!!document.querySelector('[role=dialog]')`, "名前ダイアログ");
  await click("OK", "[role=dialog]");
  await until(`!!document.querySelector(".ag-row, .tabs")`, "表の表示");

  // IPC 経由でデータを作り、保存する（本物の Session・zip・git）
  const file = join(tmp, "実機.jxcel");
  let snap = await invoke("current_file");
  const sheet = snap.file.sheets[0], schema = sheet.schemas[0], col = schema.columns[0];
  snap = await invoke("add_row", { sheet: sheet.id, schema: schema.id });
  const rowId = snap.file.sheets[0].schemas[0].rows[0].id;
  await invoke("set_cell", { sheet: sheet.id, schema: schema.id, row: rowId, column: col.id, value: "最初の行" });
  await invoke("add_form", { sheet: sheet.id, schema: schema.id, name: "受付" });
  await invoke("save_file", { path: file, message: "初回" });
  assert.equal(readFileSync(file).subarray(0, 2).toString(), "PK", "保存したファイルは zip");

  // 開く: ダイアログを差し替えて、UI の「開く」から読み込む（未保存ではないので確認は出ない）
  await stubDialog({ open: file, save: file });
  await click("開く");
  await until(`document.body.innerText.includes("最初の行")`, "開いた表の表示");
  assert.deepEqual(await exec(`return window.__dialogCalls`), ["open"], "開くダイアログが呼ばれた");
  assert.equal(await rows(), 2, "新規の空の行 + add_row");

  // フォーム配信: 実サーバーに別のクライアントから回答 → イベントで UI が更新される
  await click("フォーム");
  await until(`!!document.querySelector('aside[aria-label="フォーム配信"]')`, "フォームのパネル");
  await click("配信を開始", 'aside[aria-label="フォーム配信"]');
  const url = await until(`document.querySelector('[aria-label="受付 の URL"]')?.value`, "フォームの URL");
  assert.match(url, /^http:\/\/[\d.]+:\d+\/f\/[0-9a-z]{32}$/);
  const base = url.replace(/^http:\/\/[\d.]+/, "http://127.0.0.1");
  const def = await fetch(`${base}/def`).then((r) => r.json());
  assert.equal(def.title, "受付");
  const res = await fetch(`${base}/submit`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ values: { [col.id]: "LAN から" } }) });
  assert.equal(res.status, 200, await res.clone().text());
  await until(`document.body.innerText.includes("LAN から")`, "回答の反映（jxcel://changed）");
  assert.equal(await rows(), 3);
  await until(`/保存 \\*/.test(document.body.innerText)`, "未保存の表示");
  await click("配信を停止", 'aside[aria-label="フォーム配信"]');
  await until(`document.body.innerText.includes("停止中")`, "停止");
  // 停止後はポートが閉じる（各スレッドが抜けた時点で閉じるので、少し待つ）
  let closed = false;
  for (let i = 0; i < 50 && !closed; i++) closed = await fetch(`${base}/def`, { headers: { connection: "close" } }).then(() => sleep(100).then(() => false), () => true);
  assert.ok(closed, "停止後もポートが開いたまま");

  // 保存（ダイアログは保存先が決まっているので出ない）→ 履歴に 2 件
  await exec(`const i = document.querySelector('input[placeholder^="変更メモ"]'); const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set; set.call(i, "LAN の回答"); i.dispatchEvent(new Event("input", { bubbles: true }));`);
  await click("保存");
  await until(`!/保存 \\*/.test(document.body.innerText)`, "保存済み");
  await click("履歴");
  await until(`document.querySelectorAll(".commits li").length === 2`, "履歴 2 件");
  const msgs = await exec(`return [...document.querySelectorAll(".commits .msg")].map((e) => e.innerText);`);
  assert.deepEqual(msgs.sort(), ["LAN の回答", "初回"]);

  // 未保存警告: 行を足して「新規」→ 3 択のダイアログ。キャンセルで元のまま
  await click("+ 行を追加");
  await until(`/保存 \\*/.test(document.body.innerText)`, "未保存");
  await click("新規");
  await until(`!!document.querySelector('[role=dialog]')`, "確認ダイアログ");
  const dlg = await text("[role=dialog]");
  assert.ok(dlg.includes("保存して続行") && dlg.includes("破棄して続行"), dlg);
  await click("キャンセル", "[role=dialog]");
  await until(`!document.querySelector('[role=dialog]')`, "ダイアログが閉じる");
  assert.equal(await rows(), 4, "キャンセルすると表はそのまま");

  // ウィンドウを閉じる操作の確認（onCloseRequested）は、WebDriver の「ウィンドウを閉じる」がアプリを直接終了させて
  // ハンドラを通らないため、ここでは試せない（ブラウザ単体の e2e の guard で確認している）。
});
