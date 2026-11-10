import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import "./styles.css";

// The webview's own menu (Back, Refresh, Save as…) has no place in a desktop app. Editable
// fields keep theirs for cut, copy and paste; the app's own menus call preventDefault first.
if (!import.meta.env.DEV) {
  document.addEventListener("contextmenu", (event) => {
    const target = event.target as HTMLElement | null;
    if (target?.closest("input, textarea, [contenteditable='true']")) return;
    event.preventDefault();
  });
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
