//! Time, text and number helpers shared by every module. Times are Unix
//! seconds (UTC) unless a name says otherwise.

use std::time::{SystemTime, UNIX_EPOCH};

/// The current time in Unix seconds.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Days since 1970-01-01 of a civil date (proleptic Gregorian).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date (year, month, day) of a day number.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Parses an ISO 8601 date-time such as `2026-11-16T22:10:11Z`,
/// `2026-11-16T22:10:11.6028922Z`, `2026-11-16T22:10:11+03:00` or a bare
/// `2026-11-16T22:10:11` / `2026-11-16` (taken as UTC). Returns Unix seconds.
pub fn parse_iso_datetime(text: &str) -> Option<i64> {
    let t = text.trim();
    if t.len() < 10 {
        return None;
    }
    let y: i64 = t.get(0..4)?.parse().ok()?;
    let mo: u32 = t.get(5..7)?.parse().ok()?;
    let d: u32 = t.get(8..10)?.parse().ok()?;
    if t.as_bytes().get(4) != Some(&b'-') || !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    let mut secs = days_from_civil(y, mo, d) * 86_400;
    let rest = &t[10..];
    if rest.is_empty() {
        return Some(secs);
    }
    let rest = rest.strip_prefix(['T', ' '])?;
    let h: i64 = rest.get(0..2)?.parse().ok()?;
    let mi: i64 = rest.get(3..5)?.parse().ok()?;
    let s: i64 = rest.get(6..8).and_then(|v| v.parse().ok()).unwrap_or(0);
    secs += h * 3600 + mi * 60 + s;
    // Skip fractional seconds, then read the zone.
    let mut zone = rest.get(8..).unwrap_or("");
    if let Some(frac) = zone.strip_prefix('.') {
        let n = frac.chars().take_while(|c| c.is_ascii_digit()).count();
        zone = &frac[n..];
    }
    match zone.as_bytes().first() {
        None | Some(b'Z') | Some(b'z') => Some(secs),
        Some(&sign) if sign == b'+' || sign == b'-' => {
            let z = &zone[1..];
            let zh: i64 = z.get(0..2)?.parse().ok()?;
            let zm: i64 = z
                .get(2..)
                .map(|r| r.trim_start_matches(':'))
                .and_then(|r| r.get(0..2))
                .and_then(|r| r.parse().ok())
                .unwrap_or(0);
            let off = zh * 3600 + zm * 60;
            Some(if sign == b'+' { secs - off } else { secs + off })
        }
        _ => None,
    }
}

/// Parses an ISO 8601 duration such as `PT3H22M30S` or `P2DT1H2M51S`
/// into seconds. Years and months are not used by Windows reports and are
/// rejected.
pub fn parse_iso_duration(text: &str) -> Option<u64> {
    let t = text.trim().strip_prefix('P')?;
    let mut total = 0f64;
    let mut in_time = false;
    let mut num = String::new();
    let mut any = false;
    for c in t.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' | '.' => num.push(c),
            'D' | 'H' | 'M' | 'S' | 'W' => {
                let v: f64 = num.parse().ok()?;
                num.clear();
                any = true;
                total += v * match (c, in_time) {
                    ('W', false) => 604_800.0,
                    ('D', false) => 86_400.0,
                    ('H', true) => 3600.0,
                    ('M', true) => 60.0,
                    ('S', true) => 1.0,
                    _ => return None,
                };
            }
            _ => return None,
        }
    }
    (any && num.is_empty()).then_some(total.round() as u64)
}

/// "3 h 05 min", "42 min", "50 s", "2 d 4 h".
pub fn fmt_duration(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    if d > 0 {
        if h > 0 {
            format!("{d} d {h} h")
        } else {
            format!("{d} d")
        }
    } else if h > 0 {
        format!("{h} h {m:02} min")
    } else if m > 0 {
        format!("{m} min")
    } else {
        format!("{secs} s")
    }
}

/// Seconds east of UTC that local time has at a UTC instant.
pub fn local_offset(utc: i64) -> i64 {
    if let Some(fixed) = FIXED_OFFSET.with(|f| *f.borrow()) {
        return fixed;
    }
    crate::sys::utc_offset_at(utc) as i64
}

thread_local! {
    static FIXED_OFFSET: std::cell::RefCell<Option<i64>> = const { std::cell::RefCell::new(None) };
}

/// Pins the local offset on this thread (tests and screenshots), or
/// unpins it with `None`.
pub fn set_fixed_offset(offset: Option<i64>) {
    FIXED_OFFSET.with(|f| *f.borrow_mut() = offset);
}

/// Local civil time parts: (year, month, day, hour, minute, second).
pub fn local_parts(utc: i64) -> (i64, u32, u32, u32, u32, u32) {
    let local = utc + local_offset(utc);
    let (y, m, d) = civil_from_days(local.div_euclid(86_400));
    let s = local.rem_euclid(86_400);
    (
        y,
        m,
        d,
        (s / 3600) as u32,
        (s % 3600 / 60) as u32,
        (s % 60) as u32,
    )
}

