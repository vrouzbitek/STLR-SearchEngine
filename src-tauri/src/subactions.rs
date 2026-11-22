//! Sub-actions of an app, shown under it (↳) and learned by the brain like any other target:
//! 1. Recent projects, fully autonomous: Windows' Recent shortcuts -> real files -> default app of each
//!    extension (AssocQueryStringW / ASSOCSTR_EXECUTABLE) -> `app exe -> newest files`. No per-app code.
//! 2. Community Action Packs (`actions.json`): CLI flags / URIs per app, activated only for indexed apps.
use crate::Hit;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, RwLock};
use std::time::SystemTime;

const RECENT_PER_APP: usize = 5;
/// Newest Recent shortcuts considered per scan (each needs a COM .lnk resolve).
const RECENT_SCAN_LIMIT: usize = 200;
pub const RESCAN_SECS: u64 = 120;
const DEFAULT_PACKS: &str = include_str!("../../actions.json");

#[derive(Deserialize, Clone, Debug)]
struct ActionDef {
    name: String,
    #[serde(default)]
    args: Vec<String>,
    uri: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
struct Pack {
    #[serde(default)]
    exe: Vec<String>,
    #[serde(default)]
    names: Vec<String>,
    actions: Vec<ActionDef>,
}

#[derive(Deserialize)]
struct PackFile {
    packs: Vec<Pack>,
}

/// What a sub-action path (`act:<json>`) launches. Serialized into the id so the brain can learn it.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(untagged)]
enum Launch {
    Exe { exe: String, args: Vec<String> },
    Uri { uri: String },
}

struct Recent {
    modified: SystemTime,
    name: String,
    path: String,
}

/// lowercase Start Menu path of an indexed app -> its pack actions (activated at startup).
static ACTIVE: LazyLock<RwLock<HashMap<String, Vec<Hit>>>> = LazyLock::new(Default::default);
/// lowercase default-app exe -> newest files it opens.
static RECENT: LazyLock<RwLock<HashMap<String, Vec<Recent>>>> = LazyLock::new(Default::default);
/// ".ext" -> lowercase default exe (None = no association). Associations rarely change; cached per run.
static ASSOC: LazyLock<Mutex<HashMap<String, Option<String>>>> = LazyLock::new(Default::default);
/// Recent .lnk (lowercase) -> (its mtime, resolved target): periodic rescans only resolve what changed.
static LNK_CACHE: LazyLock<Mutex<HashMap<String, (SystemTime, Option<String>)>>> = LazyLock::new(Default::default);

fn parse_packs(json: &str) -> Vec<Pack> {
    serde_json::from_str::<PackFile>(json).map(|f| f.packs).unwrap_or_else(|e| {
        eprintln!("actions.json invalid ({e}), no action packs");
        vec![]
    })
}

fn action_hit(def: &ActionDef, exe: &str) -> Hit {
    let launch = match &def.uri {
        Some(uri) => Launch::Uri { uri: uri.clone() },
        None => Launch::Exe { exe: exe.to_string(), args: def.args.clone() },
    };
    Hit {
        name: def.name.clone(),
        path: format!("act:{}", serde_json::to_string(&launch).unwrap()),
        kind: "action",
        detail: Some("Action".into()),
        child: true,
        ..Default::default()
    }
}

fn pack_matches(p: &Pack, exe_name: &str, app_name: &str) -> bool {
    p.exe.iter().any(|e| e.eq_ignore_ascii_case(exe_name)) || p.names.iter().any(|n| n.eq_ignore_ascii_case(app_name))
}

/// Activate packs for apps that are actually indexed. `apps` = (name, Start Menu path, resolved exe).
fn activate_with(packs: &[Pack], apps: &[(String, String, Option<String>)]) -> HashMap<String, Vec<Hit>> {
    let mut out = HashMap::new();
    for (name, path, exe) in apps {
        let exe_path = exe.clone().unwrap_or_default();
        let exe_name = Path::new(&exe_path).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let hits: Vec<Hit> = packs
            .iter()
            .filter(|p| pack_matches(p, &exe_name, name))
            .flat_map(|p| {
                // Shortcut without a file target (e.g. File Explorer's shell PIDL): use the pack's exe via PATH.
                let exe = if exe_path.is_empty() { p.exe.first().cloned().unwrap_or_default() } else { exe_path.clone() };
                p.actions.iter().filter(|d| d.uri.is_some() || !exe.is_empty()).map(|d| action_hit(d, &exe)).collect::<Vec<_>>()
            })
            .collect();
        if !hits.is_empty() {
            out.insert(path.to_lowercase(), hits);
        }
    }
    out
}

/// Load packs (AppData override replaces the bundled file) and activate them for indexed apps.
/// Returns the number of apps with actions.
pub fn activate(override_file: &Path) -> usize {
    let json = std::fs::read_to_string(override_file).unwrap_or_else(|_| DEFAULT_PACKS.to_string());
    let packs = parse_packs(&json);
    let apps: Vec<_> = crate::apps::all().into_iter().map(|(n, p)| { let exe = crate::switcher::resolve(&p); (n, p, exe) }).collect();
    let map = activate_with(&packs, &apps);
    let n = map.len();
    *ACTIVE.write().unwrap() = map;
    n
}

/// Default program for an extension (lowercased exe path), e.g. ".rpp" -> "c:\program files\reaper\reaper.exe".
pub fn default_exe(ext: &str) -> Option<String> {
    let key = ext.to_lowercase();
    if let Some(hit) = ASSOC.lock().unwrap().get(&key) {
        return hit.clone();
    }
    let exe = unsafe { assoc_executable(&key) }.map(|s| s.to_lowercase());
    ASSOC.lock().unwrap().insert(key, exe.clone());
    exe
}

unsafe fn assoc_executable(ext: &str) -> Option<String> {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::UI::Shell::{AssocQueryStringW, ASSOCF_NONE, ASSOCSTR_EXECUTABLE};
    let wide: Vec<u16> = ext.encode_utf16().chain([0]).collect();
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    unsafe {
        AssocQueryStringW(ASSOCF_NONE, ASSOCSTR_EXECUTABLE, PCWSTR(wide.as_ptr()), PCWSTR::null(), Some(PWSTR(buf.as_mut_ptr())), &mut len)
            .ok()
            .ok()?;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let s = String::from_utf16_lossy(&buf[..end]);
    // Store/UWP handlers come back as a package-relative stub, not a usable exe path.
    (s.to_lowercase().ends_with(".exe") && Path::new(&s).is_absolute()).then_some(s)
}

/// Same exe, or the same install folder (e.g. Start Menu -> blender.exe, .blend -> blender-launcher.exe).
fn exe_matches(app_exe: &str, assoc_exe: &str) -> bool {
    app_exe == assoc_exe || (Path::new(app_exe).parent().is_some_and(|d| Some(d) == Path::new(assoc_exe).parent()))
}

/// Group recent files by their default app. Pure (inputs injected) so it's testable.
fn group(files: Vec<(SystemTime, PathBuf)>, assoc: impl Fn(&str) -> Option<String>) -> HashMap<String, Vec<Recent>> {
    let mut map: HashMap<String, Vec<Recent>> = HashMap::new();
    for (modified, p) in files {
        let Some(ext) = p.extension().map(|e| format!(".{}", e.to_string_lossy())) else { continue };
        if matches!(ext.to_lowercase().as_str(), ".lnk" | ".exe" | ".url") {
            continue;
        }
        let Some(exe) = assoc(&ext) else { continue };
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        map.entry(exe).or_default().push(Recent { modified, name, path: p.to_string_lossy().into_owned() });
    }
    for v in map.values_mut() {
        v.sort_by(|a, b| b.modified.cmp(&a.modified));
        v.dedup_by(|a, b| a.path.eq_ignore_ascii_case(&b.path));
        v.truncate(RECENT_PER_APP);
    }
    map
}

/// Rescan %APPDATA%\Microsoft\Windows\Recent (background thread). Returns files kept.
pub fn scan_recent() -> usize {
    let Some(dir) = std::env::var("APPDATA").ok().map(|a| PathBuf::from(a).join(r"Microsoft\Windows\Recent")) else { return 0 };
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    let mut lnks: Vec<(SystemTime, PathBuf)> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")))
        .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
        .collect();
    lnks.sort_by(|a, b| b.0.cmp(&a.0));
    lnks.truncate(RECENT_SCAN_LIMIT);
    // The Recent shortcut's mtime = when the file was last opened. Resolve only new/changed shortcuts.
    let mut cache = LNK_CACHE.lock().unwrap();
    let files: Vec<(SystemTime, PathBuf)> = lnks
        .into_iter()
        .filter_map(|(t, l)| {
            let key = l.to_string_lossy().to_lowercase();
            let target = match cache.get(&key) {
                Some((m, target)) if *m == t => target.clone(),
                _ => {
                    let target = crate::switcher::lnk_target(&l.to_string_lossy());
                    cache.insert(key, (t, target.clone()));
                    target
                }
            };
            target.map(|f| (t, PathBuf::from(f)))
        })
        .filter(|(_, f)| f.is_file())
        .collect();
    drop(cache);
    let map = group(files, default_exe);
    let n = map.values().map(Vec::len).sum();
    *RECENT.write().unwrap() = map;
    n
}

/// Default order: primary action, recent files (newest first), remaining actions. The brain re-orders.
pub fn for_app(app_path: &str) -> Vec<Hit> {
    let actions = ACTIVE.read().unwrap().get(&app_path.to_lowercase()).cloned().unwrap_or_default();
    let mut recent_hits = Vec::new();
    if let Some(app_exe) = crate::switcher::resolve(app_path) {
        let recent = RECENT.read().unwrap();
        let mut files: Vec<&Recent> = recent.iter().filter(|(exe, _)| exe_matches(&app_exe, exe)).flat_map(|(_, v)| v).collect();
        files.sort_by(|a, b| b.modified.cmp(&a.modified));
        recent_hits = files
            .into_iter()
            .take(RECENT_PER_APP)
            .map(|r| Hit { name: r.name.clone(), path: r.path.clone(), kind: "recent", detail: Some("Recent".into()), child: true, ..Default::default() })
            .collect();
    }
    let mut out = Vec::new();
    let mut actions = actions.into_iter();
    out.extend(actions.next());
    out.extend(recent_hits);
    out.extend(actions);
    out
}

/// Run an `act:` sub-action: the app's exe with args (cwd = exe folder), or a URI with its handler.
pub fn launch(path: &str) -> Result<(), String> {
    let json = path.strip_prefix("act:").ok_or("not an action")?;
    match serde_json::from_str::<Launch>(json).map_err(|e| e.to_string())? {
        Launch::Uri { uri } => tauri_plugin_opener::open_url(uri, None::<&str>).map_err(|e| e.to_string()),
        Launch::Exe { exe, args } => {
            let mut cmd = std::process::Command::new(&exe);
            cmd.args(&args);
            if let Some(dir) = Path::new(&exe).parent() {
                cmd.current_dir(dir); // e.g. OBS refuses to start outside its bin folder
            }
            cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn t(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn groups_recent_files_by_default_app() {
        let assoc = |ext: &str| match ext.to_lowercase().as_str() {
            ".rpp" => Some(r"c:\reaper\reaper.exe".to_string()),
            ".blend" => Some(r"c:\blender\blender-launcher.exe".to_string()),
            _ => None,
        };
        let files = vec![
            (t(1), PathBuf::from(r"D:\m\old.rpp")),
            (t(9), PathBuf::from(r"D:\m\new.RPP")),
            (t(5), PathBuf::from(r"D:\b\scene.blend")),
            (t(7), PathBuf::from(r"D:\x\unknown.xyz")),
            (t(8), PathBuf::from(r"D:\x\noext")),
            (t(9), PathBuf::from(r"D:\x\app.exe")),
        ];
        let mut files = files;
        for i in 0..8 {
            files.push((t(2), PathBuf::from(format!(r"D:\m\bulk{i}.rpp"))));
        }
        let map = group(files, assoc);
        let reaper: Vec<_> = map[r"c:\reaper\reaper.exe"].iter().map(|r| r.name.as_str()).collect();
        assert_eq!(reaper.len(), RECENT_PER_APP);
        assert_eq!(reaper[0], "new.RPP", "newest first");
        assert_eq!(map[r"c:\blender\blender-launcher.exe"][0].name, "scene.blend");
        assert_eq!(map.len(), 2, "unassociated / extensionless / exe skipped");
        // Start Menu points at blender.exe, .blend opens blender-launcher.exe in the same folder.
        assert!(exe_matches(r"c:\blender\blender.exe", r"c:\blender\blender-launcher.exe"));
        assert!(!exe_matches(r"c:\reaper\reaper.exe", r"c:\blender\blender.exe"));
    }

    #[test]
    fn packs_activate_only_for_indexed_apps() {
        let packs = parse_packs(DEFAULT_PACKS);
        assert!(packs.len() >= 10, "bundled actions.json parses");
        let apps = vec![
            ("REAPER (x64)".to_string(), r"C:\SM\REAPER (x64).lnk".to_string(), Some(r"c:\reaper\reaper.exe".to_string())),
            ("Discord".to_string(), r"C:\SM\Discord.lnk".to_string(), Some(r"c:\discord\update.exe".to_string())),
            ("Notepad".to_string(), r"C:\SM\Notepad.lnk".to_string(), Some(r"c:\windows\notepad.exe".to_string())),
        ];
        let map = activate_with(&packs, &apps);
        let reaper = &map[r"c:\sm\reaper (x64).lnk"];
        assert_eq!(reaper[0].name, "Start new project");
        assert_eq!(
            serde_json::from_str::<Launch>(reaper[0].path.strip_prefix("act:").unwrap()).unwrap(),
            Launch::Exe { exe: r"c:\reaper\reaper.exe".into(), args: vec!["-new".into()] }
        );
        assert!(map[r"c:\sm\discord.lnk"][0].path.contains("discord://"), "matched by name, uri action");
        assert!(!map.contains_key(r"c:\sm\notepad.lnk"));
        assert!(map.values().flatten().all(|h| h.child && h.kind == "action"));
    }

    // cargo test --lib live_subactions -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_subactions() {
        for ext in [".rpp", ".blend", ".uproject", ".psd", ".txt", ".pdf", ".flp", ".png"] {
            println!("{ext:>9} -> {:?}", default_exe(ext));
        }
        crate::apps::index();
        crate::apps::prewarm_targets();
        let t = std::time::Instant::now();
        println!("recent files kept: {} in {:?}", scan_recent(), t.elapsed());
        let t = std::time::Instant::now();
        println!("rescan (cached): {} in {:?}", scan_recent(), t.elapsed());
        println!("apps with action packs: {}", activate(Path::new("nope.json")));
        for q in ["reaper", "blender", "visual studio code", "spotify", "file explorer", "photoshop"] {
            if let Some(app) = crate::apps::search(q, 1).pop() {
                let subs: Vec<_> = for_app(&app.path).into_iter().map(|h| format!("{} [{}]", h.name, h.kind)).collect();
                println!("{} -> {subs:?}", app.name);
            }
        }
    }
}
