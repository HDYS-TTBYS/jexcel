import { useEffect, useState } from "react";
import type { Backend } from "../backend";
import { describeChange } from "../describe";
import type { Change, CommitInfo, JxcelFile } from "../types";

interface Props {
  backend: Backend;
  file: JxcelFile;
  refreshKey: number;
  onRestore: (rev: string) => void;
  onError: (message: string) => void;
}

const fmt = (t: number) => new Date(t * 1000).toLocaleString("ja-JP");

export function HistoryPanel({ backend, file, refreshKey, onRestore, onError }: Props) {
  const [log, setLog] = useState<CommitInfo[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [changes, setChanges] = useState<Change[] | null>(null);

  useEffect(() => {
    backend.historyLog().then((l) => {
      setLog(l);
      setSelected(l[0]?.id ?? null);
    }, (e) => onError(String(e)));
  }, [backend, refreshKey, onError]);

  // 選択したコミットを 1 つ前のコミットと比べる
  useEffect(() => {
    const i = log.findIndex((c) => c.id === selected);
    if (i < 0) return setChanges(null);
    if (i === log.length - 1) return setChanges([]);
    backend.historyDiff(log[i + 1].id, log[i].id).then(setChanges, (e) => onError(String(e)));
  }, [backend, log, selected, onError]);

  const isOldest = log.length > 0 && selected === log[log.length - 1].id;

  return (
    <aside className="history" aria-label="履歴">
      <h3>履歴</h3>
      {log.length === 0 && <p className="muted">まだ保存されていません。保存すると履歴が始まります。</p>}
      <ul className="commits">
        {log.map((c) => (
          <li key={c.id} className={c.id === selected ? "active" : ""} onClick={() => setSelected(c.id)}>
            <div className="msg">{c.message}</div>
            <div className="muted">{fmt(c.time)}</div>
          </li>
        ))}
      </ul>
      {selected && (
        <div className="changes">
          <h4>変更内容</h4>
          {isOldest && <p className="muted">最初の保存です。</p>}
          {changes?.length === 0 && !isOldest && <p className="muted">変更はありません。</p>}
          <ul>
            {changes?.map((c, i) => (
              <li key={i} className={c.kind}>
                {describeChange(c, file)}
              </li>
            ))}
          </ul>
          <button onClick={() => onRestore(selected)}>この時点に戻す</button>
        </div>
      )}
    </aside>
  );
}
