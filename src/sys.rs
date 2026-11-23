//! Small Windows helpers that are not part of the app itself: strings,
//! the time zone, the console, opening things, the start-with-Windows entry
//! and the single-instance guard.

use std::path::Path;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, FILETIME, GENERIC_READ, GENERIC_WRITE, HANDLE,
    HWND, SYSTEMTIME,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Console::{
    AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE, STD_HANDLE,
    STD_OUTPUT_HANDLE,
};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::System::Time::{
    FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTimeEx,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// NUL-terminated UTF-16 copy of a string.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The string in a NUL-terminated (or full) UTF-16 buffer.
pub fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

const EPOCH_DIFF_100NS: i64 = 116_444_736_000_000_000;

fn to_filetime(utc: i64) -> FILETIME {
    let v = (utc * 10_000_000 + EPOCH_DIFF_100NS).max(0) as u64;
    FILETIME {
        dwLowDateTime: v as u32,
        dwHighDateTime: (v >> 32) as u32,
    }
}

fn from_filetime(ft: &FILETIME) -> i64 {
    let v = ((ft.dwHighDateTime as u64) << 32 | ft.dwLowDateTime as u64) as i64;
    (v - EPOCH_DIFF_100NS).div_euclid(10_000_000)
}

/// Seconds east of UTC that local time had (or will have) at a UTC instant,
/// by the rules of the time zone Windows is set to, including the
/// daylight-saving rules of that year.
pub fn utc_offset_at(utc: i64) -> i32 {
    let ft = to_filetime(utc);
    let mut st = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    let mut local_ft = FILETIME::default();
    // SAFETY: plain conversions between structures owned by this function.
    unsafe {
        if FileTimeToSystemTime(&ft, &mut st).is_err()
            || SystemTimeToTzSpecificLocalTimeEx(None, &st, &mut local).is_err()
            || SystemTimeToFileTime(&local, &mut local_ft).is_err()
        {
            return 0;
        }
    }
    (from_filetime(&local_ft) - utc).clamp(-16 * 3600, 16 * 3600) as i32
}

unsafe fn std_handle_usable(which: STD_HANDLE) -> bool {
    match GetStdHandle(which) {
        Ok(h) => !h.0.is_null() && !h.is_invalid(),
        Err(_) => false,
    }
}

/// The release build is a windowed program without a console. When it is
/// started from a terminal for a command (`--report`, `--selftest`), this
/// connects its output to that terminal. Redirected output is left alone.
pub fn attach_parent_console() {
    // SAFETY: console API calls with constant arguments; a handle opened here
    // is handed over to the process's standard handle table.
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
            return;
        }
        for which in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            if std_handle_usable(which) {
                continue;
            }
            if let Ok(console) = CreateFileW(
                w!("CONOUT$"),
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                HANDLE::default(),
            ) {
                let _ = SetStdHandle(which, console);
            }
        }
    }
}

/// Opens a file or folder with whatever Windows associates with it.
pub fn open_path(path: &Path) {
    let path = wide(&path.to_string_lossy());
    // SAFETY: NUL-terminated strings that outlive the call.
    unsafe {
        ShellExecuteW(
            HWND::default(),
            w!("open"),
            PCWSTR(path.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

/// Holds the named mutex that marks the running app.
pub struct InstanceGuard(HANDLE);

// SAFETY: a mutex handle may be closed from any thread.
unsafe impl Send for InstanceGuard {}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // SAFETY: the handle was created by `single_instance` and is closed once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Claims the single-instance mutex. `None` means another PowerForge with the
/// same `name` is already running.
pub fn single_instance(name: &str) -> Option<InstanceGuard> {
    let name = wide(name);
    // SAFETY: creating a named mutex; the name outlives the call.
    unsafe {
        let handle = CreateMutexW(None, false, PCWSTR(name.as_ptr())).ok()?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(handle);
            return None;
        }
        Some(InstanceGuard(handle))
    }
}

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
/// The value name of the start-with-Windows entry.
pub const RUN_VALUE: &str = "PowerForge";

/// The command line stored in the start-with-Windows entry.
pub fn startup_command() -> String {
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "PowerForge.exe".into());
    format!("\"{exe}\" --background")
}

/// Whether the per-user start-with-Windows entry `value` exists.
pub fn startup_enabled(value: &str) -> bool {
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(RUN_KEY)
        .and_then(|k| k.get_value::<String, _>(value))
        .is_ok()
}

/// Adds or removes the per-user start-with-Windows entry `value`.
pub fn set_startup(value: &str, on: bool) -> std::io::Result<()> {
    let (key, _) =
        winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).create_subkey(RUN_KEY)?;
    if on {
        key.set_value(value, &startup_command())
    } else {
        match key.delete_value(value) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_and_times() {
        assert_eq!(from_wide(&wide("héllo")), "héllo");
        assert_eq!(from_filetime(&to_filetime(1_700_000_000)), 1_700_000_000);
        assert!(utc_offset_at(1_700_000_000).abs() <= 16 * 3600);
    }

    #[test]
    fn single_instance_guard() {
        let name = format!("PowerForge.Test.{}", std::process::id());
        let first = single_instance(&name);
        assert!(first.is_some());
        assert!(single_instance(&name).is_none());
        drop(first);
        assert!(single_instance(&name).is_some());
    }

    #[test]
    fn startup_entry_round_trip() {
        // A throwaway value name: the real entry is never touched by tests.
        let value = format!("PowerForgeTest{}", std::process::id());
        assert!(!startup_enabled(&value));
        set_startup(&value, true).unwrap();
        assert!(startup_enabled(&value));
        set_startup(&value, false).unwrap();
        assert!(!startup_enabled(&value));
        set_startup(&value, false).unwrap();
        assert!(startup_command().ends_with("--background"));
    }
}
