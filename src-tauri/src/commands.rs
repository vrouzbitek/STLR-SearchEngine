//! System commands (the Start power/user menu + empty bin). Matched by PL/EN aliases, all in RAM.
use crate::Hit;
use std::os::windows::process::CommandExt;

/// Above every learned pin (-10..-14) and match score, below calculator / explicit URL:
/// a file can never push a system command down.
pub const SYSTEM_SCORE: f32 = -40.0;

struct Cmd {
    id: &'static str,
    name: &'static str,
    aliases: &'static [&'static str],
    /// Destructive / disruptive: requires a second Enter (frontend) and `confirmed = true` (backend).
    confirm: bool,
}

const CMDS: [Cmd; 8] = [
    Cmd { id: "lock", name: "Lock screen", aliases: &["lock", "lock screen", "zablokuj", "zablokuj ekran"], confirm: false },
    Cmd { id: "sleep", name: "Sleep", aliases: &["sleep", "suspend", "uśpij", "uspij"], confirm: false },
    Cmd { id: "hibernate", name: "Hibernate", aliases: &["hibernate", "hibernacja", "hibernuj", "zahibernuj"], confirm: false },
    Cmd {
        id: "switch-user",
        name: "Switch user",
        aliases: &["switch user", "change user", "zmień użytkownika", "zmien uzytkownika", "przełącz użytkownika", "przelacz uzytkownika"],
        confirm: false,
    },
    Cmd {
        id: "sign-out",
        name: "Sign out",
        aliases: &["sign out", "log out", "logout", "log off", "wyloguj", "wyloguj się", "wyloguj sie"],
        confirm: true,
    },
    Cmd { id: "shutdown", name: "Shut down", aliases: &["shutdown", "shut down", "power off", "wyłącz", "wylacz"], confirm: true },
    Cmd { id: "restart", name: "Restart", aliases: &["restart", "reboot", "uruchom ponownie"], confirm: true },
    Cmd {
        id: "empty-bin",
        name: "Empty Recycle Bin",
        aliases: &["empty bin", "empty recycle bin", "kosz", "opróżnij kosz", "oproznij kosz"],
        confirm: true,
    },
];

/// Windows settings pages and tools: (id, name, ShellExecute target, PL/EN aliases). Tools that already have
/// an English-named Start Menu shortcut (Task Manager, Registry Editor, Control Panel) get Polish aliases only,
/// so an English query doesn't show them twice.
const PAGES: [(&str, &str, &str, &[&str]); 27] = [
    ("security", "Windows Security", "windowsdefender:", &["windows security", "zabezpieczenia windows", "zabezpieczenia", "windows defender", "defender", "antywirus", "antivirus"]),
    ("settings", "Settings", "ms-settings:", &["settings", "windows settings", "ustawienia", "ustawienia windows"]),
    ("update", "Windows Update", "ms-settings:windowsupdate", &["windows update", "update", "aktualizacje", "aktualizacja", "aktualizacje windows"]),
    ("bluetooth", "Bluetooth & devices", "ms-settings:bluetooth", &["bluetooth", "urządzenia", "urzadzenia", "devices"]),
    ("wifi", "Wi-Fi", "ms-settings:network-wifi", &["wifi", "wi-fi", "sieć", "siec", "network", "internet"]),
    ("display", "Display", "ms-settings:display", &["display", "ekran", "wyświetlacz", "wyswietlacz", "rozdzielczość", "rozdzielczosc", "resolution", "skalowanie", "scaling"]),
    ("sound", "Sound", "ms-settings:sound", &["sound", "dźwięk", "dzwiek", "audio"]),
    ("apps", "Installed apps", "ms-settings:appsfeatures", &["installed apps", "uninstall", "zainstalowane aplikacje", "odinstaluj"]),
    ("default-apps", "Default apps", "ms-settings:defaultapps", &["default apps", "domyślne aplikacje", "domyslne aplikacje"]),
    ("startup-apps", "Startup apps", "ms-settings:startupapps", &["startup apps", "aplikacje autostartu"]),
    ("personalization", "Personalization", "ms-settings:personalization", &["personalization", "personalizacja", "tapeta", "wallpaper", "background", "tło", "tlo"]),
    ("taskbar", "Taskbar settings", "ms-settings:taskbar", &["taskbar", "pasek zadań", "pasek zadan"]),
    ("power", "Power & battery", "ms-settings:powersleep", &["power", "power settings", "battery", "zasilanie", "opcje zasilania", "bateria"]),
    ("storage", "Storage", "ms-settings:storagesense", &["storage", "disk space", "miejsce na dysku", "magazyn"]),
    ("time", "Date & time", "ms-settings:dateandtime", &["date and time", "time", "data i godzina", "godzina", "czas"]),
    ("language", "Language & region", "ms-settings:regionlanguage", &["language", "region", "język", "jezyk", "klawiatura", "keyboard layout"]),
    ("mouse", "Mouse", "ms-settings:mousetouchpad", &["mouse", "touchpad", "mysz", "myszka"]),
    ("notifications", "Notifications", "ms-settings:notifications", &["notifications", "powiadomienia"]),
    ("privacy", "Privacy & security", "ms-settings:privacy", &["privacy", "prywatność", "prywatnosc"]),
    ("accounts", "Accounts", "ms-settings:yourinfo", &["account", "accounts", "konto", "konta"]),
    ("about", "About this PC", "ms-settings:about", &["about", "about pc", "system info", "informacje o komputerze", "o komputerze", "specyfikacja"]),
    ("printers", "Printers & scanners", "ms-settings:printers", &["printers", "printer", "drukarki", "drukarka", "skaner"]),
    ("device-manager", "Device Manager", "devmgmt.msc", &["device manager", "menedżer urządzeń", "menedzer urzadzen", "sterowniki", "drivers"]),
    ("services", "Services", "services.msc", &["services", "usługi", "uslugi"]),
    ("task-manager", "Task Manager", "taskmgr.exe", &["menedżer zadań", "menedzer zadan"]),
    ("regedit", "Registry Editor", "regedit.exe", &["edytor rejestru", "rejestr"]),
    ("control", "Control Panel", "control.exe", &["panel sterowania"]),
];

