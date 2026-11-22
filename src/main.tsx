import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import App from "./App";
import Settings from "./Settings";
import Wizard from "./Wizard";
import "./index.css";

// One bundle, three windows: the search bar ("main"), settings, and the first-run wizard.
const label = getCurrentWindow().label;
const View = label === "settings" ? Settings : label === "wizard" ? Wizard : App;

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <View />
  </React.StrictMode>,
);
