import { useCallback, useEffect, useMemo, useState } from "react";
import type { Backend } from "../backend";
import { formatValue } from "../parse";
import type { CellResult, Export, ExportPreview, ExportResult, JxcelFile, Snapshot } from "../types";

interface Props {
  backend: Backend;
  file: JxcelFile;
  onSnapshot: (s: Snapshot) => void;
  onError: (message: string) => void;
  onConfirm: (title: string, message: string, onOk: () => void) => void;
}

const PREVIEW_ROWS = 5;
const target = (sheet: string, schema: string) => `${sheet}|${schema}`;

function Cell({ r }: { r: CellResult }) {
  return "e" in r ? (
    <td className="cell-error" title={r.e}>
      #ERROR
    </td>
  ) : (
    <td>{formatValue(r.v)}</td>
  );
}

export default function ExportPanel({ backend, file, onSnapshot, onError, onConfirm }: Props) {
  const exports = file.exports;
  const [selectedId, setSelectedId] = useState<string | null>(exports[0]?.id ?? null);
  const selected: Export | null = exports.find((e) => e.id === selectedId) ?? exports[0] ?? null;

  // 編集中の設定。「適用」で初めてファイルに反映する（プレビューと書き出しは適用済みの設定を使う）
  const [draft, setDraft] = useState({ name: "", target: "", filename: "", filter: "" });
  const applied = useMemo(
    () => ({
      name: selected?.name ?? "",
      target: selected ? target(selected.sheet, selected.schema) : "",
      filename: selected?.filename ?? "",
      filter: selected?.filter ?? "",
    }),
    [selected],
  );
  useEffect(() => setDraft(applied), [applied]);
  const changed = JSON.stringify(draft) !== JSON.stringify(applied);

  const [preview, setPreview] = useState<ExportPreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [result, setResult] = useState<ExportResult | null>(null);
  const [running, setRunning] = useState(false);

  const selectedKey = selected?.id;
  const settingsKey = JSON.stringify(applied);
  const loadPreview = useCallback(async () => {
    if (!selectedKey) return setPreview(null);
    try {
      setPreview(await backend.exportPreview(selectedKey, PREVIEW_ROWS));
      setPreviewError(null);
    } catch (e) {
      setPreview(null);
      setPreviewError(String(e));
    }
    // 設定が変わったときにも読み直す
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [backend, selectedKey, settingsKey]);
  useEffect(() => void loadPreview(), [loadPreview]);
  useEffect(() => setResult(null), [selectedKey]);

  const run = async (fn: () => Promise<Snapshot>) => {
    try {
      onSnapshot(await fn());
      return true;
    } catch (e) {
      onError(String(e));
      return false;
    }
  };

  const add = async () => {
    const path = await backend.pickTemplatePath();
    if (!path) return;
    try {
      const snap = await backend.addExport(path);
      onSnapshot(snap);
      setSelectedId(snap.file.exports[snap.file.exports.length - 1].id);
    } catch (e) {
      onError(String(e));
    }
  };

  const replaceTemplate = async () => {
    if (!selected) return;
    const path = await backend.pickTemplatePath();
    if (path) await run(() => backend.replaceExportTemplate(selected.id, path));
  };

  const apply = () => {
    if (!selected) return;
    const [sheet, schema] = draft.target.split("|");
    void run(() => backend.updateExport(selected.id, draft.name.trim() || selected.name, sheet, schema, draft.filename, draft.filter.trim() || null));
  };

  const execute = async () => {
    if (!selected) return;
    const dir = await backend.pickFolder();
    if (!dir) return;
    setRunning(true);
    setResult(null);
    try {
      setResult(await backend.runExport(selected.id, dir));
    } catch (e) {
      onError(String(e));
    } finally {
      setRunning(false);
    }
  };

  const targets = file.sheets.flatMap((s) => s.schemas.map((c) => ({ value: target(s.id, c.id), label: `${s.name} / ${c.name}` })));
  const excludedCount = preview?.rows.filter((r) => r.excluded).length ?? 0;

  return (
    <aside className="exports" aria-label="書き出し">
      <div className="macro-bar">
        <select aria-label="書き出し" value={selected?.id ?? ""} onChange={(e) => setSelectedId(e.target.value)} disabled={exports.length === 0}>
          {exports.length === 0 && <option value="">（書き出しなし）</option>}
          {exports.map((e) => (
            <option key={e.id} value={e.id}>
              {e.name}
            </option>
          ))}
        </select>
        <button onClick={add}>+ テンプレートを追加</button>
        {selected && (
          <button
            onClick={() =>
              onConfirm("書き出しの削除", `書き出し「${selected.name}」とそのテンプレートを削除します。よろしいですか？`, () =>
                void run(() => backend.deleteExport(selected.id)),
              )
            }
          >
            削除
          </button>
        )}
      </div>

      {!selected ? (
        <div className="export-body">
          <p className="muted">
            Word（.docx）や Excel（.xlsx）のテンプレートを取り込むと、表の行ごとに差し込んだファイルをまとめて書き出せます。
          </p>
          <p className="muted">
            テンプレートの中に <code>{"{{品名}}"}</code> や <code>{"{{数量 * 単価}}"}</code> のように書きます。列名はそのまま変数として使え、
            <code>std</code>（標準ライブラリ）も使えます。
          </p>
          <p className="muted">
            表の行（Word は表の行、Excel はシートの行）のどこかに <code>{"{{#each 明細}}"}</code> と書くと、その行が <code>明細</code> の配列の要素の数だけ
            繰り返されます。行の中では要素のキー（<code>{"{{品目}}"}</code>）や <code>_item</code>・<code>_n</code>（1 から数える番号）が使えます。
            対象は、配列の列のほか、<code>{'jx.sheet("シート1").schema("明細").rows().filter(r => r.番号 === 番号)'}</code> のような式でもかまいません。
          </p>
          <p className="muted">
            複数の行をひとまとめに繰り返すには、最初の行に <code>{"{{#each 明細}}"}</code>、最後の行に <code>{"{{/each}}"}</code> と書きます（印の行も繰り返されます）。
            この形はループを入れ子にでき、内側のループの対象と欄では外側の要素のキーも使えます（<code>_parent</code> が外側の要素）。
            1 行だけの内側のループは、同じ行に <code>{"{{#each 付属}}…{{/each}}"}</code> と書きます。
          </p>
        </div>
      ) : (
        <div className="export-body">
          <div className="export-form">
            <label>
              名前
              <input aria-label="書き出しの名前" value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} />
            </label>
            <div className="template-row">
              <span>
                テンプレート: <strong>{selected.templateName}</strong> <span className="muted">（.{selected.kind}。ファイルに保存されます）</span>
              </span>
              <button onClick={replaceTemplate}>差し替え</button>
            </div>
            <label>
              対象の表
              <select aria-label="対象の表" value={draft.target} onChange={(e) => setDraft({ ...draft, target: e.target.value })}>
                {targets.map((t) => (
                  <option key={t.value} value={t.value}>
                    {t.label}
                  </option>
                ))}
              </select>
            </label>
            <label>
              出力ファイル名
              <input
                aria-label="出力ファイル名"
                value={draft.filename}
                placeholder="{{請求番号}}_{{取引先}}"
                onChange={(e) => setDraft({ ...draft, filename: e.target.value })}
              />
              <span className="muted">拡張子は自動で付きます。使えない文字は _ に置き換え、同じ名前は (2) を付けます。_no は行番号。</span>
            </label>
            <label>
              絞り込み（任意）
              <input aria-label="絞り込み" value={draft.filter} placeholder={'状態 === "確定"'} onChange={(e) => setDraft({ ...draft, filter: e.target.value })} />
              <span className="muted">この式が真になる行だけを書き出します。空なら全行。</span>
            </label>
            <div className="actions left">
              <button className="primary" disabled={!changed} onClick={apply}>
                適用
              </button>
              {changed && <span className="muted">未適用の変更があります</span>}
            </div>
          </div>

          <h4>
            プレビュー <button onClick={() => void loadPreview()} title="データを変えたあとに読み直す">↻ 更新</button>
          </h4>
          {previewError && <pre className="out-error">{previewError}</pre>}
          {preview && (
            <>
              <p className="muted">
                テンプレートの差し込み欄 {preview.placeholders.length} 個 / 対象 {preview.totalRows} 行
                {excludedCount > 0 && `（表示中の ${excludedCount} 行は絞り込みで除外）`}
              </p>
              <div className="preview-scroll">
                <table className="preview">
                  <thead>
                    <tr>
                      <th>#</th>
                      <th>出力ファイル名</th>
                      {preview.placeholders.map((p) => (
                        <th key={p}>
                          <code>{p}</code>
                        </th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {preview.rows.map((r) => (
                      <tr key={r.rowNo} className={r.excluded ? "excluded" : ""}>
                        <td>{r.rowNo}</td>
                        <Cell r={r.filename} />
                        {r.values.map((v, i) => (
                          <Cell key={i} r={v} />
                        ))}
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              {preview.totalRows > preview.rows.length && <p className="muted">先頭 {preview.rows.length} 行を表示しています。</p>}
              {preview.loops.length > 0 && (
                <div className="loops">
                  <h4>行ループ</h4>
                  {preview.loops.map((l, i) => (
                    <div key={i} className="loop" style={l.parent != null ? { marginLeft: "1.2em" } : undefined}>
                      <p>
                        {l.parent != null && <span className="muted">（{`{{#each ${preview.loops[l.parent].source}}}`} の中）</span>}
                        <code>{`{{#each ${l.source}}}`}</code> の中の欄:{" "}
                        {l.exprs.length === 0 ? <span className="muted">なし</span> : l.exprs.map((x) => <code key={x}>{x}</code>)}
                      </p>
                      <p className="muted">
                        繰り返しの回数（行ごと）:{" "}
                        {l.counts.map((c, k) => (
                          <span key={k} className={"e" in c ? "cell-error" : ""} title={"e" in c ? c.e : undefined}>
                            {k + 1} 行目 {"e" in c ? "エラー" : `${String(c.v)} 件`}
                            {k < l.counts.length - 1 ? " / " : ""}
                          </span>
                        ))}
                      </p>
                    </div>
                  ))}
                </div>
              )}
            </>
          )}

          <div className="actions left export-run">
            <button className="primary" disabled={running || changed || !!previewError} onClick={execute} title={changed ? "先に「適用」してください" : undefined}>
              {running ? "書き出し中…" : "書き出す…"}
            </button>
            <span className="muted">フォルダを選ぶと、行ごとに 1 ファイルを作ります。既存のファイルは上書きしません。</span>
          </div>

          {result && (
            <div className="export-result" role="status">
              <p className="out-summary">
                {result.written.length} 個のファイルを書き出しました
                {result.skipped > 0 && `（絞り込みで ${result.skipped} 行を除外）`}
              </p>
              <p className="muted">{result.outDir}</p>
              <ul>
                {result.written.slice(0, 20).map((w) => (
                  <li key={w.filename}>{w.filename}</li>
                ))}
                {result.written.length > 20 && <li className="muted">ほか {result.written.length - 20} 個</li>}
              </ul>
              {result.errors.length > 0 && (
                <>
                  <p className="out-error">書き出せなかった行が {result.errors.length} 件あります（他の行は書き出されています）</p>
                  <ul className="out-error">
                    {result.errors.map((e) => (
                      <li key={e.rowNo}>
                        {e.rowNo} 行目: {e.message}
                      </li>
                    ))}
                  </ul>
                </>
              )}
              {result.warnings.length > 0 && (
                <>
                  <p className="out-warn">書き出しましたが、注意が {result.warnings.length} 件あります</p>
                  <ul className="out-warn">
                    {result.warnings.map((w, i) => (
                      <li key={i}>
                        {w.rowNo} 行目: {w.message}
                      </li>
                    ))}
                  </ul>
                </>
              )}
            </div>
          )}
        </div>
      )}
    </aside>
  );
}
