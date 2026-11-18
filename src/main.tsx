import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "@fontsource-variable/geist";
import "./index.css";
import { initTheme } from "./lib/theme";
import { warmFonts } from "./lib/warm";

// Before the first render, so the card never flashes the wrong palette.
initTheme();
// The window starts hidden; get the font ready before the first show, not at its first paint.
void warmFonts();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
