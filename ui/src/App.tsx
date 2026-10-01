import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Backend } from "./backend";
import { Dialog, type DialogSpec } from "./components/Modal";
import { Grid } from "./components/Grid";
import { HistoryPanel } from "./components/HistoryPanel";
import { SchemaEditor } from "./components/SchemaEditor";

// Monaco は大きいので、マクロパネルを開いたときに初めて読み込む
const MacroPanel = lazy(() => import("./components/MacroPanel"));
import { newId, type Column, type Snapshot } from "./types";

export function App({ backend }: { backend: Backend }) {
  const [snap, setSnapState] = useState<Snapshot | null>(null);
  // 非同期の途中（マクロの書き出し直後など）でも最新の状態を読めるよう、ref にも持つ
  const snapRef = useRef<Snapshot | null>(null);
  const setSnap = useCallback((s: Snapshot | null) => {
    snapRef.current = s;
    setSnapState(s);
  }, []);
  const [sheetId, setSheetId] = useState<string | null>(null);
  const [schemaId, setSchemaId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState("");
  const [showHistory, setShowHistory] = useState(false);
  const [showSchema, setShowSchema] = useState(false);
  const [showMacros, setShowMacros] = useState(false);
  const [dialog, setDialog] = useState<DialogSpec | null>(null);
  const [historyKey, setHistoryKey] = useState(0);
  // マクロエディタの保留中の編集を書き出す（保存・確認の前に呼ぶ）
  const macroFlush = useRef<(() => Promise<void>) | null>(null);
  const registerFlush = useCallback((f: (() => Promise<void>) | null) => {
    macroFlush.current = f;
  }, []);

  const onError = useCallback((e: unknown) => setError(String(e)), []);

  // 操作を実行し、成功したら状態を差し替える。失敗は画面に出す。
  const run = useCallback(
    async (op: Promise<Snapshot>, afterSave = false) => {
      try {
        setSnap(await op);
        setError(null);
        if (afterSave) setHistoryKey((k) => k + 1);
        return true;
      } catch (e) {
        onError(e);
        return false;
      }
    },
    [onError, setSnap],
  );

  // 選択中のシート/スキーマが存在しなくなったら先頭に寄せる
  const sheet = useMemo(() => snap?.file.sheets.find((s) => s.id === sheetId) ?? snap?.file.sheets[0], [snap, sheetId]);
  const schema = useMemo(() => sheet?.schemas.find((s) => s.id === schemaId) ?? sheet?.schemas[0], [sheet, schemaId]);

  /** 保存する。保存先の選択をキャンセルした場合や失敗した場合は false。 */
  const save = useCallback(
    async (as = false): Promise<boolean> => {
      if (!snapRef.current) return false;
      await macroFlush.current?.(); // 入力直後に保存しても、最後の編集が入るように
      const current = snapRef.current;
      if (!current) return false;
      let path: string | null = null;
      if (as || !current.path) {
        path = await backend.pickSavePath(`${current.file.name || "無題"}.jxcel`);
        if (!path) return false;
      }
      const ok = await run(backend.saveFile(path, message), true);
      if (ok) setMessage("");
      return ok;
    },
    [backend, message, run],
  );

  // 未保存の変更を捨てることになる操作の前に確認する。保存に失敗/キャンセルしたら先へ進まない。
  const guard = async (title: string, action: () => void) => {
    // 入力直後で自動保存前の編集があれば、先に反映して「未保存」を正しく判定する
    await macroFlush.current?.();
    const latest = snapRef.current;
    if (!latest?.dirty) return action();
    setDialog({
      kind: "unsaved",
      title,
      message: "保存されていない変更があります。",
      onSave: async () => {
        if (await save()) action();
      },
      onDiscard: action,
    });
  };

  // ウィンドウを閉じる操作。ハンドラは一度だけ登録するので、最新の guard を ref 越しに呼ぶ。
  const closeGuard = useRef<() => void>(() => {});
  closeGuard.current = () => guard("終了前の確認", () => void backend.closeWindow());
  useEffect(() => {
    const unlisten = backend.onCloseRequested(() => closeGuard.current());
    return () => void unlisten.then((f) => f());
  }, [backend]);

  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key === "s") {
        e.preventDefault();
        void save(e.shiftKey);
      }
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [save]);

  const open = () =>
    guard("ファイルを開く前の確認", async () => {
      const p = await backend.pickOpenPath();
      if (p && (await run(backend.openFile(p)))) {
        setSheetId(null);
        setSchemaId(null);
        setHistoryKey((k) => k + 1);
      }
    });

  const create = () =>
    guard("新規作成前の確認", () =>
      setDialog({
      kind: "prompt",
      title: "新しいファイル",
      initial: "無題",
      onOk: async (name) => {
        if (await run(backend.newFile(name))) {
          setSheetId(null);
          setSchemaId(null);
          setHistoryKey((k) => k + 1);
        }
      },
      }),
    );

  const toolbar = (
    <header className="toolbar">
      <button onClick={create}>新規</button>
      <button onClick={open}>開く</button>
      <button className="primary" disabled={!snap} onClick={() => save()}>
        保存{snap?.dirty ? " *" : ""}
      </button>
      <button disabled={!snap} onClick={() => save(true)}>
        名前を付けて保存
      </button>
      <input
        className="msg-input"
        placeholder="変更メモ（任意・履歴に残ります）"
        value={message}
        disabled={!snap}
        onChange={(e) => setMessage(e.target.value)}
      />
      <span className="spacer" />
      <button disabled={!snap} aria-pressed={showMacros} onClick={() => setShowMacros((v) => !v)}>
        マクロ
      </button>
      <button disabled={!snap} aria-pressed={showHistory} onClick={() => setShowHistory((v) => !v)}>
        履歴
      </button>
    </header>
  );

  if (!snap || !sheet) {
    return (
      <div className="app">
        {toolbar}
        <main className="welcome">
          <h1>jxcel</h1>
          <p>JSON をファイルとする、型付きスプレッドシート。保存のたびに履歴が残ります。</p>
          <button className="primary" onClick={create}>
            新規作成
          </button>
        </main>
        {error && <div className="toast error">{error}</div>}
        {dialog && <Dialog key={`${dialog.kind}:${dialog.title}`} spec={dialog} onClose={() => setDialog(null)} />}
      </div>
    );
  }

  return (
    <div className="app">
      {toolbar}
      <div className="title">
        {snap.file.name}
        <span className="muted"> {snap.path ?? "（未保存）"}</span>
      </div>

      <nav className="tabs" aria-label="シート">
        {snap.file.sheets.map((s) => (
          <button
            key={s.id}
            className={s.id === sheet.id ? "tab active" : "tab"}
            onClick={() => {
              setSheetId(s.id);
              setSchemaId(null);
            }}
            onDoubleClick={() =>
              setDialog({ kind: "prompt", title: "シート名の変更", initial: s.name, onOk: (n) => void run(backend.renameSheet(s.id, n)) })
            }
          >
            {s.name}
          </button>
        ))}
        <button className="tab add" title="シートを追加" onClick={() => setDialog({ kind: "prompt", title: "シートの追加", initial: `シート${snap.file.sheets.length + 1}`, onOk: (n) => void run(backend.addSheet(n)) })}>
          +
        </button>
        <span className="spacer" />
        <button
          title="このシートを削除"
          onClick={() =>
            setDialog({ kind: "confirm", title: "シートの削除", message: `シート「${sheet.name}」を削除します。よろしいですか？`, onOk: () => void run(backend.deleteSheet(sheet.id)) })
          }
        >
          シートを削除
        </button>
      </nav>

      <nav className="tabs schemas" aria-label="スキーマ">
        {sheet.schemas.map((s) => (
          <button key={s.id} className={s.id === schema?.id ? "tab active" : "tab"} onClick={() => setSchemaId(s.id)}>
            {s.name}
          </button>
        ))}
        <button
          className="tab add"
          title="スキーマを追加"
          onClick={() =>
            setDialog({
              kind: "prompt",
              title: "スキーマの追加",
              initial: `スキーマ${sheet.schemas.length + 1}`,
              onOk: async (name) => {
                const cols: Column[] = [{ id: newId(), name: "列1", type: { kind: "string" } }];
                if (await run(backend.addSchema(sheet.id, name, cols))) setSchemaId(null);
              },
            })
          }
        >
          +
        </button>
        <span className="spacer" />
        {schema && (
          <>
            <button onClick={() => setShowSchema(true)}>列の定義</button>
            <button onClick={() => void run(backend.addRow(sheet.id, schema.id))}>+ 行を追加</button>
          </>
        )}
      </nav>

      <div className="content">
        <main className="main">
          {schema ? (
            <Grid
              key={schema.id + schema.columns.map((c) => c.id + c.type.kind).join()}
              schema={schema}
              computed={snap.computed[schema.id]}
              onError={onError}
              onSetCell={(row, col, value) => void run(backend.setCell(sheet.id, schema.id, row, col.id, value))}
              onDeleteRow={(row) => void run(backend.deleteRow(sheet.id, schema.id, row))}
            />
          ) : (
            <p className="muted pad">スキーマがありません。「+」で追加してください。</p>
          )}
        </main>
        {showMacros && (
          <Suspense fallback={<aside className="macros"><p className="muted pad">エディタを読み込み中…</p></aside>}>
            <MacroPanel
              backend={backend}
              macros={snap.file.macros}
              onSnapshot={setSnap}
              onError={onError}
              onPrompt={(title, initial, onOk) => setDialog({ kind: "prompt", title, initial, onOk })}
              onConfirm={(title, message, onOk) => setDialog({ kind: "confirm", title, message, onOk })}
              registerFlush={registerFlush}
            />
          </Suspense>
        )}
        {showHistory && (
          <HistoryPanel
            backend={backend}
            file={snap.file}
            refreshKey={historyKey}
            onError={onError}
            onRestore={(rev) =>
              setDialog({
                kind: "confirm",
                title: "履歴から戻す",
                message: "この時点の内容に戻します（未保存の変更は失われます）。保存すると新しい履歴として記録されます。",
                onOk: () => void run(backend.restore(rev)),
              })
            }
          />
        )}
      </div>

      {error && (
        <div className="toast error" role="alert" onClick={() => setError(null)}>
          {error}
        </div>
      )}
      {showSchema && schema && (
        <SchemaEditor
          schema={schema}
          onClose={() => setShowSchema(false)}
          onAdd={(c) => void run(backend.addColumn(sheet.id, schema.id, c))}
          onUpdate={(c) => void run(backend.updateColumn(sheet.id, schema.id, c))}
          onDelete={(id) => void run(backend.deleteColumn(sheet.id, schema.id, id))}
        />
      )}
      {dialog && <Dialog key={`${dialog.kind}:${dialog.title}`} spec={dialog} onClose={() => setDialog(null)} />}
    </div>
  );
}
