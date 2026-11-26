//! Lifecycle log at %APPDATA%\com.stlr.searchengine\stlr.log. The release build has no console
//! (windows_subsystem), so a launch that dies quietly (e.g. right after Windows starts) leaves no
//! trace without it. Only rare lifecycle events go here (start, setup failures, panics, exit),
//! never anything per keystroke, so it costs nothing on the hot path.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const MAX_BYTES: u64 = 256 * 1024; // above this the old log moves to stlr.log.old
static PATH: OnceLock<PathBuf> = OnceLock::new();

fn path() -> PathBuf {
    PATH.get_or_init(|| {
        // Same folder as tauri's app_data_dir on Windows (Roaming AppData + bundle identifier).
        let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        base.join("com.stlr.searchengine").join("stlr.log")
    })
    .clone()
}

/// Append one timestamped line (also echoed to stderr for `tauri dev`). Never fails.
pub fn line(msg: &str) {
    eprintln!("{msg}");
    append(&path(), msg);
}

fn append(p: &Path, msg: &str) {
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(p).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        let _ = std::fs::rename(p, p.with_extension("log.old"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let _ = writeln!(f, "[{secs}] pid {} {msg}", std::process::id());
    }
}

/// Log panics before `panic = "abort"` kills the process (the hook runs before the abort).
pub fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        line(&format!("PANIC: {info}"));
        prev(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_and_rotates() {
        let dir = std::env::temp_dir().join(format!("stlr-log-test-{}", std::process::id()));
        let p = dir.join("stlr.log");
        append(&p, "first");
        append(&p, "second");
        let s = std::fs::read_to_string(&p).unwrap();
        assert!(s.contains("first") && s.contains("second") && s.lines().count() == 2);
        std::fs::write(&p, vec![b'x'; (MAX_BYTES + 1) as usize]).unwrap();
        append(&p, "after rotate");
        assert_eq!(std::fs::read_to_string(&p).unwrap().lines().count(), 1);
        assert!(p.with_extension("log.old").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