/// An exact settings alias beats files; a prefix ranks just below app prefix matches (1), so "win" still
/// shows WinRAR before a dozen "windows ..." pages.
const PAGE_EXACT: f32 = -1.0;
const PAGE_PREFIX: f32 = 1.5;

/// Exact alias, or alias prefix once the query has 3+ chars (so "s" doesn't surface "Shut down").
pub fn search(q: &str) -> Vec<Hit> {
    let q = q.trim();
    let long_enough = q.chars().count() >= 3;
    let matches = |aliases: &[&str]| aliases.iter().any(|a| *a == q || (long_enough && a.starts_with(q)));
    let cmds = CMDS.iter().filter(|c| matches(c.aliases)).map(|c| Hit {
        name: c.name.into(),
        path: format!("sys:{}", c.id),
        kind: "system",
        score: SYSTEM_SCORE,
        detail: Some(if c.confirm { "Enter twice to confirm".into() } else { "System".into() }),
        ..Default::default()
    });
    let pages = PAGES.iter().filter(|(.., al)| matches(al)).map(|(id, name, _, al)| Hit {
        name: (*name).into(),
        path: format!("sys:{id}"),
        kind: "system",
        score: if al.contains(&q) { PAGE_EXACT } else { PAGE_PREFIX },
        detail: Some("Windows".into()),
        ..Default::default()
    });
    cmds.chain(pages).collect()
}

