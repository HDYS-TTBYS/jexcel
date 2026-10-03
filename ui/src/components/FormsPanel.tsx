import { useCallback, useEffect, useMemo, useState } from "react";
import type { Backend } from "../backend";
import { canBeField } from "../forms";
import type { Form, FormsStatus, JxcelFile, Snapshot } from "../types";

interface Props {
  backend: Backend;
  file: JxcelFile;
  onSnapshot: (s: Snapshot) => void;
  onError: (message: string) => void;
  onConfirm: (title: string, message: string, onOk: () => void) => void;
}

const DEFAULT_PORT = 8787;
const target = (sheet: string, schema: string) => `${sheet}|${schema}`;

export default function FormsPanel({ backend, file, onSnapshot, onError, onConfirm }: Props) {
  const forms = file.forms;
  const [selectedId, setSelectedId] = useState<string | null>(forms[0]?.id ?? null);
  const selected: Form | null = forms.find((f) => f.id === selectedId) ?? forms[0] ?? null;

  const targets = useMemo(
    () => file.sheets.flatMap((s) => s.schemas.map((c) => ({ value: target(s.id, c.id), label: `${s.name} / ${c.name}`, schema: c }))),
    [file],
  );
  const [newTarget, setNewTarget] = useState(targets[0]?.value ?? "");
  const newTargetValue = targets.some((t) => t.value === newTarget) ? newTarget : (targets[0]?.value ?? "");

  // 編集中の設定。「適用」で初めてファイルに反映する
  const [draft, setDraft] = useState({ name: "", target: "", description: "", columns: [] as string[] });
  // 修正の設定など、下書きに関係ない項目が変わっても、編集中の下書きを捨てないよう、使う項目だけを鍵にする
  const appliedKey = JSON.stringify(selected ? [selected.name, selected.sheet, selected.schema, selected.description ?? "", selected.columns] : null);
  const applied = useMemo(
    () => ({
      name: selected?.name ?? "",
      target: selected ? target(selected.sheet, selected.schema) : "",
      description: selected?.description ?? "",
      columns: selected?.columns ?? [],
    }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [appliedKey],
  );
  useEffect(() => setDraft(applied), [applied]);
  const changed = JSON.stringify(draft) !== JSON.stringify(applied);

  // 配信の状態
  const [status, setStatus] = useState<FormsStatus>({ running: false, port: null, protected: false, urls: [] });
  const [port, setPort] = useState(String(DEFAULT_PORT));
  const [code, setCode] = useState("");
  const [personal, setPersonal] = useState("");
  const refreshStatus = useCallback(() => backend.formsStatus().then(setStatus, (e) => onError(String(e))), [backend, onError]);
  // フォームの増減・回答の到着で、URL 一覧と回答数を読み直す
  const formsKey = JSON.stringify(forms.map((f) => f.id));
  const totalRows = file.sheets.reduce((n, s) => n + s.schemas.reduce((m, c) => m + c.rows.length, 0), 0);
  useEffect(() => void refreshStatus(), [refreshStatus, formsKey, totalRows]);

  const [copied, setCopied] = useState<string | null>(null);
  const copy = async (formId: string, url: string) => {
    try {
      await navigator.clipboard.writeText(url);
      setCopied(formId);
      setTimeout(() => setCopied((c) => (c === formId ? null : c)), 1500);
    } catch {
      onError("コピーできませんでした。URL を選択してコピーしてください");
    }
  };

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
    const t = targets.find((x) => x.value === newTargetValue);
    if (!t) return onError("対象にする表（スキーマ）がありません");
    const [sheet, schema] = t.value.split("|");
    try {
      const snap = await backend.addForm(sheet, schema, `${t.schema.name}の入力`);
      onSnapshot(snap);
      setSelectedId(snap.file.forms[snap.file.forms.length - 1].id);
    } catch (e) {
      onError(String(e));
    }
  };

  // 修正できる期間。入力が終わったとき（フォーカスが外れる・Enter）に反映する
  const [editMinutes, setEditMinutes] = useState("");
  useEffect(() => setEditMinutes(selected?.edit?.minutes ? String(selected.edit.minutes) : ""), [selected?.id, selected?.edit?.minutes]);
  const commitMinutes = () => {
    if (!selected) return;
    const text = editMinutes.trim();
    const minutes = text === "" ? null : Number(text);
    if (minutes !== null && (!Number.isInteger(minutes) || minutes < 1 || minutes > 43200)) {
      onError("修正できる期間は 1〜43200 分（30 日）の整数にしてください");
      setEditMinutes(selected.edit?.minutes ? String(selected.edit.minutes) : "");
      return;
    }
    if (minutes === (selected.edit?.minutes ?? null)) return;
    void run(() => backend.setFormEdit(selected.id, selected.edit?.allowed !== false, minutes));
  };

  const apply = () => {
    if (!selected) return;
    const [sheet, schema] = draft.target.split("|");
    void run(() => backend.updateForm(selected.id, draft.name, sheet, schema, draft.columns, draft.description));
  };

  const start = async () => {
    const n = Number(port);
    if (!Number.isInteger(n) || n < 1 || n > 65535) return onError("ポートは 1〜65535 の整数にしてください");
    try {
      setStatus(await backend.formsStart(n, code.trim() || undefined, personal.split(/\r?\n/).map((l) => l.trim()).filter((l) => l)));
    } catch (e) {
      onError(String(e));
    }
  };
  const stop = async () => {
    try {
      setStatus(await backend.formsStop());
    } catch (e) {
      onError(String(e));
    }
  };

  // 対象の表の、入力欄にできる列。選択済みの列を先に（並び順どおり）、残りをその後ろに出す
  const draftSchema = targets.find((t) => t.value === draft.target)?.schema;
  const eligible = draftSchema?.columns.filter(canBeField) ?? [];
  const ordered = [
    ...draft.columns.map((id) => eligible.find((c) => c.id === id)).filter((c): c is NonNullable<typeof c> => !!c),
    ...eligible.filter((c) => !draft.columns.includes(c.id)),
  ];
  const toggle = (id: string, on: boolean) => setDraft({ ...draft, columns: on ? [...draft.columns, id] : draft.columns.filter((c) => c !== id) });
  const move = (id: string, d: -1 | 1) => {
    const cols = [...draft.columns];
    const i = cols.indexOf(id);
    const j = i + d;
    if (i < 0 || j < 0 || j >= cols.length) return;
    [cols[i], cols[j]] = [cols[j], cols[i]];
    setDraft({ ...draft, columns: cols });
  };
  const changeTarget = (value: string) => {
    const s = targets.find((t) => t.value === value)?.schema;
    // 表を変えたら、その表の入力できる列すべてを初期値にする
    setDraft({ ...draft, target: value, columns: s ? s.columns.filter(canBeField).map((c) => c.id) : [] });
  };

  return (
    <aside className="exports forms" aria-label="フォーム配信">
      <div className="export-body">
        <h4>配信</h4>
        <div className="serve-row">
          <label className="inline">
            ポート
            <input aria-label="ポート" className="port" value={port} disabled={status.running} onChange={(e) => setPort(e.target.value)} />
          </label>
          <label className="inline">
            合言葉（任意）
            <input aria-label="合言葉" className="code" value={code} disabled={status.running} placeholder="なし" onChange={(e) => setCode(e.target.value)} />
          </label>
          {status.running ? (
            <button onClick={stop}>配信を停止</button>
          ) : (
            <button className="primary" disabled={forms.length === 0} onClick={start} title={forms.length === 0 ? "先にフォームを作ってください" : undefined}>
              配信を開始
            </button>
          )}
          <span className="muted" role="status">
            {status.running ? `配信中（ポート ${status.port}${status.protected ? "・合言葉あり" : ""}）` : "停止中"}
          </span>
        </div>
        <label className="personal">
          回答者ごとの合言葉（任意。1 行に 1 つ・1 人に 1 つ配ります）
          <textarea
            aria-label="回答者ごとの合言葉"
            rows={3}
            value={personal}
            disabled={status.running}
            placeholder={"例:\nyamada-1234\nsato-5678"}
            onChange={(e) => setPersonal(e.target.value)}
          />
        </label>
        <p className="muted">
          回答者ごとの合言葉で入った人は、1 つの合言葉につき回答は 1 件で、送信後はその合言葉で（別の端末からも）直せます。全員共通の合言葉とは別のものにしてください。
        </p>
        <p className="muted warn">
          同じネットワークで URL を知っている人は誰でも（合言葉を決めたときは、合言葉も知っている人が）、回答を行として追加できます。
          合言葉は半角の英数字と記号（4〜64 文字）で、ファイルには保存しません。間違いや存在しない URL が続いた端末は数分間ロックされ、1 台からの回答は 1 分に 30 件までです。回答は開いているファイルに追加され、保存するまで確定しません。
          配信中はこのウィンドウ（アプリ）を開いたままにしてください。URL は配信を始めるたびに変わります。
        </p>
        {status.running && (
          <ul className="urls" aria-label="配信中の URL">
            {status.urls.map((u) => (
              <li key={u.formId}>
                <strong>{u.name}</strong>
                <span className="muted"> 回答 {u.submitted} 件</span>
                <div className="url-row">
                  <input readOnly aria-label={`${u.name} の URL`} value={u.url} onFocus={(e) => e.currentTarget.select()} />
                  <button onClick={() => void copy(u.formId, u.url)}>{copied === u.formId ? "コピーしました" : "コピー"}</button>
                </div>
              </li>
            ))}
          </ul>
        )}

        <h4>フォーム</h4>
        <div className="macro-bar flat">
          <select aria-label="フォーム" value={selected?.id ?? ""} onChange={(e) => setSelectedId(e.target.value)} disabled={forms.length === 0}>
            {forms.length === 0 && <option value="">（フォームなし）</option>}
            {forms.map((f) => (
              <option key={f.id} value={f.id}>
                {f.name}
              </option>
            ))}
          </select>
          <select aria-label="新しいフォームの対象の表" value={newTargetValue} onChange={(e) => setNewTarget(e.target.value)}>
            {targets.map((t) => (
              <option key={t.value} value={t.value}>
                {t.label}
              </option>
            ))}
          </select>
          <button onClick={add}>+ フォームを追加</button>
        </div>

        {!selected ? (
          <p className="muted">
            表の列を選んでフォームにすると、同じネットワークのスマホやパソコンのブラウザから、その表に行を入力してもらえます。
          </p>
        ) : (
          <div className="export-form">
            <label>
              名前（回答者にも見える題名）
              <input aria-label="フォームの名前" value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} />
            </label>
            <label>
              回答の追加先の表
              <select aria-label="回答の追加先の表" value={draft.target} onChange={(e) => changeTarget(e.target.value)}>
                {targets.map((t) => (
                  <option key={t.value} value={t.value}>
                    {t.label}
                  </option>
                ))}
              </select>
            </label>
            <label>
              説明（任意）
              <textarea aria-label="説明" rows={2} value={draft.description} onChange={(e) => setDraft({ ...draft, description: e.target.value })} />
            </label>
            <fieldset className="fields">
              <legend>入力欄（チェックした列が、上から順に並びます）</legend>
              {ordered.length === 0 && <p className="muted">入力欄にできる列がありません（計算列・ネスト型は使えません）。</p>}
              {ordered.map((c) => {
                const on = draft.columns.includes(c.id);
                return (
                  <div key={c.id} className="field-row">
                    <label className="check">
                      <input type="checkbox" checked={on} onChange={(e) => toggle(c.id, e.target.checked)} />
                      {c.name}
                      {c.required && <span className="req" title="必須">*</span>}
                      <span className="muted"> {c.type.kind}</span>
                    </label>
                    {on && (
                      <span className="mv">
                        <button aria-label={`${c.name} を上へ`} onClick={() => move(c.id, -1)} disabled={draft.columns[0] === c.id}>
                          ↑
                        </button>
                        <button aria-label={`${c.name} を下へ`} onClick={() => move(c.id, 1)} disabled={draft.columns[draft.columns.length - 1] === c.id}>
                          ↓
                        </button>
                      </span>
                    )}
                  </div>
                );
              })}
              {draftSchema?.columns.some((c) => c.required && canBeField(c) && !draft.columns.includes(c.id)) && (
                <p className="muted warn">必須の列が入力欄に入っていません。回答のたびに「必須です」で拒否されます。</p>
              )}
            </fieldset>
            <fieldset className="fields">
              <legend>送信後の修正（変えるとすぐ反映されます）</legend>
              <label className="check">
                <input
                  type="checkbox"
                  aria-label="回答者が送信後に修正できる"
                  checked={selected.edit?.allowed !== false}
                  onChange={(e) => void run(() => backend.setFormEdit(selected.id, e.target.checked, selected.edit?.minutes ?? null))}
                />
                回答者が、送信した内容をあとから直せる
              </label>
              {selected.edit?.allowed !== false && (
                <label>
                  修正できる期間（分。空なら期限なし）
                  <input
                    aria-label="修正できる期間（分）"
                    inputMode="numeric"
                    value={editMinutes}
                    onChange={(e) => setEditMinutes(e.target.value)}
                    onBlur={commitMinutes}
                    onKeyDown={(e) => e.key === "Enter" && commitMinutes()}
                    placeholder="例: 60（1 時間）、1440（1 日）"
                  />
                </label>
              )}
              <p className="muted">
                直せるのは、送信した端末だけです（送信の応答で渡す修正用の合い鍵を、その端末が覚えています）。配信を止めると、期限内でも直せなくなります。
              </p>
            </fieldset>
            <div className="actions left">
              <button className="primary" disabled={!changed} onClick={apply}>
                適用
              </button>
              {changed && <span className="muted">未適用の変更があります</span>}
              <span className="spacer" />
              <button
                onClick={() =>
                  onConfirm("フォームの削除", `フォーム「${selected.name}」を削除します。配信中なら URL も使えなくなります。よろしいですか？`, () =>
                    void run(() => backend.deleteForm(selected.id)),
                  )
                }
              >
                削除
              </button>
            </div>
          </div>
        )}
      </div>
    </aside>
  );
}
