// Monaco をローカルのバンドルから使う（CDN には依存しない。オフラインのデスクトップアプリなので）。
import * as monaco from "monaco-editor";
import editorWorker from "monaco-editor/editor/editor.worker?worker";
import tsWorker from "monaco-editor/language/typescript/ts.worker?worker";
import { loader } from "@monaco-editor/react";
import jxcelApi from "./jxcelApi.d.ts.txt?raw";

self.MonacoEnvironment = {
  getWorker: (_id: string, label: string) =>
    label === "typescript" || label === "javascript" ? new tsWorker() : new editorWorker(),
};

loader.config({ monaco });

const ts = monaco.typescript.typescriptDefaults;
ts.setCompilerOptions({
  target: monaco.typescript.ScriptTarget.ES2020,
  module: monaco.typescript.ModuleKind.ESNext,
  strict: true,
  allowNonTsExtensions: true,
  noEmit: true,
});
// マクロからは jx（Jxcel 型）だけが見える。ファイルやネットワークの API は実行環境にないので補完にも出さない
ts.addExtraLib(jxcelApi, "file:///jxcel-api.d.ts");

// 開発時だけ、ブラウザのテストからエディタの内容を設定できるように公開する
if (import.meta.env.DEV) (window as unknown as { monaco: typeof monaco }).monaco = monaco;
