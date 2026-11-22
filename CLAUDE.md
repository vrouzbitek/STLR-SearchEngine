# STLR-SearchEngine — Architecture & Workflow Rules

## Goal
Build an ultra-fast (<5ms local latency), keyboard-first, self-learning Windows launcher in Tauri v2 (Rust backend + React/TypeScript/Tailwind frontend).

## Core UX
- Borderless, frameless, transparent, always-on-top floating search bar centered on screen.
- Toggled via global shortcut (`Alt+Space`), immediately focusing the input. `Esc` or losing focus hides the window.
- Keep the window hidden in RAM (never destroy/recreate it) for instant 0ms toggle.
- Inline Ghost Text (grayed-out autocomplete directly after cursor) predicted from local usage history.
- `Tab` accepts ghost text. `Enter` executes the #1 result immediately. `ArrowUp`/`ArrowDown` navigates.

## Future Architecture (Phases 2–4)
- Local file search via Everything SDK (voidtools) + Start Menu app indexing.
- Local SQLite (`rusqlite`) brain: Frecency + Skip Penalty (selecting item #N penalizes #1..#N-1) + Abandon/Retype linking (queries within 5s) + active window context (`GetForegroundWindow`).
- Async web/app routing (Spotify URI, YouTube, Google) and inline Instant Answers without ever blocking local <5ms results.

## Workflow & Model Escalation Rules
- Work in small, testable phases. Never rewrite working modules unless explicitly asked.
- **When to tell the user to switch to Opus (High effort):**
  1. Before implementing native Windows API / Everything SDK FFI bindings in Rust.
  2. Before designing the SQLite ranking algorithm (Frecency + Skip Penalty + session linking).
  3. Immediately if a compiler error or bug is not solved within 2 attempts — stop, explain the blocker, and ask the user to switch to Opus + High effort.