/// "14:05" in local time.
pub fn fmt_time(utc: i64) -> String {
    let (_, _, _, h, mi, _) = local_parts(utc);
    format!("{h:02}:{mi:02}")
}

/// "2026-11-24" in local time.
pub fn fmt_date(utc: i64) -> String {
    let (y, m, d, ..) = local_parts(utc);
    format!("{y:04}-{m:02}-{d:02}")
}

/// "2026-11-24 14:05" in local time.
pub fn fmt_datetime(utc: i64) -> String {
    format!("{} {}", fmt_date(utc), fmt_time(utc))
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// "24 Nov 2026" in local time.
pub fn fmt_day(utc: i64) -> String {
    let (y, m, d, ..) = local_parts(utc);
    format!("{d} {} {y}", MONTHS[(m as usize).clamp(1, 12) - 1])
}

/// "Nov 2026" in local time.
pub fn fmt_month(utc: i64) -> String {
    let (y, m, ..) = local_parts(utc);
    format!("{} {y}", MONTHS[(m as usize).clamp(1, 12) - 1])
}

/// The UTC instant of the next local wall-clock time `hh:mm` after `now`.
pub fn next_local_time(now: i64, hour: u32, minute: u32) -> i64 {
    let local = now + local_offset(now);
    let day_start = local - local.rem_euclid(86_400);
    let mut target = day_start + hour as i64 * 3600 + minute as i64 * 60;
    if target <= local {
        target += 86_400;
    }
    target - local_offset(now)
}

/// Milliwatts as "12.4 W".
pub fn fmt_watts(mw: i64) -> String {
    format!("{:.1} W", mw as f64 / 1000.0)
}

/// Milliwatt-hours as "52.3 Wh".
pub fn fmt_wh(mwh: u64) -> String {
    format!("{:.1} Wh", mwh as f64 / 1000.0)
}

/// Bytes as "1.2 GB".
pub fn fmt_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i + 1 < UNITS.len() {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// Escapes text for HTML.
pub fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Quotes a CSV field when needed.
pub fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_datetimes() {
        assert_eq!(parse_iso_datetime("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_iso_datetime("2026-11-16T22:10:11Z"),
            Some(days_from_civil(2026, 11, 16) * 86_400 + 22 * 3600 + 10 * 60 + 11)
        );
        assert_eq!(
            parse_iso_datetime("2026-11-23T21:19:08.6028922Z"),
            parse_iso_datetime("2026-11-23T21:19:08Z")
        );
        assert_eq!(
            parse_iso_datetime("2026-11-17T01:10:11+03:00"),
            parse_iso_datetime("2026-11-16T22:10:11Z")
        );
        assert_eq!(
            parse_iso_datetime("2026-11-16"),
            Some(days_from_civil(2026, 11, 16) * 86_400)
        );
        assert_eq!(parse_iso_datetime("garbage"), None);
        assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
    }

    #[test]
    fn iso_durations() {
        assert_eq!(
            parse_iso_duration("PT3H22M30S"),
            Some(3 * 3600 + 22 * 60 + 30)
        );
        assert_eq!(
            parse_iso_duration("P2DT1H2M51S"),
            Some(2 * 86_400 + 3600 + 2 * 60 + 51)
        );
        assert_eq!(parse_iso_duration("PT0S"), Some(0));
        assert_eq!(parse_iso_duration("PT1.5S"), Some(2));
        assert_eq!(parse_iso_duration("P1Y"), None);
        assert_eq!(parse_iso_duration("3H"), None);
    }

    #[test]
    fn formatting() {
        assert_eq!(fmt_duration(50), "50 s");
        assert_eq!(fmt_duration(42 * 60), "42 min");
        assert_eq!(fmt_duration(3 * 3600 + 5 * 60), "3 h 05 min");
        assert_eq!(fmt_duration(2 * 86_400 + 4 * 3600), "2 d 4 h");
        assert_eq!(fmt_watts(12_400), "12.4 W");
        assert_eq!(fmt_wh(52_340), "52.3 Wh");
        assert_eq!(fmt_bytes(1536), "1.5 KB");
        assert_eq!(html_escape("<a&'\">"), "&lt;a&amp;&#39;&quot;&gt;");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        set_fixed_offset(Some(3 * 3600));
        assert_eq!(fmt_datetime(0), "1970-01-01 03:00");
        assert_eq!(fmt_day(0), "1 Jan 1970");
        assert_eq!(next_local_time(0, 4, 0), 3600);
        assert_eq!(next_local_time(0, 2, 0), 86_400 - 3600);
        set_fixed_offset(None);
    }
}
