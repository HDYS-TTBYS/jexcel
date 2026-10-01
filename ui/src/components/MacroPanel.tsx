import Editor from "@monaco-editor/react";
import { useEffect, useRef, useState } from "react";
import "../monacoSetup";
import type { Backend } from "../backend";
import type { Macro, MacroSample, RunOutput, Snapshot } from "../types";

interface Props {
  backend: Backend;
  macros: Macro[];
  onSnapshot: (s: Snapshot) => void;
  onError: (message: string) => void;
  onPrompt: (title: string, initial: string, onOk: (value: string) => void) => void;
  onConfirm: (title: string, message: string, onOk: () => void) => void;
  /** 保存・終了確認の前に、保留中の編集を書き出す関数を呼び出し側に渡す（パネルを閉じると null） */
  registerFlush: (flush: (() => Promise<void>) | null) => void;
}

type Output = { kind: "ok"; run: RunOutput } | { kind: "error"; message: string } | null;

export default function MacroPanel({ backend, macros, onSnapshot, onError, onPrompt, onConfirm, registerFlush }: Props) {
  const [selectedId, setSelectedId] = useState<string | null>(macros[0]?.id ?? null);
  const selected = macros.find((m) => m.id === selectedId) ?? macros[0] ?? null;
  const [draft, setDraft] = useState(selected?.source ?? "");
  const [output, setOutput] = useState<Output>(null);
  const [running, setRunning] = useState(false);
  const saveTimer = useRef<number>(undefined);
  // 追加メニュー（空のマクロ / サンプルから）。サンプルは開いたときに初めて取得する
  const [menuOpen, setMenuOpen] = useState(false);
  const [samples, setSamples] = useState<MacroSample[] | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!menuOpen) return;
    const close = (e: MouseEvent) => {
      if (!menuRef.current?.contains(e.target as Node)) setMenuOpen(false);
    };
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, [menuOpen]);

  // 選択したマクロが変わったとき、または履歴の復元などで外から差し替わったときに、エディタの内容を合わせる
  const savedSource = selected?.source;
  const selectedKey = selected?.id;
  useEffect(() => {
    setDraft(savedSource ?? "");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedKey, savedSource]);
  useEffect(() => setOutput(null), [selectedKey]);

  const flush = async (id: string, name: string, source: string) => {
    window.clearTimeout(saveTimer.current);
    pending.current = null;
    try {
      onSnapshot(await backend.updateMacro(id, name, source));
    } catch (e) {
      onError(String(e));
    }
  };

  // 入力のたびに送ると重いので、少し止まってから保存する。
  // ただし保存・終了確認・パネルを閉じる前には、止まるのを待たずに書き出す（registerFlush）。
  const pending = useRef<{ id: string; name: string; source: string } | null>(null);
  const flushPending = async () => {
    const p = pending.current;
    if (p) await flush(p.id, p.name, p.source);
  };
  const flushRef = useRef(flushPending);
  flushRef.current = flushPending;
  useEffect(() => {
    registerFlush(() => flushRef.current());
    return () => {
      void flushRef.current(); // パネルを閉じる直前の編集も失わない
      registerFlush(null);
    };
  }, [registerFlush]);

  const onChange = (value: string | undefined) => {
    const next = value ?? "";
    setDraft(next);
    if (!selected) return;
    pending.current = { id: selected.id, name: selected.name, source: next };
    window.clearTimeout(saveTimer.current);
    saveTimer.current = window.setTimeout(() => void flushRef.current(), 500);
  };

  const run = async () => {
    if (!selected) return;
    setRunning(true);
    try {
      // 保存前の内容で実行する。成功すれば反映され、失敗すれば何も変わらない
      const result = await backend.runMacro(selected.id, draft);
      setOutput({ kind: "ok", run: result });
      onSnapshot(result.snapshot);
    } catch (e) {
      setOutput({ kind: "error", message: String(e) });
    } finally {
      setRunning(false);
    }
  };

  const toggleMenu = () => {
    setMenuOpen((open) => !open);
    if (!samples) backend.macroSamples().then(setSamples, (e) => onError(String(e)));
  };

  /** `sample` が null なら空のマクロ（雛形）を追加する */
  const add = (sample: MacroSample | null) => {
    setMenuOpen(false);
    onPrompt("マクロの追加", sample?.name ?? `マクロ${macros.length + 1}`, async (name) => {
      try {
        const snap = await backend.addMacro(name, sample?.source);
        onSnapshot(snap);
        setSelectedId(snap.file.macros[snap.file.macros.length - 1].id);
      } catch (e) {
        onError(String(e));
      }
    });
  };

  return (
    <aside className="macros" aria-label="マクロ">
      <div className="macro-bar">
        <select aria-label="マクロ" value={selected?.id ?? ""} onChange={(e) => setSelectedId(e.target.value)} disabled={macros.length === 0}>
          {macros.length === 0 && <option value="">（マクロなし）</option>}
          {macros.map((m) => (
            <option key={m.id} value={m.id}>
              {m.name}
            </option>
          ))}
        </select>
        <div className="add-menu-wrap" ref={menuRef}>
          <button onClick={toggleMenu} aria-haspopup="menu" aria-expanded={menuOpen}>
            + 追加 ▾
          </button>
          {menuOpen && (
            <div className="add-menu" role="menu">
              <button role="menuitem" onClick={() => add(null)}>
                <strong>空のマクロ</strong>
              </button>
              <div className="menu-title">サンプルから</div>
              {samples === null && <p className="muted pad-s">読み込み中…</p>}
              {samples?.map((sm) => (
                <button key={sm.id} role="menuitem" onClick={() => add(sm)}>
                  <strong>{sm.name}</strong>
                  <span className="muted">{sm.description}</span>
                </button>
              ))}
            </div>
          )}
        </div>
        {selected && (
          <>
            <button onClick={() => onPrompt("マクロ名の変更", selected.name, (n) => void flush(selected.id, n, draft))}>名前変更</button>
            <button
              onClick={() =>
                onConfirm("マクロの削除", `マクロ「${selected.name}」を削除します。よろしいですか？`, async () => {
                  try {
                    onSnapshot(await backend.deleteMacro(selected.id));
                  } catch (e) {
                    onError(String(e));
                  }
                })
              }
            >
              削除
            </button>
            <span className="spacer" />
            <button className="primary" onClick={run} disabled={running}>
              {running ? "実行中…" : "▶ 実行"}
            </button>
          </>
        )}
      </div>

      {selected ? (
        <>
          <div className="macro-editor">
            <Editor
              height="100%"
              language="typescript"
              path={`file:///macro-${selected.id}.ts`}
              value={draft}
              onChange={onChange}
              options={{ minimap: { enabled: false }, fontSize: 13, automaticLayout: true, scrollBeyondLastLine: false, tabSize: 2 }}
            />
          </div>
          <div className="macro-output" aria-label="実行結果">
            {!output && <p className="muted">▶ 実行すると、ここにログと結果が出ます。書き込みは検証に通ったときだけ反映されます。</p>}
            {output?.kind === "error" && <pre className="out-error">{output.message}</pre>}
            {output?.kind === "ok" && (
              <>
                <p className="out-summary">
                  {output.run.ops > 0 ? `${output.run.ops} 件の書き込みを反映しました（未保存）` : "書き込みはありませんでした"}
                </p>
                {output.run.logs.length > 0 && <pre className="out-log">{output.run.logs.join("\n")}</pre>}
                {output.run.result !== null && output.run.result !== undefined && (
                  <pre className="out-result">戻り値: {JSON.stringify(output.run.result, null, 2)}</pre>
                )}
              </>
            )}
          </div>
        </>
      ) : (
        <p className="muted pad">マクロがありません。「+ 追加」から、空のマクロまたはサンプルを作成できます。</p>
      )}
    </aside>
  );
}
