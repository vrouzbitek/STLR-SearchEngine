//! Self-learning ranking brain.
//! SQLite (WAL) is only the persistence layer; every read on the keystroke path is served from a RAM mirror.
//!
//! Model: a row per (query_prefix, target, context_exe) holding a frecency `score` that decays exponentially
//! (half-life `Weights::half_life_days`). A target's boost for a prefix = sum of its decayed scores over all
//! contexts + `context_weight` x the decayed score in the current foreground-app context.
//! Writes happen only on an explicit commit (Enter / click) and never while paused.
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Ranking knobs. Override any subset via `%APPDATA%/com.stlr.searchengine/weights.json`.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Weights {
    /// Days until a learned score halves.
    pub half_life_days: f64,
    /// Gain when the user picks the #1 result (confirms the current ranking).
    pub pick_first: f64,
    /// Gain when the user picks #N > 1 (strong correction: one pick should move it to the top).
    pub pick_lower: f64,
    /// Penalty for each result shown above the picked one (#1..#N-1), for the exact typed prefix only.
    pub skip_penalty: f64,
    /// Gain linking an abandoned query A to the target finally picked after retyping.
    pub retype_gain: f64,
    /// Max seconds between abandoning A and the commit for the A -> X link to count.
    pub retype_window_secs: u64,
    /// Extra weight for scores learned in the same foreground app.
    pub context_weight: f64,
    /// Boost clamp; match scores run 0 (exact) .. 5 (no textual match).
    pub max_boost: f64,
    pub min_score: f64,
    /// Learned targets with at least this boost are injected even if their name doesn't match.
    pub inject_threshold: f64,
    /// Prefixes longer than this are not stored separately (the full query still is).
    pub max_prefix: usize,
    /// Intent memory: the target with the strongest boost >= this for a query is pinned to #1
    /// (one explicit pick is enough). Set very high (e.g. 99) to disable pinning.
    pub pin_threshold: f64,
    /// Cold-start seed scores = source weight (0.8..1.5) x this. Must stay well below `pick_first`
    /// so any real click overrides seeds, and below `pin_threshold` / `inject_threshold`.
    pub seed_scale: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            half_life_days: 14.0,
            pick_first: 1.0,
            pick_lower: 1.5,
            skip_penalty: 0.3,
            retype_gain: 1.0,
            retype_window_secs: 6,
            context_weight: 0.5,
            max_boost: 4.0,
            min_score: -1.0,
            inject_threshold: 0.5,
            max_prefix: 16,
            pin_threshold: 1.0,
            seed_scale: 0.2,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Target {
    pub id: String, // full path
    pub name: String,
    pub kind: String,
}

/// What the user saw and picked. Sent by the frontend only on Enter / click.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub query: String,
    pub picked: usize,
    pub shown: Vec<Target>,
    /// Query the user typed, then deleted or replaced, before this one.
    pub abandoned: Option<String>,
    #[serde(default)]
    pub abandoned_age_ms: u64,
}

#[derive(Clone, Copy, Debug)]
struct Stat {
    score: f64,
    count: u32,
    last: i64,
}

struct Entry {
    name: String,
    kind: String,
    by_ctx: HashMap<String, Stat>,
}

#[derive(Serialize, Debug)]
pub struct TopLink {
    pub prefix: String,
    pub name: String,
    pub score: f64,
}

#[derive(Serialize, Debug)]
pub struct Stats {
    pub links: usize,
    pub queries: usize,
    pub paused: bool,
    pub top: Vec<TopLink>,
}

pub struct Brain {
    pub w: Weights,
    conn: Connection,
    sel: HashMap<String, HashMap<String, Entry>>, // prefix -> target id -> entry
    hist: HashMap<String, (u32, i64)>,           // committed query -> (count, last_used)
    paused: bool,
}

/// Never learned or penalized: calculator results and open tabs are ephemeral ids, and system commands
/// must never be promoted into zero-query (one stray Enter would shut the PC down).
fn learnable(t: &Target) -> bool {
    !matches!(t.kind.as_str(), "calc" | "tab" | "system")
}

pub fn norm(q: &str) -> String {
    q.trim().to_lowercase()
}

