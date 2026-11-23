//! Running `powercfg.exe` (and other system tools such as `wevtutil`)
//! without a console window and capturing the output.

use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Output of a finished command.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Output {
    pub code: i32,
    /// stdout and stderr, decoded.
    pub text: String,
}

/// `%SystemRoot%\System32\<exe>`.
pub fn system_tool(exe: &str) -> PathBuf {
    std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\Windows"))
        .join("System32")
        .join(exe)
}

/// Runs `%SystemRoot%\System32\powercfg.exe` with `args`, hidden, with a
/// timeout.
pub fn run(args: &[&str], timeout_secs: u64) -> Result<Output, String> {
    run_system_tool("powercfg.exe", args, timeout_secs)
}

/// Runs any program from System32 the same way (e.g. "wevtutil.exe").
pub fn run_system_tool(exe: &str, args: &[&str], timeout_secs: u64) -> Result<Output, String> {
    let mut child = Command::new(system_tool(exe))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("{exe} could not be started: {e}"))?;
    // Read both pipes on threads so a full pipe never blocks the child.
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let t_out = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(o) = out.as_mut() {
            let _ = o.read_to_end(&mut buf);
        }
        buf
    });
    let t_err = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(e) = err.as_mut() {
            let _ = e.read_to_end(&mut buf);
        }
        buf
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if start.elapsed() > Duration::from_secs(timeout_secs.max(1)) {
                    // Only the child this function started.
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "{exe} did not finish within {timeout_secs} seconds"
                    ));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    let mut bytes = t_out.join().unwrap_or_default();
    let err_bytes = t_err.join().unwrap_or_default();
    if !err_bytes.is_empty() {
        bytes.extend_from_slice(b"\n");
        bytes.extend_from_slice(&err_bytes);
    }
    Ok(Output {
        code: status.code().unwrap_or(-1),
        text: decode(&bytes),
    })
}

/// Decodes console output: UTF-16LE (with or without BOM), UTF-8, or the
/// OEM code page.
pub fn decode(bytes: &[u8]) -> String {
    let utf16 = bytes.starts_with(&[0xFF, 0xFE])
        || (bytes.len() >= 4 && bytes.len().is_multiple_of(2) && bytes[1] == 0 && bytes[3] == 0);
    let text = if utf16 {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        String::from_utf16_lossy(&units)
            .trim_start_matches('\u{feff}')
            .to_string()
    } else if let Ok(s) = std::str::from_utf8(bytes) {
        s.to_string()
    } else {
        oem_to_string(bytes)
    };
    text.replace("\r\n", "\n")
}

fn oem_to_string(bytes: &[u8]) -> String {
    use windows::Win32::Globalization::{
        MultiByteToWideChar, CP_OEMCP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS,
    };
    // SAFETY: the output buffer is sized by the first call.
    unsafe {
        let n = MultiByteToWideChar(CP_OEMCP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0), bytes, None);
        if n <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let mut buf = vec![0u16; n as usize];
        let n = MultiByteToWideChar(
            CP_OEMCP,
            MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0),
            bytes,
            Some(&mut buf),
        );
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

/// Whether the output says administrator rights are required.
pub fn needs_admin(text: &str) -> bool {
    let t = text.to_lowercase();
    t.contains("administrator privileges")
        || t.contains("elevated command prompt")
        || t.contains("requires administrator")
        || t.contains("access is denied")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoding() {
        assert_eq!(decode(b"Hello\r\nWorld"), "Hello\nWorld");
        let utf16: Vec<u8> = "Hi there"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        assert_eq!(decode(&utf16), "Hi there");
        let mut bom = vec![0xFF, 0xFE];
        bom.extend_from_slice(&utf16);
        assert_eq!(decode(&bom), "Hi there");
        // Invalid UTF-8 goes through the OEM code page without panicking.
        assert!(!decode(&[0x82, 0x41]).is_empty());
    }

    #[test]
    fn admin_detection() {
        assert!(needs_admin(
            "This command requires administrator privileges and must be executed from an elevated command prompt."
        ));
        assert!(!needs_admin("Wake History Count - 1"));
    }

    #[test]
    fn runs_read_only_query() {
        // `powercfg /list` only reads.
        let out = run(&["/list"], 30).unwrap();
        assert_eq!(out.code, 0);
        assert!(!out.text.trim().is_empty());
    }
}
