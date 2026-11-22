import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import ActionIcon, { ActionKind, BADGE } from "./ActionIcon";

type Hit = { name: string; path: string; kind: Exclude<ActionKind, "search">; score: number; running: boolean };
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
const FILE_KINDS = ["app", "folder", "file"]; // only these have native icons; web/calc use SVG glyphs

// Instant answers: 2+ words or a question; never for commands, math or URLs.
const QUESTION = /^(kto|co|ile|jak|jaki|jaka|jakie|gdzie|kiedy|dlaczego|czy|what|who|how|when|where|why|which)\b|\?$/i;
const wantsAnswer = (q: string) =>
  !q.startsWith("/") && !/^[\d\s.,+\-*/^()%×÷]+$/.test(q) && (q.split(/\s+/).length >= 2 || QUESTION.test(q));

const COMMANDS: Info[] = [
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
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<Hit[]>([]);
  const [zero, setZero] = useState<Hit[]>([]);
  const [info, setInfo] = useState<Info[]>([]);
  const [flash, setFlash] = useState<Info | null>(null);
  const [ghostFull, setGhostFull] = useState("");
  const [paused, setPaused] = useState(false);
  const [sel, setSel] = useState(0); // -1 = answer card
  const [answer, setAnswer] = useState<Answer | null>(null);
  const [icons, setIcons] = useState<Record<string, string>>({});
  const requested = useRef(new Set<string>());
  const reqId = useRef(0);
  const answerId = useRef(0);
  const topKind = useRef<string | undefined>(undefined);
  const inputRef = useRef<HTMLInputElement>(null);
  // Abandon & retype tracking: longest query of the current streak, and the last one given up.
  const peak = useRef("");
  const abandoned = useRef<{ text: string; t: number } | null>(null);

  const cmd = query.trim().toLowerCase();
  const isCmd = cmd.startsWith("/");
  const list: Hit[] = isCmd ? [] : query ? hits : zero;
  const infoRows = isCmd ? (flash ? [flash, ...info] : info) : [];
  const rowCount = isCmd ? infoRows.length : list.length;
  const card = !isCmd && query ? answer : null;
  topKind.current = hits[0]?.kind;

  // Local (apps + brain) renders immediately; Everything results merge in when they arrive. Stale replies are dropped.
  useEffect(() => {
    const id = ++reqId.current;
    setSel(0);
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
            ...COMMANDS.slice(1),
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
      // Stable sort on the shared score: local results win ties.
      const merged = [...l.hits, ...f.filter((h) => !seen.has(h.path.toLowerCase()))].sort((x, y) => x.score - y.score);
      setHits(merged.slice(0, MAX));
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

  const copy = (text: string) =>
    invoke("copy_text", { text })
      .catch((e) => console.error("copy failed:", e))
      .finally(done);

  const run = (i: number) => {
    const h = list[i];
    if (!h) return;
    if (h.kind === "calc") return copy(h.name); // ephemeral: never learned
    const ab = abandoned.current;
    // The only path that writes to the brain: an explicit pick.
    invoke("commit", {
      commit: {
        query,
        picked: i,
        shown: list.map((x) => ({ id: x.path, name: x.name, kind: x.kind })),
        abandoned: ab?.text ?? null,
        abandonedAgeMs: ab ? Date.now() - ab.t : 0,
      },
    }).catch((e) => console.error("commit failed:", e));
    invoke<string>("launch", { path: h.path, kind: h.kind }).catch((e) => console.error("launch failed:", e));
    done();
  };

  const openSource = (a: Answer) => {
    invoke("launch", { path: a.url, kind: "url" }).catch((e) => console.error("open failed:", e));
    done();
  };

  const runCommand = async () => {
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
    }
  };

  // Shared by input and ghost layer so both sit on the same baseline.
  const line = "m-0 p-0 border-0 h-8 text-2xl leading-8 font-light font-sans";
  const badge = "shrink-0 rounded-md px-2 py-0.5 text-xs";
  return (
    // Fills the whole window: DWM Acrylic + 8px native round corners sit exactly behind this container.
    <div className="h-full flex flex-col rounded-lg bg-black/40 border border-white/15 overflow-hidden text-white">
      <div className="h-[70px] shrink-0 flex items-center gap-3 px-5">
        <ActionIcon kind={current?.kind ?? "search"} src={current && icons[current.path]} />
        <div className="relative flex-1">
          <div aria-hidden className={`absolute inset-0 whitespace-pre pointer-events-none ${line}`}>
            <span className="invisible">{query}</span>
            <span className="text-white/40">{ghost}</span>
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
            className={`relative block w-full bg-transparent outline-none text-white placeholder:text-white/40 caret-white ${line}`}
          />
        </div>
        {paused && <span className={`${badge} bg-amber-400/15 text-amber-300`}>PAUSED</span>}
        {query && !isCmd && <span className={`${badge} bg-white/15 text-white/80`}>{BADGE[current?.kind ?? "search"]}</span>}
      </div>
      {card && (
        <div
          onMouseMove={() => setSel(-1)}
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => copy(card.text)}
          className={`h-[88px] shrink-0 border-t border-white/15 px-5 py-2.5 flex gap-3 cursor-default ${sel < 0 ? "bg-white/15" : ""}`}
        >
          <p className="flex-1 text-[13px] leading-5 text-white/90 line-clamp-3">{card.text}</p>
          <button
            onClick={(e) => {
              e.stopPropagation();
              openSource(card);
            }}
            className="self-start shrink-0 text-[11px] text-white/50 hover:text-white/80"
            title="Open source (Ctrl+Enter)"
          >
            {card.source} ↗
          </button>
        </div>
      )}
      {rowCount > 0 && (
        <ul className="border-t border-white/15 py-1">
          {isCmd
            ? infoRows.map((r, i) => (
                <li key={i} className="h-11 flex items-center gap-3 px-5">
                  <span className="flex-1 truncate text-sm text-white">{r.title}</span>
                  <span className="max-w-[45%] truncate text-xs text-white/60">{r.detail}</span>
                </li>
              ))
            : list.map((h, i) => (
                <li
                  key={h.path}
                  onMouseMove={() => setSel(i)}
                  onMouseDown={(e) => e.preventDefault()} // keep input focus
                  onClick={() => run(i)}
                  className={`h-11 flex items-center gap-3 px-5 cursor-default ${i === sel ? "bg-white/15" : ""}`}
                >
                  <ActionIcon kind={h.kind} src={icons[h.path]} />
                  <span className="flex-1 truncate text-sm text-white">{h.name}</span>
                  {h.running ? (
                    <span className="flex items-center gap-1.5 text-xs text-emerald-300">
                      <span className="h-1.5 w-1.5 rounded-full bg-emerald-400" />
                      Switch
                    </span>
                  ) : (
                    <span className="max-w-[45%] truncate text-xs text-white/60">
                      {h.kind === "calc"
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
