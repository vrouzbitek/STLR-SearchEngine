import { useEffect, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type LearningMode = "calm" | "balanced" | "aggressive";
type Provider = "none" | "groq" | "gemini" | "openai";
type Settings = {
  shortcut: string;
  autostart: boolean;
  learningMode: LearningMode;
  webAnswers: boolean;
  llm: { provider: Provider; apiKey: string; model: string; baseUrl: string };
};
type Status = { paused: boolean; autostart: boolean; browsers: { browser: string; tabs: number }[]; extensionPath: string };

const SHORTCUTS = ["Alt+Space", "Ctrl+Space", "Alt+Q", "Ctrl+Alt+Space"];
const MODES: { id: LearningMode; label: string; hint: string }[] = [
  { id: "calm", label: "Calm", hint: "Needs several picks before reordering" },
  { id: "balanced", label: "Balanced", hint: "One pick below #1 moves it up" },
  { id: "aggressive", label: "Aggressive", hint: "Adapts instantly, forgets faster" },
];
const PROVIDERS: { id: Provider; label: string; model: string }[] = [
  { id: "none", label: "None (free Wiki/DDG only)", model: "" },
  { id: "groq", label: "Groq", model: "llama-3.1-8b-instant" },
  { id: "gemini", label: "Gemini", model: "gemini-2.5-flash" },
  { id: "openai", label: "OpenAI-compatible", model: "gpt-4o-mini" },
];

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="rounded-xl border border-white/10 bg-white/[0.03] p-5">
      <h2 className="mb-4 text-xs font-semibold uppercase tracking-wider text-white/45">{title}</h2>
      <div className="space-y-4">{children}</div>
    </section>
  );
}

function Row({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-6">
      <div>
        <div className="text-sm text-white/90">{label}</div>
        {hint && <div className="mt-0.5 text-xs text-white/45">{hint}</div>}
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  );
}

function Toggle({ on, onChange, label }: { on: boolean; onChange: (v: boolean) => void; label: string }) {
  return (
    <button
      role="switch"
      aria-checked={on}
      aria-label={label}
      onClick={() => onChange(!on)}
      className={`relative h-6 w-11 rounded-full transition-colors ${on ? "bg-sky-500" : "bg-white/15"}`}
    >
      <span className={`absolute top-0.5 h-5 w-5 rounded-full bg-white transition-all ${on ? "left-[22px]" : "left-0.5"}`} />
    </button>
  );
}

const input = "rounded-md border border-white/10 bg-black/40 px-3 py-1.5 text-sm text-white outline-none focus:border-sky-400/60";
const button = "rounded-md border border-white/10 bg-white/[0.06] px-3 py-1.5 text-sm text-white/85 hover:bg-white/10";

