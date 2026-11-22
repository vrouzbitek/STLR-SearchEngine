import { useEffect, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useTheme } from "./theme";

type Item = { id: string; name: string; kind: string; source: string; detail: string; weight: number };
type Candidates = { apps: Item[]; sites: Item[] };
type Row = Item & { on: boolean };

const SOURCE: Record<string, string> = {
  taskbar: "Taskbar",
  open: "Open now",
  startup: "Startup",
  tab: "Open tab",
  bookmark: "Bookmark",
  custom: "Added",
};

const input =
  "w-full rounded-md border border-[var(--line)] bg-[var(--input)] px-3 py-1.5 text-sm text-[var(--fg)] outline-none placeholder:text-[var(--faint)]";
const button = "rounded-md border border-[var(--line)] px-4 py-1.5 text-sm";

function List({ rows, onToggle, empty }: { rows: Row[]; onToggle: (id: string) => void; empty: string }) {
  if (!rows.length) return <div className="py-3 text-xs text-[var(--faint)]">{empty}</div>;
  return (
    <ul className="max-h-52 overflow-y-auto rounded-md border border-[var(--line)]">
      {rows.map((r) => (
        <li key={r.id}>
          <label className="flex cursor-pointer items-center gap-3 px-3 py-1.5 hover:bg-[var(--sel)]">
            <input type="checkbox" checked={r.on} onChange={() => onToggle(r.id)} className="accent-[var(--toggle)]" />
            <span className={`flex-1 truncate text-sm ${r.on ? "" : "text-[var(--faint)] line-through"}`}>{r.name}</span>
            {r.detail && <span className="max-w-[40%] truncate text-xs text-[var(--faint)]">{r.detail}</span>}
            <span className="chip">{SOURCE[r.source] ?? r.source}</span>
          </label>
        </li>
      ))}
    </ul>
  );
}

function Block({ title, hint, children }: { title: string; hint: string; children: ReactNode }) {
  return (
    <section className="space-y-2">
      <div className="flex items-baseline justify-between">
        <h2 className="text-xs font-semibold uppercase tracking-wider text-[var(--muted)]">{title}</h2>
        <span className="text-xs text-[var(--faint)]">{hint}</span>
      </div>
      {children}
    </section>
  );
}

const merge = (rows: Row[], item: Item): Row[] =>
  rows.some((r) => r.id.toLowerCase() === item.id.toLowerCase())
    ? rows.map((r) => (r.id.toLowerCase() === item.id.toLowerCase() ? { ...r, on: true } : r))
    : [...rows, { ...item, on: true }];

