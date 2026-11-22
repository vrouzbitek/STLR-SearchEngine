import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import App from "./App";
import Settings from "./Settings";
import "./index.css";

// One bundle, two windows: the search bar ("main") and the settings window.
const isSettings = getCurrentWindow().label === "settings";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>{isSettings ? <Settings /> : <App />}</React.StrictMode>,
);
