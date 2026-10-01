import { useMemo } from "react";
import { AgGridReact } from "ag-grid-react";
import { AllCommunityModule, ModuleRegistry, type CellEditRequestEvent, type ColDef, type ICellRendererParams } from "ag-grid-community";
import { formatValue, parseInput } from "../parse";
import type { Column, DataSchema, Row } from "../types";

ModuleRegistry.registerModules([AllCommunityModule]);

interface Props {
  schema: DataSchema;
  onSetCell: (row: string, column: Column, value: unknown) => void;
  onDeleteRow: (row: string) => void;
  onError: (message: string) => void;
}

export function Grid({ schema, onSetCell, onDeleteRow, onError }: Props) {
  const columnDefs = useMemo<ColDef<Row>[]>(() => {
    const cols: ColDef<Row>[] = schema.columns.map((c) => ({
      colId: c.id,
      headerName: `${c.name}${c.required ? " *" : ""}`,
      headerTooltip: c.type.kind,
      editable: true,
      valueGetter: (p) => formatValue(p.data?.cells[c.id]),
      cellEditor: c.type.kind === "enum" ? "agSelectCellEditor" : undefined,
      cellEditorParams: c.type.kind === "enum" ? { values: ["", ...c.type.values] } : undefined,
      cellClass: c.type.kind === "int" || c.type.kind === "float" || c.type.kind === "decimal" ? "num" : undefined,
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
  }, [schema.columns, onDeleteRow]);

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
