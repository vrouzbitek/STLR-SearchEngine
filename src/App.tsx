import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import ActionIcon, { ActionKind, BADGE } from "./ActionIcon";
import { useTheme } from "./theme";

type Hit = {
  name: string;
  path: string;
  kind: Exclude<ActionKind, "search">;
  score: number;
  running: boolean;
  detail?: string | null; // right-hand label from the backend ("Edge Tab", "Chrome · github.com")
  icon?: string | null; // remote favicon (tabs)
  child?: boolean; // ↳ sub-action / recent project of the app right above it
};
type Expanded = { app: Hit; items: Hit[] };
// Need a second Enter within the same selection (Rust also refuses them without `confirmed`).
const DANGEROUS = ["sys:shutdown", "sys:restart", "sys:empty-bin"];
type Answer = { text: string; source: string; url: string };
type Local = { hits: Hit[]; ghost: string | null };
type Zero = { hits: Hit[]; paused: boolean };
type Stats = { links: number; queries: number; paused: boolean; top: { prefix: string; name: string; score: number }[] };
type Info = { title: string; detail?: string };
type SeedReport = { taskbar: number; open: number; startup: number; recent: number; total: number; rows: number };
const seedDetail = (r: SeedReport) =>
  `${r.total} items · ${r.taskbar} taskbar, ${r.open} open, ${r.startup} startup, ${r.recent} folders`;

const MAX = 8;
const WIDTH = 680;
const BAR_H = 72; // must match the Rust toggle() reset size
const ROW_H = 44;
const LIST_PAD = 9; // border-t 1px + py-1 8px
const CARD_H = 89; // border-t 1px + h-[88px]
const ANSWER_DEBOUNCE_MS = 250;
const FILE_KINDS = ["app", "folder", "file", "recent"]; // only these have native icons; others use SVG glyphs

// Instant answers: 2+ words or a question; never for commands, math or URLs.
const QUESTION = /^(kto|co|ile|jak|jaki|jaka|jakie|gdzie|kiedy|dlaczego|czy|what|who|how|when|where|why|which)\b|\?$/i;
const wantsAnswer = (q: string) =>
  !q.startsWith("/") && !/^[\d\s.,+\-*/^()%×÷]+$/.test(q) && (q.split(/\s+/).length >= 2 || QUESTION.test(q));

const COMMANDS: Info[] = [
  { title: "/settings", detail: "open settings" },
  { title: "/brain", detail: "stats" },
  { title: "/brain reset", detail: "wipe everything learned" },
  { title: "/brain reseed", detail: "rescan taskbar, open apps, startup, recent" },
  { title: "/brain pause", detail: "toggle read-only mode" },
  { title: "/brain resume", detail: "resume learning" },
];

const win = getCurrentWindow();
const hide = () => win.hide();
const parentDir = (p: string) => p.slice(0, Math.max(p.lastIndexOf("\\"), 0));