fn decay(score: f64, last: i64, now: i64, half_life_days: f64) -> f64 {
    let days = (now - last).max(0) as f64 / 86_400.0;
    score * 0.5f64.powf(days / half_life_days)
}

const SCHEMA: &str = "
PRAGMA journal_mode=WAL;
PRAGMA synchronous=NORMAL;
CREATE TABLE IF NOT EXISTS selections(
  query_prefix TEXT NOT NULL, target_id TEXT NOT NULL, target_type TEXT NOT NULL, target_name TEXT NOT NULL,
  score REAL NOT NULL, count INTEGER NOT NULL, last_used INTEGER NOT NULL, context_exe TEXT NOT NULL,
  PRIMARY KEY(query_prefix, target_id, context_exe));
CREATE TABLE IF NOT EXISTS query_history(query TEXT PRIMARY KEY, count INTEGER NOT NULL, last_used INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);";

impl Brain {
    pub fn open(path: &Path, w: Weights) -> rusqlite::Result<Self> {
        Self::init(Connection::open(path)?, w)
    }

    pub fn in_memory(w: Weights) -> Self {
        Self::init(Connection::open_in_memory().expect("sqlite in-memory"), w).expect("schema")
    }

    fn init(conn: Connection, w: Weights) -> rusqlite::Result<Self> {
        conn.execute_batch(SCHEMA)?;
        let mut sel: HashMap<String, HashMap<String, Entry>> = HashMap::new();
        {
            let mut st = conn.prepare(
                "SELECT query_prefix,target_id,target_type,target_name,score,count,last_used,context_exe FROM selections",
            )?;
            let rows = st.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    Stat { score: r.get(4)?, count: r.get(5)?, last: r.get(6)? },
                    r.get::<_, String>(7)?,
                ))
            })?;
            for row in rows {
                let (p, id, kind, name, stat, ctx) = row?;
                sel.entry(p)
                    .or_default()
                    .entry(id)
                    .or_insert_with(|| Entry { name, kind, by_ctx: HashMap::new() })
                    .by_ctx
                    .insert(ctx, stat);
            }
        }
        let mut hist = HashMap::new();
        {
            let mut st = conn.prepare("SELECT query,count,last_used FROM query_history")?;
            for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?)))? {
                let (q, c, l) = row?;
                hist.insert(q, (c, l));
            }
        }
        let paused = conn
            .query_row("SELECT value FROM meta WHERE key='paused'", [], |r| r.get::<_, String>(0))
            .is_ok_and(|v| v == "1");
        Ok(Self { w, conn, sel, hist, paused })
    }

    fn entry_boost(&self, e: &Entry, ctx: &str, now: i64) -> f64 {
        let hl = self.w.half_life_days;
        let general: f64 = e.by_ctx.values().map(|s| decay(s.score, s.last, now, hl)).sum();
        let local = match (ctx.is_empty(), e.by_ctx.get(ctx)) {
            (false, Some(s)) => decay(s.score, s.last, now, hl).max(0.0),
            _ => 0.0,
        };
        (general + self.w.context_weight * local).clamp(self.w.min_score, self.w.max_boost)
    }

    /// Learned boost for `id` under normalized `prefix` (subtract from the match score; higher = better).
    pub fn boost(&self, prefix: &str, id: &str, ctx: &str, now: i64) -> f64 {
        self.sel.get(prefix).and_then(|m| m.get(id)).map_or(0.0, |e| self.entry_boost(e, ctx, now))
    }

    /// Learned targets for `prefix` with boost >= `min`, strongest first.
    pub fn top(&self, prefix: &str, ctx: &str, now: i64, min: f64) -> Vec<(Target, f64)> {
        let Some(m) = self.sel.get(prefix) else { return vec![] };
        let mut v: Vec<_> = m
            .iter()
            .map(|(id, e)| {
                (Target { id: id.clone(), name: e.name.clone(), kind: e.kind.clone() }, self.entry_boost(e, ctx, now))
            })
            .filter(|(_, b)| *b >= min)
            .collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1));
        v
    }

    /// Most frequently committed query extending `prefix`.
    pub fn ghost(&self, prefix: &str) -> Option<String> {
        if prefix.is_empty() {
            return None;
        }
        self.hist
            .iter()
            .filter(|(q, _)| q.len() > prefix.len() && q.starts_with(prefix))
            .max_by_key(|(_, (count, last))| (*count, *last))
            .map(|(q, _)| q.clone())
    }

    /// "" (zero-query) + every char prefix of `q` up to `max_prefix` + `q` itself.
    fn prefixes(&self, q: &str) -> Vec<String> {
        let mut v = vec![String::new()];
        let mut s = String::new();
        for ch in q.chars().take(self.w.max_prefix) {
            s.push(ch);
            v.push(s.clone());
        }
        if s != q {
            v.push(q.to_string());
        }
        v
    }

    fn stat_mut(&mut self, prefix: &str, t: &Target, ctx: &str, now: i64) -> &mut Stat {
        let e = self.sel.entry(prefix.to_string()).or_default().entry(t.id.clone()).or_insert_with(|| Entry {
            name: t.name.clone(),
            kind: t.kind.clone(),
            by_ctx: HashMap::new(),
        });
        e.name.clone_from(&t.name);
        e.kind.clone_from(&t.kind);
        e.by_ctx.entry(ctx.to_string()).or_insert(Stat { score: 0.0, count: 0, last: now })
    }

    fn add(&mut self, prefix: &str, t: &Target, ctx: &str, delta: f64, now: i64, dirty: &mut Vec<(String, String, String)>) {
        let (hl, min) = (self.w.half_life_days, self.w.min_score);
        let s = self.stat_mut(prefix, t, ctx, now);
        s.score = (decay(s.score, s.last, now, hl) + delta).max(min);
        if delta > 0.0 {
            s.count += 1;
        }
        s.last = now;
        dirty.push((prefix.to_string(), t.id.clone(), ctx.to_string()));
    }

    /// Learn from an explicit pick. Returns false (and writes nothing) when paused or the commit is invalid.
    pub fn record(&mut self, c: &Commit, ctx: &str, now: i64) -> bool {
        if self.paused || c.picked >= c.shown.len() {
            return false;
        }
        let q = norm(&c.query);
        if q.starts_with('/') {
            return false; // commands are never learned
        }
        let picked = c.shown[c.picked].clone();
        if !learnable(&picked) {
            return false;
        }
        let mut dirty = Vec::new();

        // Frecency (+ strong bonus when the user had to go below #1).
        let gain = if c.picked == 0 { self.w.pick_first } else { self.w.pick_lower };
        for p in self.prefixes(&q) {
            self.add(&p, &picked, ctx, gain, now, &mut dirty);
        }
        // Skip penalty: everything shown above the pick, for the prefix the user actually saw.
        let penalty = -self.w.skip_penalty;
        for t in c.shown[..c.picked].to_vec() {
            if t.id != picked.id && learnable(&t) {
                self.add(&q, &t, ctx, penalty, now, &mut dirty);
            }
        }
        // Abandon & retype: link the given-up query A (and its prefixes not shared with q) to the pick.
        if let Some(a) = c.abandoned.as_deref().map(norm) {
            let fresh = c.abandoned_age_ms <= self.w.retype_window_secs * 1000;
            if fresh && !a.is_empty() && !a.starts_with('/') && !q.starts_with(&a) {
                for p in self.prefixes(&a) {
                    if !p.is_empty() && !q.starts_with(&p) {
                        self.add(&p, &picked, ctx, self.w.retype_gain, now, &mut dirty);
                    }
                }
            }
        }
        let hist_key = (!q.is_empty()).then(|| {
            let h = self.hist.entry(q.clone()).or_insert((0, now));
            h.0 += 1;
            h.1 = now;
            q.clone()
        });
        self.persist(&dirty, hist_key.as_deref());
        true
    }

    pub fn is_empty(&self) -> bool {
        self.sel.is_empty()
    }

    /// Cold-start seeding: `weight` x `seed_scale` under zero-query ("") and the first 1-2 letters of the name.
    /// Only creates missing rows: learned scores, skip penalties and earlier seeds are never touched,
    /// so it's safe to re-run (`/brain reseed` adds newly found items). Returns rows written; 0 while paused.
    pub fn seed(&mut self, seeds: &[(Target, f64)], now: i64) -> usize {
        if self.paused {
            return 0;
        }
        let mut dirty = Vec::new();
        for (t, weight) in seeds {
            let score = weight * self.w.seed_scale;
            let mut prefixes = vec![String::new()];
            let mut p = String::new();
            for ch in norm(&t.name).chars().take(2) {
                p.push(ch);
                prefixes.push(p.clone());
            }
            for p in prefixes {
                // Only rows that don't exist yet: anything already there (a pick, a skip penalty, an older
                // seed) is left untouched, so reseeding can never undo learning.
                let exists = self.sel.get(&p).and_then(|m| m.get(&t.id)).is_some_and(|e| e.by_ctx.contains_key(""));
                if !exists {
                    let s = self.stat_mut(&p, t, "", now);
                    s.score = score;
                    s.last = now;
                    dirty.push((p, t.id.clone(), String::new()));
                }
            }
        }
        self.persist(&dirty, None);
        dirty.len()
    }

    fn persist(&mut self, dirty: &[(String, String, String)], hist_key: Option<&str>) {
        let Self { conn, sel, hist, .. } = self;
        let r = (|| -> rusqlite::Result<()> {
            let tx = conn.transaction()?;
            {
                let mut up = tx.prepare_cached("INSERT OR REPLACE INTO selections VALUES (?1,?2,?3,?4,?5,?6,?7,?8)")?;
                for (p, id, ctx) in dirty {
                    let e = &sel[p][id];
                    let s = &e.by_ctx[ctx];
                    up.execute(params![p, id, e.kind, e.name, s.score, s.count, s.last, ctx])?;
                }
                if let Some(q) = hist_key {
                    let (c, l) = hist[q];
                    tx.execute("INSERT OR REPLACE INTO query_history VALUES (?1,?2,?3)", params![q, c, l])?;
                }
            }
            tx.commit()
        })();
        if let Err(e) = r {
            eprintln!("brain: write failed: {e}"); // RAM mirror stays correct for this session
        }
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        let v = if paused { "1" } else { "0" };
        if let Err(e) = self.conn.execute("INSERT OR REPLACE INTO meta VALUES ('paused', ?1)", params![v]) {
            eprintln!("brain: pause flag not saved: {e}");
        }
    }

    pub fn reset(&mut self) {
        self.sel.clear();
        self.hist.clear();
        if let Err(e) = self.conn.execute_batch("DELETE FROM selections; DELETE FROM query_history;") {
            eprintln!("brain: reset failed: {e}");
        }
    }

    pub fn stats(&self, now: i64) -> Stats {
        let mut pairs: Vec<TopLink> = Vec::new();
        for (p, m) in &self.sel {
            if p.is_empty() {
                continue; // zero-query rows mirror every pick; not a "link"
            }
            for e in m.values() {
                let score = self.entry_boost(e, "", now);
                if score > 0.0 {
                    pairs.push(TopLink { prefix: p.clone(), name: e.name.clone(), score });
                }
            }
        }
        let links = pairs.len();
        // Strongest first; on ties prefer the longest prefix. One row per target.
        pairs.sort_by(|a, b| b.score.total_cmp(&a.score).then(b.prefix.len().cmp(&a.prefix.len())));
        let mut seen = std::collections::HashSet::new();
        let top = pairs.into_iter().filter(|t| seen.insert(t.name.clone())).take(5).collect();
        Stats { links, queries: self.hist.len(), paused: self.paused, top }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;
    const T0: i64 = 1_700_000_000;

    fn t(id: &str) -> Target {
        Target { id: id.into(), name: id.into(), kind: "app".into() }
    }

    fn commit(q: &str, shown: &[&str], picked: usize) -> Commit {
        Commit { query: q.into(), picked, shown: shown.iter().map(|s| t(s)).collect(), abandoned: None, abandoned_age_ms: 0 }
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn frecency_grows_and_decays() {
        let mut b = Brain::in_memory(Weights::default());
        b.record(&commit("spo", &["spotify"], 0), "", T0);
        b.record(&commit("spo", &["spotify"], 0), "", T0);
        assert!(approx(b.boost("spo", "spotify", "", T0), 2.0));
        assert!(approx(b.boost("s", "spotify", "", T0), 2.0), "shorter prefixes learn too");
        assert!(approx(b.boost("", "spotify", "", T0), 2.0), "zero-query learns too");
        assert!(approx(b.boost("spo", "spotify", "", T0 + 14 * DAY), 1.0), "halves after half-life");
        // Decay is applied before adding: one more pick two weeks later -> 1.0 + 1.0
        b.record(&commit("spo", &["spotify"], 0), "", T0 + 14 * DAY);
        assert!(approx(b.boost("spo", "spotify", "", T0 + 14 * DAY), 2.0));
    }

    #[test]
    fn skip_penalty_moves_pick_to_top_after_one_click() {
        let mut b = Brain::in_memory(Weights::default());
        let shown = ["steam", "slack", "spotify"];
        b.record(&commit("s", &shown, 2), "", T0);
        assert!(approx(b.boost("s", "spotify", "", T0), 1.5));
        assert!(approx(b.boost("s", "steam", "", T0), -0.3));
        assert!(approx(b.boost("s", "slack", "", T0), -0.3));
        // Equal textual match (prefix = 1.0): spotify now ranks first.
        let rank = |id: &str| 1.0 - b.boost("s", id, "", T0);
        assert!(rank("spotify") < rank("steam") && rank("spotify") < rank("slack"));
        // Penalty applies only to the prefix that was actually shown.
        assert!(approx(b.boost("", "steam", "", T0), 0.0));
    }

    #[test]
    fn penalty_floor() {
        let mut b = Brain::in_memory(Weights::default());
        for _ in 0..20 {
            b.record(&commit("s", &["steam", "spotify"], 1), "", T0);
        }
        assert!(approx(b.boost("s", "steam", "", T0), -1.0));
    }

    #[test]
    fn abandon_and_retype_links_old_query() {
        let mut b = Brain::in_memory(Weights::default());
        let mut c = commit("spotify", &["spotify"], 0);
        c.abandoned = Some("Music".into());
        c.abandoned_age_ms = 2_000;
        b.record(&c, "", T0);
        assert!(approx(b.boost("music", "spotify", "", T0), 1.0));
        assert!(approx(b.boost("mu", "spotify", "", T0), 1.0));
        let injected = b.top("music", "", T0, b.w.inject_threshold);
        assert_eq!(injected[0].0.id, "spotify");
    }

    #[test]
    fn abandon_ignored_when_stale_or_continuation() {
        let mut b = Brain::in_memory(Weights::default());
        let mut c = commit("spotify", &["spotify"], 0);
        c.abandoned = Some("music".into());
        c.abandoned_age_ms = 7_000;
        b.record(&c, "", T0);
        assert!(approx(b.boost("music", "spotify", "", T0), 0.0));
        // "spo" -> "spotify" is a continuation, not an abandon: no extra retype gain on "spo".
        let mut c = commit("spotify", &["spotify"], 0);
        c.abandoned = Some("spo".into());
        c.abandoned_age_ms = 1_000;
        b.record(&c, "", T0);
        assert!(approx(b.boost("spo", "spotify", "", T0), 2.0)); // two plain picks only
    }

    #[test]
    fn context_bonus() {
        let mut b = Brain::in_memory(Weights::default());
        b.record(&commit("b", &["blender"], 0), "unrealeditor.exe", T0);
        let elsewhere = b.boost("b", "blender", "chrome.exe", T0);
        let same = b.boost("b", "blender", "unrealeditor.exe", T0);
        assert!(approx(elsewhere, 1.0) && approx(same, 1.5));
        // Zero-query predictions favour the current context.
        b.record(&commit("r", &["reaper"], 0), "chrome.exe", T0);
        assert_eq!(b.top("", "unrealeditor.exe", T0, 0.01)[0].0.id, "blender");
        assert_eq!(b.top("", "chrome.exe", T0, 0.01)[0].0.id, "reaper");
    }

    #[test]
    fn pause_blocks_all_writes_and_persists() {
        let path = std::env::temp_dir().join(format!("stlr-brain-test-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let mut b = Brain::open(&path, Weights::default()).unwrap();
            b.set_paused(true);
            assert!(!b.record(&commit("spo", &["spotify"], 0), "", T0));
            assert!(approx(b.boost("spo", "spotify", "", T0), 0.0));
            assert_eq!(b.ghost("s"), None);
            b.set_paused(false);
            assert!(b.record(&commit("spo", &["spotify"], 0), "", T0));
        }
        let b = Brain::open(&path, Weights::default()).unwrap();
        assert!(!b.paused());
        assert!(approx(b.boost("spo", "spotify", "", T0), 1.0), "survives reopen");
        assert_eq!(b.ghost("s").as_deref(), Some("spo"));
        drop(b);
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
    }

    #[test]
    fn ghost_prefers_most_frequent_query() {
        let mut b = Brain::in_memory(Weights::default());
        b.record(&commit("spotify kids", &["x"], 0), "", T0);
        b.record(&commit("spo", &["spotify"], 0), "", T0);
        b.record(&commit("spo", &["spotify"], 0), "", T0);
        assert_eq!(b.ghost("s").as_deref(), Some("spo"));
        assert_eq!(b.ghost("spo").as_deref(), Some("spotify kids"));
        assert_eq!(b.ghost("x"), None);
    }

    #[test]
    fn commands_and_invalid_commits_are_ignored() {
        let mut b = Brain::in_memory(Weights::default());
        assert!(!b.record(&commit("/brain", &["x"], 0), "", T0));
        assert!(!b.record(&commit("x", &["x"], 3), "", T0));
        // Calculator results are never learned nor penalized.
        let mut c = commit("2*3", &["calc:6", "google"], 0);
        c.shown[0].kind = "calc".into();
        assert!(!b.record(&c, "", T0));
        c.picked = 1;
        assert!(b.record(&c, "", T0));
        assert!(approx(b.boost("2*3", "calc:6", "", T0), 0.0));
        b.reset();
        assert_eq!(b.stats(T0).links, 0);
    }

    #[test]
    fn seeds_are_weak_and_never_override_learning() {
        let mut b = Brain::in_memory(Weights::default());
        let spotify = Target { id: "spotify.lnk".into(), name: "Spotify".into(), kind: "app".into() };
        let steam = Target { id: "steam.lnk".into(), name: "Steam".into(), kind: "app".into() };
        assert_eq!(b.seed(&[(spotify.clone(), 1.5), (steam.clone(), 1.0)], T0), 6); // "", s, sp / "", s, st
        assert!(approx(b.boost("", "spotify.lnk", "", T0), 0.3));
        assert!(approx(b.boost("sp", "spotify.lnk", "", T0), 0.3));
        assert!(approx(b.boost("spo", "spotify.lnk", "", T0), 0.0), "only 1-2 letter prefixes");
        assert_eq!(b.top("", "", T0, 0.01)[0].0.id, "spotify.lnk", "zero-query has predictions");
        // Seeds stay below pin/inject thresholds.
        assert!(b.top("s", "", T0, b.w.inject_threshold).is_empty());
        // One real click beats any seed.
        let pick = Commit { query: "s".into(), picked: 1, shown: vec![spotify.clone(), steam.clone()], abandoned: None, abandoned_age_ms: 0 };
        b.record(&pick, "", T0);
        assert!(b.boost("s", "steam.lnk", "", T0) > b.boost("s", "spotify.lnk", "", T0));
        // Reseed never lowers learning nor erases the skip penalty.
        let before = (b.boost("s", "steam.lnk", "", T0), b.boost("s", "spotify.lnk", "", T0));
        b.seed(&[(spotify.clone(), 1.5), (steam.clone(), 1.0)], T0);
        assert_eq!(before, (b.boost("s", "steam.lnk", "", T0), b.boost("s", "spotify.lnk", "", T0)));
        // Paused: no writes.
        b.set_paused(true);
        let other = Target { id: "x.lnk".into(), name: "X".into(), kind: "app".into() };
        assert_eq!(b.seed(&[(other, 1.5)], T0), 0);
        assert!(approx(b.boost("", "x.lnk", "", T0), 0.0));
    }

    #[test]
    fn stats_and_reset() {
        let mut b = Brain::in_memory(Weights::default());
        b.record(&commit("spo", &["spotify"], 0), "", T0);
        let s = b.stats(T0);
        assert_eq!(s.links, 3); // s, sp, spo
        assert_eq!(s.queries, 1);
        assert_eq!(s.top.len(), 1);
        assert_eq!(s.top[0].prefix, "spo");
        b.reset();
        assert_eq!(b.stats(T0).links, 0);
        assert!(approx(b.boost("spo", "spotify", "", T0), 0.0));
    }
}
