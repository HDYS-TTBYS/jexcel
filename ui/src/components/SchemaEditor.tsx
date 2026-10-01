import { lazy, Suspense, useState } from "react";
import { FORMULA_TEMPLATE, newId, type Column, type DataSchema } from "../types";
import { Modal } from "./Modal";
import { TypeEditor } from "./TypeEditor";

// Monaco は大きいので、計算式を編集するときに初めて読み込む
const FormulaEditor = lazy(() => import("./FormulaEditor"));

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
          <ColumnRow key={c.id} schemaId={schema.id} column={c} onUpdate={onUpdate} onDelete={onDelete} />
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

function ColumnRow({
  schemaId,
  column,
  onUpdate,
  onDelete,
}: {
  schemaId: string;
  column: Column;
  onUpdate: (c: Column) => void;
  onDelete: (id: string) => void;
}) {
  const [draft, setDraft] = useState(column);
  const changed = JSON.stringify(draft) !== JSON.stringify(column);
  return (
    <div className="formula-row">
      <div className="column-row">
        <input aria-label="列名" value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} />
        <TypeEditor value={draft.type} onChange={(type) => setDraft({ ...draft, type })} />
        <label title="値を保存せず、式から計算する列にします">
          <input
            type="checkbox"
            aria-label="計算列"
            checked={!!draft.computed}
            onChange={(e) =>
              setDraft(
                e.target.checked
                  ? { ...draft, required: undefined, computed: { source: draft.computed?.source ?? FORMULA_TEMPLATE } }
                  : { ...draft, computed: undefined },
              )
            }
          />
          計算列
        </label>
        {!draft.computed && (
          <label>
            <input type="checkbox" checked={!!draft.required} onChange={(e) => setDraft({ ...draft, required: e.target.checked })} />
            必須
          </label>
        )}
        <button className="primary" disabled={!changed} onClick={() => onUpdate(draft)}>
          適用
        </button>
        <button title="列を削除" onClick={() => onDelete(column.id)}>
          削除
        </button>
      </div>
      {draft.computed && (
        <>
          <p className="muted">
            計算式: 値はファイルに保存されず、式から計算されます。この列の結果は、右側の列の式や、マクロからも読めます。
          </p>
          <Suspense fallback={<p className="muted">エディタを読み込み中…</p>}>
            <FormulaEditor
              path={`file:///formula-${schemaId}-${column.id}.ts`}
              value={draft.computed.source ?? ""}
              onChange={(source) => setDraft((d) => ({ ...d, computed: { source } }))}
            />
          </Suspense>
        </>
      )}
    </div>
  );
}