export default function App() {
  useTheme();
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<Hit[]>([]);
  const [zero, setZero] = useState<Hit[]>([]);
  const [info, setInfo] = useState<Info[]>([]);
  const [flash, setFlash] = useState<Info | null>(null);
  const [ghostFull, setGhostFull] = useState("");
  const [paused, setPaused] = useState(false);
  const [sel, setSel] = useState(0); // -1 = answer card
  const [answer, setAnswer] = useState<Answer | null>(null);
  const [armed, setArmed] = useState<string | null>(null); // path of a dangerous command awaiting 2nd Enter
  const [expanded, setExpanded] = useState<Expanded | null>(null); // ArrowRight on an app: its full sub-list
  const [icons, setIcons] = useState<Record<string, string>>({});
  const requested = useRef(new Set<string>());
  const reqId = useRef(0);
  const answerId = useRef(0);
  const topKind = useRef<string | undefined>(undefined);
  const inputRef = useRef<HTMLInputElement>(null);
  // WebView fires mousemove when rows appear under a resting cursor: only a real move (new position) may select.
  const mouse = useRef<string | null>(null);
  const hover = (e: { screenX: number; screenY: number }, i: number) => {
    const p = `${e.screenX},${e.screenY}`;
    const moved = mouse.current !== null && mouse.current !== p;
    mouse.current = p;
    if (moved) setSel(i);
  };
  // Abandon & retype tracking: longest query of the current streak, and the last one given up.
  const peak = useRef("");
  const abandoned = useRef<{ text: string; t: number } | null>(null);

  const cmd = query.trim().toLowerCase();
  const isCmd = cmd.startsWith("/");
  const list: Hit[] = isCmd ? [] : expanded ? [expanded.app, ...expanded.items] : query ? hits : zero;
  const infoRows = isCmd ? (flash ? [flash, ...info] : info) : [];
  const rowCount = isCmd ? infoRows.length : list.length;
  const card = !isCmd && query ? answer : null;
  topKind.current = hits[0]?.kind;

  // Local (apps + brain) renders immediately; Everything results merge in when they arrive. Stale replies are dropped.
  useEffect(() => setArmed(null), [query, sel]);

  useEffect(() => {
    const id = ++reqId.current;
    setSel(0);
    setExpanded(null);
    if (!query.trim()) {
      setHits([]);
      setGhostFull("");
      return;
    }
    if (isCmd) {
      if (cmd === "/brain") {
        invoke<Stats>("brain_stats").then((s) => {
          if (id !== reqId.current) return;
          setPaused(s.paused);
          setInfo([
            { title: `${s.links} learned links · ${s.queries} queries`, detail: s.paused ? "PAUSED" : "learning" },
            ...s.top.map((t) => ({ title: `${t.prefix} → ${t.name}`, detail: t.score.toFixed(2) })),
            ...COMMANDS.filter((c) => c.title.startsWith("/brain ")),
          ]);
        });
      } else {
        setInfo(COMMANDS.filter((c) => c.title.startsWith(cmd)));
      }
      return;
    }
    const local = invoke<Local>("search_local", { query });
    local.then((l) => {
      if (id !== reqId.current) return;
      setHits(l.hits);
      setGhostFull(l.ghost ?? "");
    });
    Promise.all([local, invoke<Hit[]>("search_files", { query })]).then(([l, f]) => {
      if (id !== reqId.current) return;
      const seen = new Set(l.hits.map((h) => h.path.toLowerCase()));
      // Stable sort on the shared score: local results win ties. Children stay glued under their app.
      const kids = l.hits.filter((h) => h.child);
      const merged = [...l.hits.filter((h) => !h.child), ...f.filter((h) => !seen.has(h.path.toLowerCase()))]
        .sort((x, y) => x.score - y.score)
        .slice(0, MAX);
      if (kids.length && merged[0]?.path === l.hits[0]?.path) merged.splice(1, 0, ...kids);
      setHits(merged);
    });
  }, [query]);

  // Instant answer: debounced, async, never blocks local results; no answer -> no card.
  useEffect(() => {
    const id = ++answerId.current;
    setAnswer(null);
    const q = query.trim();
    if (!wantsAnswer(q)) return;
    const t = setTimeout(() => {
      if (topKind.current === "calc") return;
      invoke<Answer | null>("instant_answer", { query: q }).then((a) => id === answerId.current && setAnswer(a));
    }, ANSWER_DEBOUNCE_MS);
    return () => clearTimeout(t);
  }, [query]);

  // Icons load off the hot path; backend caches them, this ref avoids re-asking.
  useEffect(() => {
    for (const h of list) {
      if (!FILE_KINDS.includes(h.kind) || requested.current.has(h.path)) continue;
      requested.current.add(h.path);
      invoke<string | null>("get_icon", { path: h.path, kind: h.kind }).then(
        (src) => src && setIcons((m) => ({ ...m, [h.path]: src })),
      );
    }
  }, [list]);

  useEffect(() => {
    win.setSize(new LogicalSize(WIDTH, BAR_H + (card ? CARD_H : 0) + (rowCount ? LIST_PAD + rowCount * ROW_H : 0)));
  }, [rowCount, !!card]);

  useEffect(() => {
    const un = listen("focus-input", () => {
      setQuery("");
      setSel(0);
      setFlash(null);
      mouse.current = null;
      peak.current = "";
      abandoned.current = null;
      inputRef.current?.focus();
      invoke<Zero>("zero_query").then((z) => {
        setZero(z.hits);
        setPaused(z.paused);
      });
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  const onChange = (next: string) => {
    const prev = query;
    if (prev && !next.startsWith(prev) && (next === "" || !prev.startsWith(next))) {
      // Cleared or replaced (not a typo fix): remember what was given up.
      abandoned.current = { text: peak.current || prev, t: Date.now() };
      peak.current = next;
    } else if (next.length >= peak.current.length || !peak.current.startsWith(next)) {
      peak.current = next;
    }
    if (!next.trim().startsWith("/")) setFlash(null);
    setQuery(next);
  };

  const current = list[Math.max(0, Math.min(sel, list.length - 1))];
  const top = hits[0];
  const lower = query.toLowerCase();
  // History ghost first (most frequent committed query), else the #1 result's name.
  const ghostSrc =
    ghostFull && ghostFull.startsWith(lower)
      ? ghostFull
      : top && top.name.toLowerCase().startsWith(lower)
        ? top.name
        : "";
  const ghost = query && !isCmd && ghostSrc.length > query.length ? ghostSrc.slice(query.length) : "";

  const done = () => {
    setQuery("");
    hide();
  };

  // Hide only AFTER the launch: Windows lets only the foreground process hand the foreground to another app
  // (switch to Edge, browser focusing a tab). Hidden first, the switch fails and the app just flashes.
  const open = (path: string, kind: string) =>
    invoke("launch", { path, kind })
      .catch((e) => console.error("launch failed:", e))
      .finally(done);

  const copy = (text: string) =>
    invoke("copy_text", { text })
      .catch((e) => console.error("copy failed:", e))
      .finally(done);

  const run = (i: number) => {
    const h = list[i];
    if (!h) return;
    if (h.kind === "calc") return copy(h.name); // ephemeral: never learned
    if (h.kind === "system") {
      const dangerous = DANGEROUS.includes(h.path);
      if (dangerous && armed !== h.path) return setArmed(h.path); // first Enter only arms it
      done(); // hide first so e.g. the lock screen isn't fighting our window
      invoke("run_system", { path: h.path, confirmed: dangerous }).catch((e) => console.error("system failed:", e));
      return;
    }
    if (h.kind === "tab") return open(h.path, h.kind);
    const ab = abandoned.current;
    // Skip penalties only among peers: a ↳ pick ranks against its siblings (never penalizes the app itself),
    // a top-level pick ranks against top-level results (never the ↳ rows).
    const peers = list.filter((x) => !!x.child === !!h.child);
    // The only path that writes to the brain: an explicit pick.
    invoke("commit", {
      commit: {
        query,
        picked: peers.indexOf(h),
        shown: peers.map((x) => ({ id: x.path, name: x.name, kind: x.kind })),
        abandoned: ab?.text ?? null,
        abandonedAgeMs: ab ? Date.now() - ab.t : 0,
      },
    }).catch((e) => console.error("commit failed:", e));
    open(h.path, h.kind);
  };

  const openSource = (a: Answer) => open(a.url, "url");

  const runCommand = async () => {
    if (cmd === "/settings") {
      setQuery("");
      return invoke("open_settings");
    }
    if (cmd === "/brain reset") {
      const r = await invoke<SeedReport>("brain_reset");
      setZero([]);
      setFlash({ title: "✓ Brain reset & reseeded", detail: r.rows ? seedDetail(r) : "paused: not reseeded" });
    } else if (cmd === "/brain reseed") {
      const r = await invoke<SeedReport>("brain_reseed");
      setFlash({
        title: paused ? "Brain is paused: nothing written" : `✓ Reseeded: ${r.rows} new rows`,
        detail: seedDetail(r),
      });
    } else if (cmd === "/brain pause" || cmd === "/brain resume") {
      const p = await invoke<boolean>("brain_pause", { paused: cmd === "/brain pause" ? null : false });
      setPaused(p);
      setFlash({ title: p ? "⏸ Learning paused" : "▶ Learning resumed", detail: p ? "read-only" : "" });
    } else {
      return;
    }
    setQuery("/brain"); // show fresh stats under the confirmation
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      hide();
    } else if (e.key === "Tab") {
      e.preventDefault();
      if (ghost) onChange(query + ghost);
    } else if (e.key === "Enter") {
      if (isCmd) runCommand();
      else if (card && (sel < 0 || !list.length)) {
        if (e.ctrlKey) openSource(card);
        else copy(card.text);
      } else run(Math.min(sel, list.length - 1));
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setSel((s) => Math.min(s + 1, list.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSel((s) => Math.max(s - 1, card ? -1 : 0));
    } else if (e.key === "ArrowRight" && !expanded && current?.kind === "app") {
      // Only when the caret is at the end, so ArrowRight still moves the cursor while editing.
      const el = inputRef.current;
      if (el && el.selectionStart !== el.value.length) return;
      e.preventDefault();
      const app = current;
      invoke<Hit[]>("subactions", { path: app.path, query }).then((items) => {
        if (!items.length) return;
        setExpanded({ app, items });
        setSel(1);
      });
    } else if (e.key === "ArrowLeft" && expanded) {
      e.preventDefault();
      setExpanded(null);
      setSel(0);
    }
  };

  // Shared by input and ghost layer so both sit on the same baseline (font comes from the theme).
  const line = "m-0 p-0 border-0 h-8 text-2xl leading-8 font-light";
  return (
    // Fills the whole window: native DWM (Acrylic / corners) sits exactly behind this container.
    <div className="h-full flex flex-col overflow-hidden rounded-[var(--radius)] border border-[var(--line)] bg-[var(--bg)] text-[var(--fg)]">
      <div className="h-[70px] shrink-0 flex items-center gap-3 px-5">
        <ActionIcon kind={current?.kind ?? "search"} src={current && (icons[current.path] ?? current.icon ?? undefined)} />
        <div className="relative flex-1">
          <div aria-hidden className={`absolute inset-0 whitespace-pre pointer-events-none ${line}`}>
            <span className="invisible">{query}</span>
            <span className="text-[var(--faint)]">{ghost}</span>
          </div>
          <input
            ref={inputRef}
            autoFocus
            spellCheck={false}
            autoComplete="off"
            value={query}
            onChange={(e) => onChange(e.target.value)}
            onKeyDown={onKeyDown}
            placeholder="Search..."
            className={`relative block w-full bg-transparent outline-none text-[var(--fg)] placeholder:text-[var(--faint)] caret-[var(--fg)] ${line}`}
          />
        </div>
        {paused && (
          <span className="flex items-center gap-1.5">
            <span className="dot dot-warn" />
            <span className="chip chip-warn">Paused</span>
          </span>
        )}
        {query && !isCmd && <span className="chip">{BADGE[current?.kind ?? "search"]}</span>}
        {current?.kind === "app" && !expanded && list.some((h) => h.child) && (
          <span className="shrink-0 text-[11px] text-[var(--faint)]" title="ArrowRight: all actions">→</span>
        )}
      </div>
      {card && (
        <div
          onMouseMove={(e) => hover(e, -1)}
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => copy(card.text)}
          className={`h-[88px] shrink-0 border-t border-[var(--line)] px-5 py-2.5 flex gap-3 cursor-default ${sel < 0 ? "bg-[var(--sel)]" : ""}`}
        >
          <p className="flex-1 text-[13px] leading-5 line-clamp-3">{card.text}</p>
          <button
            onClick={(e) => {
              e.stopPropagation();
              openSource(card);
            }}
            className="self-start shrink-0 text-[11px] text-[var(--muted)] hover:text-[var(--fg)]"
            title="Open source (Ctrl+Enter)"
          >
            {card.source} ↗
          </button>
        </div>
      )}
      {rowCount > 0 && (
        <ul className="border-t border-[var(--line)] py-1">
          {isCmd
            ? infoRows.map((r, i) => (
                <li key={i} className="h-11 flex items-center gap-3 px-5">
                  <span className="flex-1 truncate text-sm">{r.title}</span>
                  <span className="max-w-[45%] truncate text-xs text-[var(--muted)]">{r.detail}</span>
                </li>
              ))
            : list.map((h, i) => (
                <li
                  key={h.path}
                  onMouseMove={(e) => hover(e, i)}
                  onMouseDown={(e) => e.preventDefault()} // keep input focus
                  onClick={() => run(i)}
                  className={`h-11 flex items-center gap-3 pr-5 cursor-default ${h.child ? "pl-10" : "pl-5"} ${i === sel ? "bg-[var(--sel)]" : ""}`}
                >
                  {h.child && <span className="-mr-1 text-sm text-[var(--faint)]">↳</span>}
                  <ActionIcon kind={h.kind} src={icons[h.path] ?? h.icon ?? undefined} />
                  <span className={`flex-1 truncate text-sm ${h.child ? "text-[var(--muted)]" : ""}`}>{h.name}</span>
                  {h.child ? (
                    <span className={`chip ${h.kind === "action" ? "chip-action" : ""}`}>{BADGE[h.kind]}</span>
                  ) : armed === h.path ? (
                    <span className="flex items-center gap-1.5">
                      <span className="dot dot-warn" />
                      <span className="chip chip-warn">Press Enter again to confirm</span>
                    </span>
                  ) : h.running ? (
                    <span className="flex items-center gap-1.5">
                      <span className="dot" />
                      <span className="chip">Switch</span>
                    </span>
                  ) : h.kind === "tab" ? (
                    <span className="chip">{h.detail ?? "Tab"}</span>
                  ) : (
                    <span className="max-w-[45%] truncate text-xs text-[var(--muted)]">
                      {h.detail
                        ? h.detail
                        : h.kind === "calc"
                          ? "Enter to copy"
                          : h.kind === "folder" || h.kind === "file"
                            ? parentDir(h.path)
                            : BADGE[h.kind]}
                    </span>
                  )}
                </li>
              ))}
        </ul>
      )}
    </div>
  );
}
