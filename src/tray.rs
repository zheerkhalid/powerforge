//! Notification-area pieces used by the engine thread: the icon with the
//! charge drawn on it, the tooltip text, balloons and the hotkey parser.

use crate::battery::{BatteryInfo, ChargeState};
use std::ffi::c_void;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HANDLE, HINSTANCE, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject,
    DrawTextW, SelectObject, SetBkMode, SetTextColor, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS,
    DT_CENTER, DT_NOCLIP, DT_SINGLELINE, DT_VCENTER, HDC, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, GetSystemMetrics, LoadIconW, HICON, ICONINFO, IDI_APPLICATION, SM_CXSMICON,
};

/// The colour family of the tray icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconStyle {
    /// On battery, normal charge: deep teal.
    Normal,
    /// Charging or plugged in: teal to sun yellow.
    Charging,
    /// Low battery: red.
    Low,
}

pub fn style_for(info: &BatteryInfo, low_pct: u8) -> IconStyle {
    if info.ac_online
        || matches!(
            info.state,
            ChargeState::Charging | ChargeState::Full | ChargeState::Idle
        )
    {
        IconStyle::Charging
    } else if info.percent <= low_pct as f32 {
        IconStyle::Low
    } else {
        IconStyle::Normal
    }
}

/// BGRA, premultiplied, top-down pixels of the icon background: a rounded
/// square in the style's colours, with an optional yellow dot in the top
/// right corner (keep-awake on).
pub fn icon_background(size: usize, style: IconStyle, dot: bool) -> Vec<u8> {
    let mut px = vec![0u8; size * size * 4];
    let radius = size as f32 * 0.22;
    let max = size as f32 - 1.0;
    let (a, b): ([f32; 3], [f32; 3]) = match style {
        IconStyle::Normal => (
            [0x0a as f32, 0x4e as f32, 0x5e as f32],
            [0x14 as f32, 0x7a as f32, 0x78 as f32],
        ),
        IconStyle::Charging => (
            [0x0a as f32, 0x5c as f32, 0x6e as f32],
            [0xb8 as f32, 0x93 as f32, 0x1c as f32],
        ),
        IconStyle::Low => (
            [0xa8 as f32, 0x1e as f32, 0x1e as f32],
            [0xd8 as f32, 0x45 as f32, 0x2a as f32],
        ),
    };
    let dot_r = size as f32 * 0.2;
    let (dx, dy) = (max - dot_r + 0.5, dot_r - 0.5);
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32, y as f32);
            let cx = fx.clamp(radius, max - radius);
            let cy = fy.clamp(radius, max - radius);
            let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
            let alpha = (radius + 1.0 - d).clamp(0.0, 1.0);
            let t = (fx + fy) / (2.0 * max.max(1.0));
            let mut rgb = [0f32; 3];
            for c in 0..3 {
                rgb[c] = a[c] + (b[c] - a[c]) * t;
            }
            let mut alpha = alpha;
            if dot {
                let dd = ((fx - dx).powi(2) + (fy - dy).powi(2)).sqrt();
                let cover = (dot_r + 0.5 - dd).clamp(0.0, 1.0);
                if cover > 0.0 {
                    let yellow = [0xff as f32, 0xd2 as f32, 0x3f as f32];
                    for c in 0..3 {
                        rgb[c] = rgb[c] * (1.0 - cover) + yellow[c] * cover;
                    }
                    alpha = alpha.max(cover);
                }
            }
            let i = (y * size + x) * 4;
            px[i] = (rgb[2] * alpha) as u8;
            px[i + 1] = (rgb[1] * alpha) as u8;
            px[i + 2] = (rgb[0] * alpha) as u8;
            px[i + 3] = (alpha * 255.0) as u8;
        }
    }
    px
}

