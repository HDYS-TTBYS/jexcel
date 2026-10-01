import Editor from "@monaco-editor/react";
import "../monacoSetup";

/** 計算列の式のエディタ。マクロと同じ Monaco で、jx / row の型が効く。 */
export default function FormulaEditor({ path, value, onChange }: { path: string; value: string; onChange: (v: string) => void }) {
  return (
    <div className="formula-editor">
      <Editor
        height="100%"
        language="typescript"
        path={path}
        value={value}
        onChange={(v) => onChange(v ?? "")}
        options={{ minimap: { enabled: false }, fontSize: 13, automaticLayout: true, scrollBeyondLastLine: false, tabSize: 2, lineNumbers: "off" }}
      />
    </div>
  );
}
