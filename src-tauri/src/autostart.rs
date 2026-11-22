//! Start with Windows via HKCU\...\Run (per-user, no admin). The registry is the source of truth.
//! shortcut: in `tauri dev` this registers the debug exe (needs the dev server); fine for testing,
//! the installed build registers its own exe.
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Registry::{RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ};

const RUN_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const VALUE: &str = "STLR Search";

pub fn is_enabled() -> bool {
    exists(RUN_KEY, VALUE)
}

pub fn set(enabled: bool) -> Result<(), String> {
    if enabled {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        write(RUN_KEY, VALUE, &format!("\"{}\"", exe.display()))
    } else {
        delete(RUN_KEY, VALUE)
    }
}

fn exists(key: PCWSTR, name: &str) -> bool {
    unsafe { RegGetValueW(HKEY_CURRENT_USER, key, &HSTRING::from(name), RRF_RT_REG_SZ, None, None, None).is_ok() }
}

fn write(key: PCWSTR, name: &str, data: &str) -> Result<(), String> {
    let wide: Vec<u16> = data.encode_utf16().chain([0]).collect();
    unsafe {
        RegSetKeyValueW(HKEY_CURRENT_USER, key, &HSTRING::from(name), REG_SZ.0, Some(wide.as_ptr() as _), (wide.len() * 2) as u32)
    }
    .ok()
    .map_err(|e| e.to_string())
}

fn delete(key: PCWSTR, name: &str) -> Result<(), String> {
    let r = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key, &HSTRING::from(name)) };
    if r == ERROR_FILE_NOT_FOUND { Ok(()) } else { r.ok().map_err(|e| e.to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Uses a throwaway HKCU key, never the real Run key (a crashed test must not leave an autostart entry).
    #[test]
    fn registry_value_lifecycle() {
        let key = w!(r"Software\STLR-SearchEngine-Test");
        let name = format!("test-{}", std::process::id());
        assert!(!exists(key, &name));
        write(key, &name, r#""C:\x\app.exe""#).unwrap();
        assert!(exists(key, &name));
        delete(key, &name).unwrap();
        assert!(!exists(key, &name));
        delete(key, &name).unwrap(); // idempotent
        unsafe {
            let _ = windows::Win32::System::Registry::RegDeleteKeyW(HKEY_CURRENT_USER, key);
        }
    }
}
