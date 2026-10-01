import { useState } from "react";
import { newId, type Column, type DataSchema } from "../types";
import { Modal } from "./Modal";
import { TypeEditor } from "./TypeEditor";

interface Props {
  schema: DataSchema;
  onClose: () => void;
  onUpdate: (c: Column) => void;
  onAdd: (c: Column) => void;
  onDelete: (columnId: string) => void;
}

export function SchemaEditor({ schema, onClose, onUpdate, onAdd, onDelete }: Props) {
  return (
    <Modal title={`スキーマ「${schema.name}」の列`} onClose={onClose} wide>
      <div className="column-list">
        {schema.columns.map((c) => (
          <ColumnRow key={c.id} column={c} onUpdate={onUpdate} onDelete={onDelete} />
        ))}
      </div>
      <div className="actions">
        <button onClick={() => onAdd({ id: newId(), name: `列${schema.columns.length + 1}`, type: { kind: "string" } })}>+ 列を追加</button>
        <button className="primary" onClick={onClose}>
          閉じる
        </button>
      </div>
    </Modal>
  );
}

function ColumnRow({ column, onUpdate, onDelete }: { column: Column; onUpdate: (c: Column) => void; onDelete: (id: string) => void }) {
  const [draft, setDraft] = useState(column);
  const changed = JSON.stringify(draft) !== JSON.stringify(column);
  return (
    <div className="column-row">
      <input aria-label="列名" value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} />
      <TypeEditor value={draft.type} onChange={(type) => setDraft({ ...draft, type })} />
      <label>
        <input type="checkbox" checked={!!draft.required} onChange={(e) => setDraft({ ...draft, required: e.target.checked })} />
        必須
      </label>
      <button className="primary" disabled={!changed} onClick={() => onUpdate(draft)}>
        適用
      </button>
      <button title="列を削除" onClick={() => onDelete(column.id)}>
        削除
      </button>
    </div>
  );
}
