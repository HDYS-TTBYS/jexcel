import { ALL_KINDS, KIND_LABEL, defaultType, newId, type Column, type DataType, type Kind } from "../types";

/** データ型の編集。array / object は再帰的にネストできる。 */
export function TypeEditor({ value, onChange }: { value: DataType; onChange: (t: DataType) => void }) {
  return (
    <span className="type-editor">
      <select
        aria-label="型"
        value={value.kind}
        onChange={(e) => onChange(defaultType(e.target.value as Kind))}
      >
        {ALL_KINDS.map((k) => (
          <option key={k} value={k}>
            {KIND_LABEL[k]}
          </option>
        ))}
        {value.kind === "custom" && <option value="custom">{KIND_LABEL.custom}</option>}
      </select>
      {value.kind === "enum" && (
        <input
          aria-label="選択肢（カンマ区切り）"
          placeholder="選択肢をカンマ区切りで"
          defaultValue={value.values.join(",")}
          onBlur={(e) =>
            onChange({ kind: "enum", values: e.target.value.split(",").map((s) => s.trim()).filter(Boolean) })
          }
        />
      )}
      {value.kind === "array" && (
        <span className="nested">
          要素: <TypeEditor value={value.item} onChange={(item) => onChange({ kind: "array", item })} />
        </span>
      )}
      {value.kind === "object" && <FieldsEditor fields={value.fields} onChange={(fields) => onChange({ kind: "object", fields })} />}
    </span>
  );
}

function FieldsEditor({ fields, onChange }: { fields: Column[]; onChange: (f: Column[]) => void }) {
  const set = (i: number, patch: Partial<Column>) => onChange(fields.map((f, j) => (j === i ? { ...f, ...patch } : f)));
  return (
    <div className="nested fields">
      {fields.map((f, i) => (
        <div key={f.id} className="field-row">
          <input aria-label="フィールド名" value={f.name} onChange={(e) => set(i, { name: e.target.value })} />
          <TypeEditor value={f.type} onChange={(type) => set(i, { type })} />
          <label>
            <input type="checkbox" checked={!!f.required} onChange={(e) => set(i, { required: e.target.checked })} />
            必須
          </label>
          <button title="フィールドを削除" onClick={() => onChange(fields.filter((_, j) => j !== i))}>
            ×
          </button>
        </div>
      ))}
      <button onClick={() => onChange([...fields, { id: newId(), name: `フィールド${fields.length + 1}`, type: { kind: "string" } }])}>
        + フィールド
      </button>
    </div>
  );
}
