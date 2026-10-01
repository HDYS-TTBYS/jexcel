import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { createBackend } from "./backend";
import "./styles.css";

createBackend().then((backend) => {
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <App backend={backend} />
    </StrictMode>,
  );
});
