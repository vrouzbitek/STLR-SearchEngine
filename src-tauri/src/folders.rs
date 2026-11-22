//! Windows known folders (Downloads, Documents, Desktop...) by PL/EN alias. Paths are resolved once via
//! SHGetKnownFolderPath, so moved / OneDrive-redirected folders open where they really are.
use crate::Hit;
use std::sync::OnceLock;
use windows::core::GUID;
use windows::Win32::UI::Shell::*;

/// Above every match score (an exact Everything "Downloads" hit scores 0), below learned pins (-10..).
const FOLDER_SCORE: f32 = -1.0;

const FOLDERS: [(&GUID, &str, &[&str]); 12] = [
    (&FOLDERID_Downloads, "Downloads", &["downloads", "download", "pobrane"]),
    (&FOLDERID_Documents, "Documents", &["documents", "my documents", "dokumenty", "moje dokumenty"]),
    (&FOLDERID_Desktop, "Desktop", &["desktop", "pulpit"]),
    (&FOLDERID_Pictures, "Pictures", &["pictures", "obrazy", "zdjęcia", "zdjecia"]),
    (&FOLDERID_Screenshots, "Screenshots", &["screenshots", "zrzuty ekranu"]),
    (&FOLDERID_Videos, "Videos", &["videos", "wideo", "filmy"]),
    (&FOLDERID_Music, "Music", &["music", "muzyka"]),
    (&FOLDERID_Profile, "User folder", &["home", "user folder", "folder użytkownika", "folder uzytkownika"]),
    (&FOLDERID_RoamingAppData, "AppData (Roaming)", &["appdata", "roaming"]),
    (&FOLDERID_LocalAppData, "AppData (Local)", &["localappdata", "appdata local"]),
    (&FOLDERID_Startup, "Startup folder", &["startup", "autostart", "autostart folder"]),
    (&FOLDERID_ProgramFiles, "Program Files", &["program files", "programy"]),
];

fn resolve(id: &GUID) -> Option<String> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as _));
        s
    }
}

/// (name, aliases, path) for folders that exist on this machine.
fn all() -> &'static [(&'static str, &'static [&'static str], String)] {
    static ALL: OnceLock<Vec<(&str, &[&str], String)>> = OnceLock::new();
    ALL.get_or_init(|| FOLDERS.iter().filter_map(|(id, name, al)| Some((*name, *al, resolve(id)?))).collect())
}

/// Exact alias, or alias prefix once the query has 3+ chars ("pob" -> Downloads), like system commands.
pub fn search(q: &str) -> Vec<Hit> {
    let long_enough = q.chars().count() >= 3;
    all()
        .iter()
        .filter(|(_, al, _)| al.iter().any(|a| *a == q || (long_enough && a.starts_with(q))))
        .map(|(name, _, path)| Hit { name: name.to_string(), path: path.clone(), kind: "folder", score: FOLDER_SCORE, ..Default::default() })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_folders() {
        let dl = search("pobrane");
        assert_eq!(dl.len(), 1);
        assert!(std::path::Path::new(&dl[0].path).is_dir(), "{}", dl[0].path);
        assert_eq!(search("pob")[0].path, dl[0].path);
        assert_eq!(search("downloads")[0].path, dl[0].path);
        assert_eq!(search("moje dokumenty")[0].name, "Documents");
        assert!(search("do").is_empty(), "too short for prefix match");
        assert!(search("spotify").is_empty());
        assert_eq!(all().len(), FOLDERS.len(), "every known folder resolves");
    }
}