export default function SettingsPanel() {
  const [s, setS] = useState<Settings | null>(null);
  const [status, setStatus] = useState<Status | null>(null);
  const [msg, setMsg] = useState<{ text: string; error?: boolean } | null>(null);
  const [confirmReset, setConfirmReset] = useState(false);
  // API key / model / URL are edited locally and saved on blur, not per keystroke.
  const [draft, setDraft] = useState({ apiKey: "", model: "", baseUrl: "" });

  const load = () =>
    invoke<Settings>("get_settings").then((v) => {
      setS(v);
      setDraft({ apiKey: v.llm.apiKey, model: v.llm.model, baseUrl: v.llm.baseUrl });
    });

  useEffect(() => {
    load();
    const un = listen("settings-opened", load);
    const poll = () => invoke<Status>("get_status").then(setStatus);
    poll();
    const t = setInterval(poll, 1000);
    return () => {
      clearInterval(t);
      un.then((f) => f());
    };
  }, []);

  const flash = (text: string, error = false) => {
    setMsg({ text, error });
    setTimeout(() => setMsg(null), 3500);
  };

  const save = (next: Settings) => {
    setS(next);
    invoke<Settings>("update_settings", { new: next })
      .then((applied) => {
        setS(applied);
        flash("Saved");
      })
      .catch((e) => {
        flash(String(e), true);
        load(); // show what is actually in effect
      });
  };

  if (!s || !status) return <div className="h-full bg-[#0d0d12]" />;

  const llmOn = s.llm.provider !== "none";
  const provider = PROVIDERS.find((p) => p.id === s.llm.provider)!;
  const saveLlm = () => save({ ...s, llm: { ...s.llm, ...draft } });

  return (
    <div className="h-full overflow-y-auto bg-[#0d0d12] text-white">
      <div className="mx-auto max-w-2xl space-y-5 p-6">
        <header className="flex items-baseline justify-between">
          <h1 className="text-lg font-semibold">STLR Search</h1>
          {msg && <span className={`text-xs ${msg.error ? "text-rose-300" : "text-emerald-300"}`}>{msg.text}</span>}
        </header>

        <Section title="General">
          <Row label="Global shortcut" hint="Opens the search bar from anywhere">
            <select className={input} value={s.shortcut} onChange={(e) => save({ ...s, shortcut: e.target.value })}>
              {SHORTCUTS.map((k) => (
                <option key={k} value={k} className="bg-neutral-900">
                  {k}
                </option>
              ))}
            </select>
          </Row>
          <Row label="Start with Windows" hint="Per-user, no admin rights needed">
            <Toggle label="Start with Windows" on={status.autostart} onChange={(v) => save({ ...s, autostart: v })} />
          </Row>
        </Section>

        <Section title="Brain">
          <Row label="Pause learning" hint="Ranking still uses what was learned; nothing new is written">
            <Toggle
              label="Pause learning"
              on={status.paused}
              onChange={(v) => invoke<boolean>("brain_pause", { paused: v }).then((p) => setStatus({ ...status, paused: p }))}
            />
          </Row>
          <div>
            <div className="mb-2 text-sm text-white/90">Learning speed</div>
            <div className="grid grid-cols-3 gap-1 rounded-lg bg-black/40 p-1">
              {MODES.map((m) => (
                <button
                  key={m.id}
                  onClick={() => save({ ...s, learningMode: m.id })}
                  className={`rounded-md py-1.5 text-sm ${s.learningMode === m.id ? "bg-sky-500/80 text-white" : "text-white/60 hover:text-white"}`}
                >
                  {m.label}
                </button>
              ))}
            </div>
            <div className="mt-1.5 text-xs text-white/45">{MODES.find((m) => m.id === s.learningMode)!.hint}</div>
          </div>
          <div className="flex gap-2">
            <button
              className={button}
              onClick={() =>
                invoke<{ rows: number; total: number }>("brain_reseed").then((r) =>
                  flash(status.paused ? "Brain is paused: nothing written" : `Reseeded: ${r.rows} new rows (${r.total} items found)`),
                )
              }
            >
              Reseed now
            </button>
            <button
              className={`${button} ${confirmReset ? "border-rose-400/50 text-rose-300" : ""}`}
              onBlur={() => setConfirmReset(false)}
              onClick={() => {
                if (!confirmReset) return setConfirmReset(true);
                setConfirmReset(false);
                invoke("brain_reset").then(() => flash("Brain reset and reseeded"));
              }}
            >
              {confirmReset ? "Click again to wipe everything" : "Reset brain"}
            </button>
          </div>
        </Section>

        <Section title="Instant Answers & Privacy">
          <Row label="Enable web answers" hint="Off = nothing you type ever leaves this PC">
            <Toggle label="Enable web answers" on={s.webAnswers} onChange={(v) => save({ ...s, webAnswers: v })} />
          </Row>
          <div className={s.webAnswers ? "" : "pointer-events-none opacity-40"}>
            <Row label="LLM provider" hint="Questions go to the LLM first; free Wiki/DDG otherwise">
              <select
                className={input}
                value={s.llm.provider}
                onChange={(e) => save({ ...s, llm: { ...s.llm, ...draft, provider: e.target.value as Provider } })}
              >
                {PROVIDERS.map((p) => (
                  <option key={p.id} value={p.id} className="bg-neutral-900">
                    {p.label}
                  </option>
                ))}
              </select>
            </Row>
            {llmOn && (
              <div className="mt-4 space-y-3">
                <label className="block">
                  <span className="mb-1 block text-xs text-white/55">API key (stored only in your local settings.json)</span>
                  <input
                    type="password"
                    autoComplete="off"
                    spellCheck={false}
                    className={`${input} w-full`}
                    value={draft.apiKey}
                    placeholder="Empty = free Wiki/DDG only"
                    onChange={(e) => setDraft({ ...draft, apiKey: e.target.value })}
                    onBlur={saveLlm}
                  />
                </label>
                <label className="block">
                  <span className="mb-1 block text-xs text-white/55">Model</span>
                  <input
                    className={`${input} w-full`}
                    value={draft.model}
                    placeholder={provider.model}
                    onChange={(e) => setDraft({ ...draft, model: e.target.value })}
                    onBlur={saveLlm}
                  />
                </label>
                {s.llm.provider === "openai" && (
                  <label className="block">
                    <span className="mb-1 block text-xs text-white/55">Base URL</span>
                    <input
                      className={`${input} w-full`}
                      value={draft.baseUrl}
                      placeholder="https://api.openai.com/v1"
                      onChange={(e) => setDraft({ ...draft, baseUrl: e.target.value })}
                      onBlur={saveLlm}
                    />
                  </label>
                )}
                <p className="text-xs text-white/45">
                  Only real questions (a question word, “?”, or 3+ words) are sent to the LLM. Answers are 1–2 sentences in
                  the language of the question.
                </p>
              </div>
            )}
          </div>
        </Section>

        <Section title="Browser Tabs Extension">
          <div className="flex items-center gap-2 text-sm">
            <span className={`h-2 w-2 rounded-full ${status.browsers.length ? "bg-emerald-400" : "bg-white/25"}`} />
            {status.browsers.length
              ? `Connected: ${status.browsers.map((b) => `${b.browser} (${b.tabs} tabs)`).join(", ")}`
              : "Not connected"}
          </div>
          <p className="text-xs leading-5 text-white/55">
            Open <code className="text-white/80">edge://extensions</code> (or chrome:// / brave://), turn on Developer mode.
            <br />
            Click “Load unpacked” and pick the folder below. It connects within a few seconds.
          </p>
          <div className="flex items-center gap-2">
            <code className="flex-1 truncate rounded-md bg-black/40 px-3 py-1.5 text-xs text-white/70">{status.extensionPath}</code>
            <button
              className={button}
              onClick={() => invoke("copy_text", { text: status.extensionPath }).then(() => flash("Path copied"))}
            >
              Copy path
            </button>
          </div>
        </Section>
      </div>
    </div>
  );
}