/** First-run wizard: shows exactly what would seed the brain; only checked rows are seeded. */
export default function Wizard() {
  useTheme();
  const [apps, setApps] = useState<Row[] | null>(null);
  const [sites, setSites] = useState<Row[]>([]);
  const [appQuery, setAppQuery] = useState("");
  const [suggest, setSuggest] = useState<Item[]>([]);
  const [site, setSite] = useState("");
  const [siteError, setSiteError] = useState(false);
  const [busy, setBusy] = useState(false);
  const reqId = useRef(0);

  const load = () => {
    setApps(null);
    invoke<Candidates>("wizard_candidates").then((c) => {
      setApps(c.apps.map((a) => ({ ...a, on: true })));
      setSites(c.sites.map((s) => ({ ...s, on: true })));
    });
  };

  useEffect(() => {
    load();
    const un = listen("wizard-opened", load);
    return () => {
      un.then((f) => f());
    };
  }, []);

  // "Add app" autocomplete over the Start Menu index (in-RAM, instant).
  useEffect(() => {
    const id = ++reqId.current;
    if (!appQuery.trim()) return setSuggest([]);
    invoke<Item[]>("wizard_search_apps", { query: appQuery }).then((s) => id === reqId.current && setSuggest(s));
  }, [appQuery]);

  const toggle = (set: typeof setSites) => (id: string) =>
    set((rows) => rows.map((r) => (r.id === id ? { ...r, on: !r.on } : r)));

  const addApp = (it: Item) => {
    setApps((rows) => merge(rows ?? [], it));
    setAppQuery("");
    setSuggest([]);
  };

  const addSite = () =>
    invoke<Item | null>("wizard_site", { input: site }).then((it) => {
      if (!it) return setSiteError(true);
      setSites((rows) => merge(rows, it));
      setSite("");
    });

  const finish = (seed: boolean) => {
    setBusy(true);
    const items = seed ? [...(apps ?? []), ...sites].filter((r) => r.on).map(({ on: _on, ...it }) => it) : [];
    invoke<number>("wizard_finish", { items, seed }).finally(() => setBusy(false));
  };

  const checked = (apps ?? []).filter((r) => r.on).length + sites.filter((r) => r.on).length;

  return (
    <div className="flex h-full flex-col bg-[var(--panel)] text-[var(--fg)]">
      <div className="flex-1 space-y-5 overflow-y-auto p-6">
        <header>
          <h1 className="text-lg font-semibold">Welcome to STLR Search</h1>
          <p className="mt-1 text-sm text-[var(--muted)]">
            Pick what the brain should know on day one. Only checked items are saved, with a low starting weight: your
            first real picks always win. Nothing leaves this PC.
          </p>
        </header>

        <Block title="Detected apps" hint="taskbar · open windows · startup">
          {apps === null ? (
            <div className="py-3 text-xs text-[var(--faint)]">Scanning…</div>
          ) : (
            <List rows={apps} onToggle={toggle(setApps as typeof setSites)} empty="Nothing detected. Add apps below." />
          )}
          <div className="relative">
            <input
              className={input}
              placeholder="Add app: start typing (e.g. blender)"
              value={appQuery}
              spellCheck={false}
              onChange={(e) => setAppQuery(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && suggest[0] && addApp(suggest[0])}
            />
            {suggest.length > 0 && (
              <ul className="absolute z-10 mt-1 w-full overflow-hidden rounded-md border border-[var(--line)] bg-[var(--panel)] shadow-lg">
                {suggest.map((s) => (
                  <li key={s.id}>
                    <button className="w-full px-3 py-1.5 text-left text-sm hover:bg-[var(--sel)]" onClick={() => addApp(s)}>
                      + {s.name}
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </Block>

        <Block title="Detected sites" hint="open tabs · recent bookmarks">
          <List rows={sites} onToggle={toggle(setSites)} empty="No tabs or bookmarks found. Add sites below." />
          <div className="flex gap-2">
            <input
              className={`${input} ${siteError ? "border-rose-400/70" : ""}`}
              placeholder="Add website (e.g. github.com)"
              value={site}
              spellCheck={false}
              onChange={(e) => {
                setSite(e.target.value);
                setSiteError(false);
              }}
              onKeyDown={(e) => e.key === "Enter" && addSite()}
            />
            <button className={`${button} shrink-0 hover:bg-[var(--sel)]`} onClick={addSite}>
              Add
            </button>
          </div>
          {siteError && <div className="text-xs text-rose-400">That doesn’t look like a website address.</div>}
        </Block>
      </div>

      <footer className="flex items-center justify-between border-t border-[var(--line)] px-6 py-4">
        <span className="text-xs text-[var(--faint)]">{checked} selected</span>
        <div className="flex gap-2">
          <button disabled={busy} className={`${button} text-[var(--muted)] hover:bg-[var(--sel)]`} onClick={() => finish(false)}>
            Skip
          </button>
          <button
            disabled={busy || apps === null}
            className={`${button} border-transparent bg-[var(--toggle)] text-[var(--panel)] disabled:opacity-50`}
            onClick={() => finish(true)}
          >
            Finish &amp; Seed
          </button>
        </div>
      </footer>
    </div>
  );
}
