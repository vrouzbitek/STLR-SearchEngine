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

/// Exact alias, or alias prefix once the query has 3+ chars (so "s" doesn't surface "Shut down").
pub fn search(q: &str) -> Vec<Hit> {
    let q = q.trim();
    let long_enough = q.chars().count() >= 3;
    CMDS.iter()
        .filter(|c| c.aliases.iter().any(|a| *a == q || (long_enough && a.starts_with(q))))
        .map(|c| Hit {
            name: c.name.into(),
            path: format!("sys:{}", c.id),
            kind: "system",
            score: SYSTEM_SCORE,
            detail: Some(if c.confirm { "Enter twice to confirm".into() } else { "System".into() }),
            ..Default::default()
        })
        .collect()
}

pub fn run(path: &str, confirmed: bool) -> Result<(), String> {
    let id = path.strip_prefix("sys:").unwrap_or(path);
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
    fn destructive_commands_need_confirmation() {
        for id in ["sys:shutdown", "sys:restart", "sys:empty-bin", "sys:sign-out"] {
            assert_eq!(run(id, false), Err("confirmation required".into()));
        }
        assert!(run("sys:nope", true).is_err());
    }
}
