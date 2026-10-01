import { useEffect, useMemo, useRef } from "react";
import { AgGridReact } from "ag-grid-react";
import { AllCommunityModule, ModuleRegistry, type CellEditRequestEvent, type ColDef, type ICellRendererParams } from "ag-grid-community";
import { formatValue, parseInput } from "../parse";
import type { CellResult, Column, DataSchema, Row } from "../types";

ModuleRegistry.registerModules([AllCommunityModule]);

interface Props {
  schema: DataSchema;
  /** この表の計算列の値（列 ID → 行 ID → 結果） */
  computed?: Record<string, Record<string, CellResult>>;
  onSetCell: (row: string, column: Column, value: unknown) => void;
  onDeleteRow: (row: string) => void;
  onError: (message: string) => void;
}

export function Grid({ schema, computed, onSetCell, onDeleteRow, onError }: Props) {
  const grid = useRef<AgGridReact<Row>>(null);
  // 計算列の値は valueGetter が参照している。行の追加などで結果だけが変わっても、AG Grid は
  // 既存・新規のセルの値を取り直さないことがあるので、結果が変わったら強制的に再描画する。
  useEffect(() => {
    grid.current?.api?.refreshCells({ force: true });
  }, [computed, schema.rows]);

  const columnDefs = useMemo<ColDef<Row>[]>(() => {
    // 計算列: 値は保存されておらず、状態と一緒に届く。編集はできない。失敗したセルだけ #ERROR にする。
    const result = (c: Column, row: Row | undefined): CellResult | undefined => (row ? computed?.[c.id]?.[row.id] : undefined);
    const cols: ColDef<Row>[] = schema.columns.map((c) => ({
      colId: c.id,
      headerName: c.computed ? `ƒ ${c.name}` : `${c.name}${c.required ? " *" : ""}`,
      headerTooltip: c.computed ? `${c.type.kind}（計算列）` : c.type.kind,
      editable: !c.computed,
      valueGetter: c.computed
        ? (p) => {
            const r = result(c, p.data);
            return !r ? "" : "e" in r ? "#ERROR" : formatValue(r.v);
          }
        : (p) => formatValue(p.data?.cells[c.id]),
      tooltipValueGetter: c.computed
        ? (p) => {
            const r = result(c, p.data);
            return r && "e" in r ? r.e : undefined;
          }
        : undefined,
      cellEditor: c.type.kind === "enum" ? "agSelectCellEditor" : undefined,
      cellEditorParams: c.type.kind === "enum" ? { values: ["", ...c.type.values] } : undefined,
      cellClass: (p) => {
        const numeric = c.type.kind === "int" || c.type.kind === "float" || c.type.kind === "decimal";
        const r = c.computed ? result(c, p.data) : undefined;
        return [numeric ? "num" : "", c.computed ? "computed" : "", r && "e" in r ? "cell-error" : ""].filter(Boolean);
      },
      minWidth: 120,
      flex: 1,
    }));
    return [
      {
        colId: "__delete",
        headerName: "",
        width: 64,
        suppressSizeToFit: true,
        cellStyle: { padding: 0, display: "flex", justifyContent: "center", alignItems: "center" },
        pinned: "left",
        editable: false,
        sortable: false,
        cellRenderer: (p: ICellRendererParams<Row>) => (
          <button className="row-delete" title="行を削除" onClick={() => p.data && onDeleteRow(p.data.id)}>
            ×
          </button>
        ),
      },
      ...cols,
    ];
  }, [schema.columns, computed, onDeleteRow]);

  // readOnlyEdit: グリッドは値を書き換えず、バックエンドの検証を通った結果で再描画する
  const onCellEditRequest = (e: CellEditRequestEvent<Row>) => {
    const column = schema.columns.find((c) => c.id === e.colDef.colId);
    if (!column || !e.data) return;
    const parsed = parseInput(column.type, String(e.newValue ?? ""));
    if (!parsed.ok) return onError(`${column.name}: ${parsed.error}`);
    if (JSON.stringify(parsed.value ?? null) === JSON.stringify(e.data.cells[column.id] ?? null)) return;
    onSetCell(e.data.id, column, parsed.value);
  };

  return (
    <div className="grid-wrap">
      <AgGridReact<Row>
        ref={grid}
        rowData={schema.rows}
        columnDefs={columnDefs}
        getRowId={(p) => p.data.id}
        readOnlyEdit
        onCellEditRequest={onCellEditRequest}
        singleClickEdit={false}
        stopEditingWhenCellsLoseFocus
        suppressMovableColumns
      />
    </div>
  );
}
