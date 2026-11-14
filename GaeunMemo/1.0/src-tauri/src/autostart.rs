//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` handling.

pub const AUTOSTART_ARG: &str = "--autostart";

#[cfg(windows)]
mod imp {
    use super::AUTOSTART_ARG;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};
    use winreg::RegKey;

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const NAME: &str = "gaeunmemo";

    fn key(write: bool) -> Option<RegKey> {
        let flags = if write { KEY_READ | KEY_SET_VALUE } else { KEY_READ };
        RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN, flags).ok()
    }

    pub fn is_enabled() -> bool {
        key(false).map(|k| k.get_value::<String, _>(NAME).is_ok()).unwrap_or(false)
    }

    pub fn set_enabled(on: bool) {
        let Some(k) = key(true) else { return };
        if on {
            if let Ok(exe) = std::env::current_exe() {
                let _ = k.set_value(NAME, &format!("\"{}\" {}", exe.display(), AUTOSTART_ARG));
            }
        } else {
            let _ = k.delete_value(NAME);
        }
    }

    /// Removes Run entries left behind by the old StickyMemo app.
    pub fn remove_legacy() {
        let Some(k) = key(true) else { return };
        let stale: Vec<String> = k
            .enum_values()
            .filter_map(|v| v.ok())
            .filter(|(name, val)| {
                if name.eq_ignore_ascii_case(NAME) {
                    return false;
                }
                let data = val.to_string().to_ascii_lowercase();
                name.to_ascii_lowercase().contains("stickymemo") || data.contains("stickymemo")
            })
            .map(|(name, _)| name)
            .collect();
        for name in stale {
            let _ = k.delete_value(name);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn is_enabled() -> bool {
        false
    }
    pub fn set_enabled(_: bool) {}
    pub fn remove_legacy() {}
}

pub use imp::*;
