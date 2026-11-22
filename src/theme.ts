import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type Theme = "acrylic" | "stlr" | "dark" | "light";

const apply = (t: Theme) => {
  document.documentElement.dataset.theme = t;
};

/** Keeps `<html data-theme>` in sync with settings; switches live on the backend's `theme-changed` event. */
export function useTheme() {
  useEffect(() => {
    invoke<{ theme: Theme }>("get_settings").then((s) => apply(s.theme));
    const un = listen<Theme>("theme-changed", (e) => apply(e.payload));
    return () => {
      un.then((f) => f());
    };
  }, []);
}
