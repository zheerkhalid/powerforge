//! Running one powercfg command as administrator, only when the user asks
//! for an action that needs it. PowerForge starts a second copy of itself
//! through the UAC prompt (`runas`) with `--elevated <job-file>`; that copy
//! runs only whitelisted powercfg commands, writes the output next to the
//! job file and exits. The job folder lives under %TEMP% and is deleted
//! afterwards. Commands that write an `/output` file are given a path the
//! caller chose inside %TEMP% (the caller reads and deletes it).

use crate::diag::DiagError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Whether this process already runs elevated.
pub fn is_elevated() -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    // SAFETY: querying our own token into a correctly sized structure; the
    // token handle is closed.
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Job {
    args: Vec<String>,
}

/// A new, empty folder under %TEMP% for one job.
pub fn temp_job_dir(tag: &str) -> std::io::Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("PowerForge-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Runs `powercfg <args>` elevated and returns its output. Blocking (waits
/// for the UAC prompt and the command); call from a worker thread. Only
/// argument lists accepted by [`allowed`] are run.
pub fn powercfg(args: &[String], timeout_secs: u64) -> Result<String, DiagError> {
    if !allowed(args) {
        return Err(DiagError::Failed("this command is not allowed".into()));
    }
    if is_elevated() {
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        return crate::powercfg::run(&refs, timeout_secs)
            .map(|o| o.text)
            .map_err(DiagError::Failed);
    }
    let dir = temp_job_dir("elev").map_err(|e| DiagError::Failed(e.to_string()))?;
    let result = run_job(&dir, args, timeout_secs);
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn run_job(dir: &Path, args: &[String], timeout_secs: u64) -> Result<String, DiagError> {
    let job = dir.join("job.json");
    let text = serde_json::to_vec(&Job {
        args: args.to_vec(),
    })
    .map_err(|e| DiagError::Failed(e.to_string()))?;
    std::fs::write(&job, text).map_err(|e| DiagError::Failed(e.to_string()))?;
    let exe = std::env::current_exe().map_err(|e| DiagError::Failed(e.to_string()))?;
    let params = format!("--elevated \"{}\"", job.display());
    let code = shell_runas(&exe, &params, timeout_secs)?;
    let out = std::fs::read(dir.join("out.txt")).unwrap_or_default();
    let out = String::from_utf8_lossy(&out).into_owned();
    if code != 0 && out.trim().is_empty() {
        return Err(DiagError::Failed(format!(
            "the command failed (exit code {code})"
        )));
    }
    Ok(out)
}

/// Starts `exe params` through the UAC prompt and waits for it. Returns the
/// exit code.
fn shell_runas(exe: &Path, params: &str, timeout_secs: u64) -> Result<u32, DiagError> {
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_CANCELLED, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SEE_MASK_NO_CONSOLE,
        SHELLEXECUTEINFOW,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
    let file = crate::sys::wide(&exe.to_string_lossy());
    let params = crate::sys::wide(params);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NO_CONSOLE | SEE_MASK_NOASYNC,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // SAFETY: the strings outlive the call; the process handle is waited on
    // and closed.
    unsafe {
        if ShellExecuteExW(&mut info).is_err() {
            return Err(if GetLastError() == ERROR_CANCELLED {
                DiagError::Declined
            } else {
                DiagError::Failed("the administrator copy could not be started".into())
            });
        }
        if info.hProcess.is_invalid() || info.hProcess.0.is_null() {
            return Err(DiagError::Failed("no process to wait for".into()));
        }
        let waited = WaitForSingleObject(info.hProcess, (timeout_secs.max(1) * 1000) as u32);
        let mut code = 1u32;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        if waited != WAIT_OBJECT_0 {
            return Err(DiagError::Failed("the command took too long".into()));
        }
        Ok(code)
    }
}

/// Whether `path` is a plain file path inside %TEMP%.
fn in_temp(path: &str) -> bool {
    if path.contains("..") || path.contains('"') || path.contains('\n') || path.contains('*') {
        return false;
    }
    let temp = std::env::temp_dir().to_string_lossy().to_lowercase();
    let temp = temp.trim_end_matches('\\');
    let p = path.to_lowercase();
    p.starts_with(&format!("{temp}\\")) && Path::new(path).is_absolute()
}

fn device_name_ok(name: &str) -> bool {
    !name.trim().is_empty()
        && name.chars().count() <= 256
        && !name.contains(['"', '\n', '\r', '\0', '%', '^', '&', '|', '<', '>'])
}

/// Validates `/output <file> /duration <n> /xml` style options.
fn options_ok(rest: &[String], max_duration: u32, need_output: bool) -> bool {
    let mut i = 0;
    let mut has_output = false;
    while i < rest.len() {
        match rest[i].to_lowercase().as_str() {
            "/xml" => i += 1,
            "/output" => {
                let Some(p) = rest.get(i + 1) else {
                    return false;
                };
                if !in_temp(p) {
                    return false;
                }
                has_output = true;
                i += 2;
            }
            "/duration" => {
                let Some(n) = rest.get(i + 1).and_then(|v| v.parse::<u32>().ok()) else {
                    return false;
                };
                if !(1..=max_duration).contains(&n) {
                    return false;
                }
                i += 2;
            }
            _ => return false,
        }
    }
    has_output || !need_output
}

/// The whitelist: /requests, /waketimers, /lastwake, /devicequery ...,
/// /deviceenablewake <name>, /devicedisablewake <name>, /hibernate on|off,
/// /energy (with /output inside %TEMP%, /xml, /duration N),
/// /sleepstudy (with /output inside %TEMP%).
pub fn allowed(args: &[String]) -> bool {
    let Some(first) = args.first() else {
        return false;
    };
    let rest = &args[1..];
    match first.to_lowercase().as_str() {
        "/requests" | "/waketimers" | "/lastwake" | "/a" => rest.is_empty(),
        "/devicequery" => {
            rest.len() == 1
                && matches!(
                    rest[0].to_lowercase().as_str(),
                    "wake_armed" | "wake_programmable" | "wake_from_any"
                )
        }
        "/deviceenablewake" | "/devicedisablewake" => rest.len() == 1 && device_name_ok(&rest[0]),
        "/hibernate" => rest.len() == 1 && matches!(rest[0].to_lowercase().as_str(), "on" | "off"),
        "/energy" => options_ok(rest, 300, true),
        "/sleepstudy" => options_ok(rest, 28, true),
        _ => false,
    }
}

/// Handles `--elevated <job-file>` in `main`. Returns the exit code when
/// the arguments were for this, `None` otherwise.
pub fn handle_cli(args: &[String]) -> Option<i32> {
    if args.first().map(|s| s.as_str()) != Some("--elevated") {
        return None;
    }
    Some(serve(args.get(1).map(|s| s.as_str())))
}

fn serve(job_path: Option<&str>) -> i32 {
    let Some(job_path) = job_path else { return 2 };
    if !in_temp(job_path) || !job_path.to_lowercase().ends_with("\\job.json") {
        return 2;
    }
    let Ok(bytes) = std::fs::read(job_path) else {
        return 2;
    };
    let Ok(job) = serde_json::from_slice::<Job>(&bytes) else {
        return 2;
    };
    if !allowed(&job.args) {
        return 3;
    }
    let dir = Path::new(job_path).parent().unwrap_or(Path::new("."));
    let refs: Vec<&str> = job.args.iter().map(|s| s.as_str()).collect();
    match crate::powercfg::run(&refs, 400) {
        Ok(out) => {
            let _ = std::fs::write(dir.join("out.txt"), out.text.as_bytes());
            let _ = std::fs::write(dir.join("code.txt"), out.code.to_string());
            if out.code == 0 {
                0
            } else {
                4
            }
        }
        Err(e) => {
            let _ = std::fs::write(dir.join("out.txt"), e.as_bytes());
            5
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn whitelist() {
        let temp = std::env::temp_dir();
        let out = temp.join("PowerForge-x").join("energy.xml");
        let out = out.to_string_lossy().to_string();
        assert!(allowed(&v(&["/requests"])));
        assert!(allowed(&v(&["/WAKETIMERS"])));
        assert!(!allowed(&v(&["/requests", "extra"])));
        assert!(allowed(&v(&["/devicequery", "wake_armed"])));
        assert!(!allowed(&v(&["/devicequery", "all_devices_verbose"])));
        assert!(allowed(&v(&[
            "/devicedisablewake",
            "Example Wireless Adapter"
        ])));
        assert!(!allowed(&v(&["/devicedisablewake", "x\" & del"])));
        assert!(!allowed(&v(&["/devicedisablewake", ""])));
        assert!(allowed(&v(&["/hibernate", "off"])));
        assert!(!allowed(&v(&["/hibernate", "size", "50"])));
        assert!(allowed(&v(&[
            "/energy",
            "/xml",
            "/output",
            &out,
            "/duration",
            "60"
        ])));
        assert!(!allowed(&v(&["/energy", "/output", "C:\\Windows\\x.xml"])));
        assert!(!allowed(&v(&[
            "/energy",
            "/output",
            &out,
            "/duration",
            "9999"
        ])));
        assert!(!allowed(&v(&["/energy", "/duration", "60"])));
        assert!(!allowed(&v(&[
            "/energy",
            "/output",
            &format!("{out}\\..\\..\\x")
        ])));
        assert!(allowed(&v(&["/sleepstudy", "/output", &out])));
        assert!(!allowed(&v(&["/setactive", "scheme_min"])));
        assert!(!allowed(&v(&["/delete", "x"])));
        assert!(!allowed(&[]));
    }

    #[test]
    fn cli_and_job_round_trip() {
        assert_eq!(handle_cli(&v(&["--demo"])), None);
        assert_eq!(handle_cli(&v(&[])), None);
        // Paths outside %TEMP% or not named job.json are refused before anything runs.
        assert_eq!(handle_cli(&v(&["--elevated", "C:\\job.json"])), Some(2));
        assert_eq!(handle_cli(&v(&["--elevated"])), Some(2));
        let dir = temp_job_dir("test").unwrap();
        let job = dir.join("job.json");
        // A job with a forbidden command is refused.
        std::fs::write(
            &job,
            serde_json::to_vec(&Job {
                args: v(&["/setactive", "x"]),
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            handle_cli(&v(&["--elevated", &job.to_string_lossy()])),
            Some(3)
        );
        // A read-only allowed command runs (no elevation involved here).
        std::fs::write(
            &job,
            serde_json::to_vec(&Job {
                args: v(&["/lastwake"]),
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            handle_cli(&v(&["--elevated", &job.to_string_lossy()])),
            Some(0)
        );
        let out = std::fs::read_to_string(dir.join("out.txt")).unwrap();
        assert!(out.contains("Wake"));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(!dir.exists());
    }
}