pub fn run(path: &str, confirmed: bool) -> Result<(), String> {
    let id = path.strip_prefix("sys:").unwrap_or(path);
    // Target comes from the table, never from the caller.
    if let Some((_, _, target, _)) = PAGES.iter().find(|p| p.0 == id) {
        return open(target);
    }
    let cmd = CMDS.iter().find(|c| c.id == id).ok_or_else(|| format!("unknown command {id}"))?;
    if cmd.confirm && !confirmed {
        return Err("confirmation required".into());
    }
    match id {
        "lock" => unsafe { windows::Win32::System::Shutdown::LockWorkStation() }.map_err(|e| e.to_string()),
        "sleep" => {
            // bHibernate = false -> sleep (S3/modern standby), not hibernate.
            let ok = unsafe { windows::Win32::System::Power::SetSuspendState(false, false, false) };
            if ok { Ok(()) } else { Err("SetSuspendState failed".into()) }
        }
        "hibernate" => {
            let ok = unsafe { windows::Win32::System::Power::SetSuspendState(true, false, false) };
            if ok { Ok(()) } else { Err("Hibernate is turned off on this PC (powercfg /h on)".into()) }
        }
        // What Start's "Switch user" does: leave the session running and go to the sign-in screen.
        // (tsdiscon.exe would too, but Windows Home doesn't ship it.)
        "switch-user" => unsafe {
            use windows::Win32::System::RemoteDesktop::{WTSDisconnectSession, WTS_CURRENT_SERVER_HANDLE, WTS_CURRENT_SESSION};
            WTSDisconnectSession(Some(WTS_CURRENT_SERVER_HANDLE), WTS_CURRENT_SESSION, false)
        }
        .map_err(|e| e.to_string()),
        "sign-out" => shutdown_exe(&["/l"]),
        "shutdown" => shutdown_exe(&["/s", "/t", "0"]),
        "restart" => shutdown_exe(&["/r", "/t", "0"]),
        "empty-bin" => {
            use windows::Win32::UI::Shell::{SHEmptyRecycleBinW, SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI, SHERB_NOSOUND};
            // Already-empty bin returns an error HRESULT; that's not a failure for the user.
            let _ = unsafe {
                SHEmptyRecycleBinW(None, windows::core::PCWSTR::null(), SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND)
            };
            Ok(())
        }
        _ => unreachable!(),
    }
}

/// ShellExecute handles URIs (ms-settings:, windowsdefender:), .msc snap-ins and exes on PATH alike, incl. UAC.
fn open(target: &str) -> Result<(), String> {
    use windows::core::{w, HSTRING};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let r = unsafe { ShellExecuteW(None, w!("open"), &HSTRING::from(target), None, None, SW_SHOWNORMAL) };
    if r.0 as isize > 32 { Ok(()) } else { Err(format!("could not open {target}")) }
}

fn shutdown_exe(args: &[&str]) -> Result<(), String> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("shutdown").args(args).creation_flags(CREATE_NO_WINDOW).spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(q: &str) -> Vec<String> {
        search(q).into_iter().map(|h| h.path).collect()
    }

    #[test]
    fn matching() {
        assert_eq!(ids("lock"), ["sys:lock"]);
        assert_eq!(ids("zabl"), ["sys:lock"]);
        assert_eq!(ids("wyłącz"), ["sys:shutdown"]);
        assert_eq!(ids("uruchom ponownie"), ["sys:restart"]);
        assert_eq!(ids("kosz"), ["sys:empty-bin"]);
        assert_eq!(ids("wyloguj"), ["sys:sign-out"]);
        assert_eq!(ids("log out"), ["sys:sign-out"]);
        assert_eq!(ids("zmień"), ["sys:switch-user"]);
        assert_eq!(ids("przelacz"), ["sys:switch-user"]);
        assert_eq!(ids("switch user"), ["sys:switch-user"]);
        assert_eq!(ids("hibernacja"), ["sys:hibernate"]);
        assert_eq!(ids("wyl"), ["sys:sign-out", "sys:shutdown"]);
        assert!(ids("s").is_empty() && ids("sh").is_empty(), "too short for prefix match");
        assert!(ids("spotify").is_empty());
        assert!(search("restart")[0].score < -14.0, "beats any learned pin");
    }

    #[test]
    fn windows_pages() {
        assert_eq!(ids("zabezpieczenia windows"), ["sys:security"]);
        assert_eq!(ids("windows security"), ["sys:security"]);
        assert_eq!(ids("zabezp"), ["sys:security"]);
        assert_eq!(ids("ustawienia"), ["sys:settings"]);
        assert_eq!(ids("menedżer urządzeń"), ["sys:device-manager"]);
        assert_eq!(search("bluetooth")[0].score, PAGE_EXACT);
        assert!(search("windows").iter().all(|h| h.score == PAGE_PREFIX), "prefix ranks below apps");
        let mut seen = std::collections::HashSet::new();
        for (id, ..) in PAGES {
            assert!(seen.insert(id) && !CMDS.iter().any(|c| c.id == id), "duplicate id {id}");
        }
    }

    #[test]
    fn destructive_commands_need_confirmation() {
        for id in ["sys:shutdown", "sys:restart", "sys:empty-bin", "sys:sign-out"] {
            assert_eq!(run(id, false), Err("confirmation required".into()));
        }
        assert!(run("sys:nope", true).is_err());
    }
}
