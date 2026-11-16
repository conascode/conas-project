//! "컴퓨터 켤 때 자동 실행" (F-19, F-35): a value under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, like v1.1's autostart.rs.
//!
//! Development builds use their own value name `GaeunMemo2-dev` (user decision 2026-11-16):
//! v1.1's `gaeunmemo` value is never read, changed or removed, so both apps start at logon
//! while v2 is being built. Taking over `gaeunmemo` is the installer's job (P7). The old
//! StickyMemo clean-up of v1.1 is left out for the same reason (it deletes other values).
//! Only the interactive app calls this; test and measurement modes never touch the registry.

use crate::win::wide;
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub const NAME: &str = "GaeunMemo2-dev";
/// Given to the app when Windows starts it: it opens what was open, without taking focus.
pub const ARG: &str = "--autostart";

/// The command line Windows runs at logon: this exe, `--autostart`, and the data folder
/// when it is not the default one.
pub fn command(data: Option<&std::path::Path>) -> String {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    match data {
        Some(d) => format!("\"{exe}\" {ARG} --data \"{}\"", d.display()),
        None => format!("\"{exe}\" {ARG}"),
    }
}

/// The value as it is now, if there is one.
pub fn current() -> Option<String> {
    current_in(RUN)
}

/// `current` under another key (tests use their own key, never the real Run key).
pub fn current_in(run: &str) -> Option<String> {
    let (key, name) = (wide(run), wide(NAME));
    let mut buf = vec![0u16; 2048];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr() as *mut _, &mut size) };
    if r != ERROR_SUCCESS {
        return None;
    }
    let n = (size as usize / 2).saturating_sub(1).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..n]))
}

pub fn is_enabled() -> bool {
    current().is_some()
}

/// Writes (`on`) or removes the value; true when the registry now says what was asked.
pub fn set_enabled(on: bool, data: Option<&std::path::Path>) -> bool {
    set_enabled_in(RUN, on, data)
}

pub fn set_enabled_in(run: &str, on: bool, data: Option<&std::path::Path>) -> bool {
    let (key, name) = (wide(run), wide(NAME));
    unsafe {
        if on {
            let v = wide(&command(data));
            let r = RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), REG_SZ, v.as_ptr() as *const _, (v.len() * 2) as u32);
            r == ERROR_SUCCESS
        } else {
            let r = RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr());
            r == ERROR_SUCCESS || current_in(run).is_none()
        }
    }
}

/// What start-up does about autostart (v1.1 main.rs rules, F-35). `saved` is the stored
/// choice ("1" / "0" / none), `first_run` the very first start of this data folder. Returns
/// the choice to store (if it changed) and the registry action, separately so they can be
/// tested without the registry.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    None,
    /// write the value (first run, or the value was removed behind the app's back)
    Register,
    /// the value points at another copy of the exe (a development build moved): rewrite it
    Update,
}

pub fn plan(saved: Option<&str>, first_run: bool, value: Option<&str>, wanted: &str) -> (Option<bool>, Action) {
    match saved {
        // no choice stored yet (first start, or data from before P4): on for a first run,
        // otherwise whatever the registry says now
        None => {
            let on = first_run || value.is_some();
            (Some(on), if on && value.is_none() { Action::Register } else if on && value != Some(wanted) { Action::Update } else { Action::None })
        }
        Some("1") => match value {
            None => (None, Action::Register),
            Some(v) if v != wanted => (None, Action::Update),
            _ => (None, Action::None),
        },
        _ => (None, Action::None),
    }
}

#[cfg(test)]
mod tests {
    use super::{plan, Action};

    #[test]
    fn first_run_turns_it_on() {
        assert_eq!(plan(None, true, None, "x"), (Some(true), Action::Register));
    }

    #[test]
    fn data_from_before_takes_the_registry_as_it_is() {
        assert_eq!(plan(None, false, None, "x"), (Some(false), Action::None));
        assert_eq!(plan(None, false, Some("x"), "x"), (Some(true), Action::None));
    }

    #[test]
    fn a_removed_value_comes_back_when_the_user_wanted_it() {
        assert_eq!(plan(Some("1"), false, None, "x"), (None, Action::Register));
    }

    #[test]
    fn a_moved_exe_is_followed() {
        assert_eq!(plan(Some("1"), false, Some("old"), "x"), (None, Action::Update));
    }

    #[test]
    fn off_stays_off() {
        assert_eq!(plan(Some("0"), false, None, "x"), (None, Action::None));
        assert_eq!(plan(Some("0"), true, None, "x"), (None, Action::None));
    }
}