/// Renders an icon with `text` (up to three characters) on it. Returns 0
/// on failure. The caller destroys the icon with DestroyIcon.
pub fn render_icon(text: &str, style: IconStyle, dot: bool) -> isize {
    // SAFETY: GDI objects created here are selected out and deleted before
    // returning; the DIB pixels are accessed within the size they were
    // created with.
    unsafe {
        let size = GetSystemMetrics(SM_CXSMICON).clamp(16, 64);
        let n = size as usize;
        let dc = CreateCompatibleDC(HDC::default());
        if dc.is_invalid() {
            return 0;
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let Ok(color) =
            CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0)
        else {
            let _ = DeleteDC(dc);
            return 0;
        };
        if bits.is_null() {
            let _ = DeleteObject(color);
            let _ = DeleteDC(dc);
            return 0;
        }
        let background = icon_background(n, style, dot);
        let pixels = std::slice::from_raw_parts_mut(bits as *mut u8, n * n * 4);
        pixels.copy_from_slice(&background);
        let old_bitmap = SelectObject(dc, color);
        let chars = text.chars().count().max(1) as i32;
        let height = if chars >= 3 {
            size * 60 / 100
        } else {
            size * 80 / 100
        };
        let font = CreateFontW(
            -height,
            0,
            0,
            0,
            700,
            0,
            0,
            0,
            0,
            0,
            0,
            4,
            0,
            w!("Segoe UI"),
        );
        let old_font = SelectObject(dc, font);
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, COLORREF(0x00ff_ffff));
        let mut buf: Vec<u16> = text.encode_utf16().collect();
        let mut rect = RECT {
            left: 0,
            top: -(size / 16),
            right: size,
            bottom: size - size / 16,
        };
        DrawTextW(
            dc,
            &mut buf,
            &mut rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOCLIP,
        );
        // GDI does not write alpha: restore it from the background, and make
        // the white text opaque where it was drawn.
        for i in 0..n * n {
            let a = background[i * 4 + 3];
            pixels[i * 4 + 3] = a;
            for c in 0..3 {
                pixels[i * 4 + c] = pixels[i * 4 + c].min(a);
            }
        }
        SelectObject(dc, old_font);
        SelectObject(dc, old_bitmap);
        let _ = DeleteObject(font);
        let _ = DeleteDC(dc);
        let mask = CreateBitmap(size, size, 1, 1, None);
        let icon_info = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&icon_info)
            .map(|i| i.0 as isize)
            .unwrap_or(0);
        let _ = DeleteObject(mask);
        let _ = DeleteObject(color);
        icon
    }
}

/// The program icon embedded by the build script (resource 1).
pub fn app_icon() -> HICON {
    // SAFETY: loading a shared icon resource; shared icons are not destroyed.
    unsafe {
        let module = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
        LoadIconW(HINSTANCE(module.0), PCWSTR(std::ptr::without_provenance(1)))
            .or_else(|_| LoadIconW(HINSTANCE::default(), IDI_APPLICATION))
            .unwrap_or_default()
    }
}

/// Copies text into a fixed UTF-16 buffer, cut to fit, NUL-terminated.
pub fn copy_wide(target: &mut [u16], text: &str) {
    let max = target.len() - 1;
    let mut n = 0;
    for unit in text.encode_utf16().take(max) {
        target[n] = unit;
        n += 1;
    }
    target[n] = 0;
}

/// The text drawn on the icon: "64", "100" or "" for no battery.
pub fn icon_text(info: Option<&BatteryInfo>) -> String {
    info.map(|b| format!("{:.0}", b.percent.clamp(0.0, 100.0)))
        .unwrap_or_default()
}

/// The tooltip: "PowerForge: 64 %, on battery, 3 h 10 min left. Keeping
/// awake." Cut to the 127 characters Windows allows.
pub fn tip_text(
    info: Option<&BatteryInfo>,
    on_ac: Option<bool>,
    awake: bool,
    plan: &str,
) -> String {
    let mut s = String::from("PowerForge");
    match info {
        Some(b) => {
            s.push_str(&format!(
                ": {:.0} %, {}",
                b.percent,
                b.state.label().to_lowercase()
            ));
            if let Some(secs) = b.seconds_left {
                let what = if b.state == ChargeState::Charging {
                    "to full"
                } else {
                    "left"
                };
                s.push_str(&format!(
                    ", {} {what}",
                    crate::util::fmt_duration(secs as u64)
                ));
            }
        }
        None => {
            if on_ac == Some(true) {
                s.push_str(": mains power");
            }
        }
    }
    if !plan.is_empty() {
        s.push_str(&format!("\nPlan: {plan}"));
    }
    if awake {
        s.push_str("\nKeeping the PC awake");
    }
    s.chars().take(127).collect()
}

