import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { AppShell } from "./shell/AppShell";
import "./styles.css";

const container = document.getElementById("root");
if (container === null) {
  // The host loads a document we ship; a missing mount point is a
  // packaging defect, and a blank window would hide it.
  throw new Error("index.html is missing its #root mount point");
}

createRoot(container).render(
  <StrictMode>
    <AppShell />
  </StrictMode>,
);
