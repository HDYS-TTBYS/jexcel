import { useEffect, useRef, useState, type ReactNode } from "react";

export function Modal({ title, onClose, children, wide }: { title: string; onClose: () => void; children: ReactNode; wide?: boolean }) {
  useEffect(() => {
    const h = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [onClose]);
  return (
    <div className="backdrop" onMouseDown={onClose}>
      <div className={wide ? "modal wide" : "modal"} role="dialog" aria-label={title} onMouseDown={(e) => e.stopPropagation()}>
        <h2>{title}</h2>
        {children}
      </div>
    </div>
  );
}

// WebView によっては window.prompt / confirm が使えないため、自前のダイアログを使う。
export type DialogSpec =
  | { kind: "prompt"; title: string; initial: string; onOk: (value: string) => void }
  | { kind: "confirm"; title: string; message: string; onOk: () => void };

export function Dialog({ spec, onClose }: { spec: DialogSpec; onClose: () => void }) {
  const [value, setValue] = useState(spec.kind === "prompt" ? spec.initial : "");
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => ref.current?.select(), []);
  const ok = () => {
    if (spec.kind === "prompt") {
      if (!value.trim()) return;
      spec.onOk(value.trim());
    } else spec.onOk();
    onClose();
  };
  return (
    <Modal title={spec.title} onClose={onClose}>
      {spec.kind === "prompt" ? (
        <input ref={ref} value={value} autoFocus onChange={(e) => setValue(e.target.value)} onKeyDown={(e) => e.key === "Enter" && ok()} />
      ) : (
        <p>{spec.message}</p>
      )}
      <div className="actions">
        <button onClick={onClose}>キャンセル</button>
        <button className="primary" onClick={ok}>
          OK
        </button>
      </div>
    </Modal>
  );
}