/// A parsed global hotkey.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Hotkey {
    pub modifiers: u32,
    pub vk: u32,
}

/// Parses `Ctrl+Alt+K`, `Win+Shift+F9`. Needs Ctrl, Alt or Win and exactly
/// one key: a letter, a digit or F1 to F24.
pub fn parse_hotkey(text: &str) -> Option<Hotkey> {
    let mut modifiers = 0u32;
    let mut vk = None;
    for part in text.split('+') {
        let part = part.trim().to_lowercase();
        match part.as_str() {
            "ctrl" | "control" => modifiers |= MOD_CONTROL.0,
            "alt" => modifiers |= MOD_ALT.0,
            "shift" => modifiers |= MOD_SHIFT.0,
            "win" | "windows" => modifiers |= MOD_WIN.0,
            key => {
                if vk.is_some() {
                    return None;
                }
                let mut chars = key.chars();
                let first = chars.next()?;
                let rest: String = chars.collect();
                vk = Some(if rest.is_empty() && first.is_ascii_alphanumeric() {
                    first.to_ascii_uppercase() as u32
                } else if first == 'f' {
                    let n: u32 = rest.parse().ok()?;
                    if !(1..=24).contains(&n) {
                        return None;
                    }
                    0x70 + n - 1
                } else {
                    return None;
                });
            }
        }
    }
    let vk = vk?;
    if modifiers & (MOD_CONTROL.0 | MOD_ALT.0 | MOD_WIN.0) == 0 {
        return None;
    }
    Some(Hotkey { modifiers, vk })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkeys() {
        let h = parse_hotkey("Ctrl+Alt+K").unwrap();
        assert_eq!(h.modifiers, MOD_CONTROL.0 | MOD_ALT.0);
        assert_eq!(h.vk, 'K' as u32);
        assert_eq!(parse_hotkey("win + shift + f9").unwrap().vk, 0x78);
        assert!(parse_hotkey("Shift+K").is_none());
        assert!(parse_hotkey("K").is_none());
        assert!(parse_hotkey("Ctrl+K+L").is_none());
        assert!(parse_hotkey("Ctrl+F25").is_none());
        assert!(parse_hotkey("").is_none());
    }

    #[test]
    fn icon_pixels_and_texts() {
        let px = icon_background(16, IconStyle::Normal, false);
        assert_eq!(px.len(), 16 * 16 * 4);
        // Corners are transparent, the centre opaque.
        assert_eq!(px[3], 0);
        assert_eq!(px[(8 * 16 + 8) * 4 + 3], 255);
        // The dot paints the top-right corner.
        let dot = icon_background(16, IconStyle::Normal, true);
        let i = (2 * 16 + 13) * 4;
        assert!(dot[i + 2] > 200 && dot[i + 1] > 150);
        let b = crate::battery::demo();
        assert_eq!(icon_text(Some(&b)), "64");
        assert_eq!(icon_text(None), "");
        assert_eq!(style_for(&b, 15), IconStyle::Normal);
        let low = BatteryInfo {
            percent: 9.0,
            ..b.clone()
        };
        assert_eq!(style_for(&low, 15), IconStyle::Low);
        let tip = tip_text(Some(&b), Some(false), true, "Balanced");
        assert!(tip.starts_with("PowerForge: 64 %, on battery, 3 h"));
        assert!(tip.contains("Plan: Balanced") && tip.contains("awake"));
        assert!(tip_text(None, Some(true), false, "").contains("mains"));
        let mut buf = [0u16; 4];
        copy_wide(&mut buf, "abcdef");
        assert_eq!(buf, [97, 98, 99, 0]);
    }

    #[test]
    fn renders_an_icon() {
        let icon = render_icon("64", IconStyle::Charging, true);
        assert!(icon != 0);
        // SAFETY: destroying the icon created above.
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyIcon(HICON(icon as _));
        }
    }
}
