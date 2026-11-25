# STLR-SearchEngine — Architecture & Workflow Rules

## Goal
An ultra-fast (**<1 ms local results per keystroke**), keyboard-first, self-learning Windows 11 launcher in Tauri v2 (Rust backend + React/TypeScript/Tailwind frontend).

## Core UX
- Borderless, frameless, transparent, always-on-top floating search bar centered on screen.
- Toggled via global shortcut (`Alt+Space` by default, configurable), immediately focusing the input. `Esc` or losing focus hides the window.
- Keep the window hidden in RAM (never destroy/recreate it) for an instant toggle.
- Inline Ghost Text (grayed-out autocomplete directly after cursor) predicted from local usage history.
- `Tab` accepts ghost text. `Enter` executes the #1 result immediately. `ArrowUp`/`ArrowDown` navigates.

## Performance rules
- Local results must stay **under 1 ms per keystroke** (currently ~0.25 ms). Everything on the keystroke path lives in RAM.
- Network work (answers, LLM) is always async and must never block or delay local results.

## Status: shipped (phases 1–4)
The README `## Architecture` section is the current module map; check it before changing code.
- **Phase 1 — Launcher shell:** floating bar, global shortcut, ghost text, keyboard navigation, themes.
- **Phase 2 — Local search:** Start Menu app index + file search via Everything over native `WM_COPYDATA` IPC (`everything.rs`, no SDK DLL).
- **Phase 3 — SQLite brain (`brain.rs`):** WAL SQLite mirrored in RAM: frecency, skip penalty, abandon/retype linking, foreground-window context, zero-query, cold-start seeding.
- **Phase 4 — Answers & updates:** async web routes and instant answers (`answers.rs`, `routes.rs`, `calc.rs`), plus the in-app auto-updater (`tauri-plugin-updater`, signed GitHub Releases + `latest.json`).

## Conventions
- Commits and code in English, with prefixes `feat:` / `fix:` / `perf:` / `chore:` / `docs:`.
- **New system commands get both PL and EN aliases** (see `commands.rs`, e.g. `lock` / `zablokuj`), including diacritic-free PL variants (`uśpij` / `uspij`).

## Testing
- After any Rust change, run `cargo test` in `src-tauri/`.
- Live-system tests (`#[ignore]`) and builds (`npm run tauri build`, `scripts/smoke-release.ps1`) run on Vrouz's Windows PC, not in the cloud.

## Releases
- **Only Vrouz releases, locally**, with `powershell -File scripts/release.ps1 -Version X.Y.Z`. It bumps the version, makes a signed build, smoke-tests it and publishes the GitHub Release + `latest.json` for the auto-updater.
- The updater signing key (`~/.tauri/stlr-updater.key`) **never goes into the repo or the cloud**. Losing it means installed copies can no longer receive updates.
- Threads and cloud sessions prepare a PR; they never bump the version, tag, or publish a release.

## Workflow & Model Escalation Rules
- Work in small, testable phases. Never rewrite working modules unless explicitly asked.
- **When to tell the user to switch to Opus (High effort):**
  1. Before working on native Windows API / FFI / IPC bindings in Rust (WinAPI, Everything IPC, shell/COM).
  2. Before changing the ranking algorithm (frecency, skip penalty, retype linking, context weighting).
  3. Immediately if a compiler error or bug is not solved within 2 attempts — stop, explain the blocker, and ask the user to switch to Opus + High effort.
