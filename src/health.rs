//! Battery health maths: health percentage, grades, wear trend, runtime
//! estimates. Pure functions, no system calls.

/// Full-charge capacity as a percentage of design capacity (may exceed 100
/// for a new battery; capped at 110 for display).
pub fn health_percent(design_mwh: Option<u32>, full_mwh: Option<u32>) -> Option<f32> {
    match (design_mwh, full_mwh) {
        (Some(d), Some(f)) if d > 0 && f > 0 => Some((f as f32 / d as f32 * 100.0).min(110.0)),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grade {
    Excellent,
    Good,
    Fair,
    Worn,
    Replace,
}

impl Grade {
    pub fn label(self) -> &'static str {
        match self {
            Grade::Excellent => "Excellent",
            Grade::Good => "Good",
            Grade::Fair => "Fair",
            Grade::Worn => "Worn",
            Grade::Replace => "Consider replacing",
        }
    }
}

/// Grade of a health percentage.
pub fn grade(health_pct: f32) -> Grade {
    if health_pct >= 90.0 {
        Grade::Excellent
    } else if health_pct >= 80.0 {
        Grade::Good
    } else if health_pct >= 70.0 {
        Grade::Fair
    } else if health_pct >= 60.0 {
        Grade::Worn
    } else {
        Grade::Replace
    }
}

/// One plain sentence about a health percentage and cycle count.
pub fn explain(health_pct: f32, cycles: Option<u32>) -> String {
    let pct = health_pct.round() as i32;
    let held = match grade(health_pct) {
        Grade::Excellent => format!("The battery still holds {pct} % of the charge it held when new: it is in excellent shape."),
        Grade::Good => format!("The battery holds {pct} % of its original charge. That is normal wear; you will notice little difference."),
        Grade::Fair => format!("The battery holds {pct} % of its original charge, so a full charge lasts noticeably less than it did."),
        Grade::Worn => format!("The battery holds only {pct} % of its original charge. Expect clearly shorter battery life."),
        Grade::Replace => format!("The battery holds only {pct} % of its original charge. Replacing it would bring back most of the battery life."),
    };
    match cycles {
        Some(c) if c > 0 => format!(
            "{held} It has been through {c} charge cycles; most laptop batteries are designed for 300 to 1000."
        ),
        _ => held,
    }
}

/// One health measurement over time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HealthPoint {
    pub t: i64,
    pub health_pct: f32,
    pub cycles: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WearTrend {
    /// Health lost per year (positive = wearing).
    pub pct_per_year: f32,
    /// Health lost per 100 charge cycles, when cycle counts are known.
    pub pct_per_100_cycles: Option<f32>,
    /// Months until health reaches 80 % at this rate (None if already
    /// below, or not wearing).
    pub months_to_80: Option<f32>,
}

/// Slope and intercept of the least-squares line through (x, y).
fn least_squares(points: &[(f64, f64)]) -> Option<(f64, f64)> {
    let n = points.len() as f64;
    if points.len() < 2 {
        return None;
    }
    let mx = points.iter().map(|p| p.0).sum::<f64>() / n;
    let my = points.iter().map(|p| p.1).sum::<f64>() / n;
    let sxx: f64 = points.iter().map(|p| (p.0 - mx).powi(2)).sum();
    if sxx <= f64::EPSILON {
        return None;
    }
    let sxy: f64 = points.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
    let slope = sxy / sxx;
    Some((slope, my - slope * mx))
}

const YEAR: f64 = 365.25 * 86_400.0;

/// Least-squares wear trend; needs at least two points spanning 14+ days.
pub fn wear_trend(points: &[HealthPoint]) -> Option<WearTrend> {
    let first = points.iter().map(|p| p.t).min()?;
    let last = points.iter().map(|p| p.t).max()?;
    if last - first < 14 * 86_400 {
        return None;
    }
    let xy: Vec<(f64, f64)> = points
        .iter()
        .map(|p| ((p.t - first) as f64 / YEAR, p.health_pct as f64))
        .collect();
    let (slope, intercept) = least_squares(&xy)?;
    let pct_per_year = (-slope) as f32;
    let cyc: Vec<(f64, f64)> = points
        .iter()
        .filter_map(|p| p.cycles.map(|c| (c as f64 / 100.0, p.health_pct as f64)))
        .collect();
    let pct_per_100_cycles = least_squares(&cyc).map(|(s, _)| (-s) as f32);
    // Where the line is now, and when it crosses 80 %.
    let now_x = (last - first) as f64 / YEAR;
    let now_pct = intercept + slope * now_x;
    let months_to_80 = (slope < -0.01 && now_pct > 80.0)
        .then(|| ((now_pct - 80.0) / -slope * 12.0) as f32)
        .filter(|m| m.is_finite() && *m < 1200.0);
    Some(WearTrend {
        pct_per_year,
        pct_per_100_cycles,
        months_to_80,
    })
}

/// Seconds a full battery lasts at an average drain in milliwatts.
pub fn runtime_secs(full_mwh: u32, drain_mw: u32) -> Option<u64> {
    if full_mwh == 0 || drain_mw < 100 {
        return None;
    }
    Some((full_mwh as f64 / drain_mw as f64 * 3600.0).round() as u64)
}

/// A plain sentence about the wear trend.
pub fn explain_trend(trend: &WearTrend) -> String {
    if trend.pct_per_year <= 0.5 {
        return "Capacity has been steady: no measurable wear over this period.".into();
    }
    let mut text = format!(
        "The battery is losing about {:.1} % of its capacity per year",
        trend.pct_per_year
    );
    if let Some(c) = trend.pct_per_100_cycles {
        if c > 0.0 {
            text.push_str(&format!(" ({c:.1} % per 100 charge cycles)"));
        }
    }
    text.push('.');
    if let Some(m) = trend.months_to_80 {
        if m < 24.0 {
            text.push_str(&format!(
                " At this rate it reaches 80 % in about {} months.",
                m.round().max(1.0)
            ));
        } else {
            text.push_str(&format!(
                " At this rate it reaches 80 % in about {:.0} years.",
                m / 12.0
            ));
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_and_grades() {
        assert_eq!(health_percent(Some(50_000), Some(45_000)), Some(90.0));
        assert_eq!(health_percent(Some(50_000), Some(60_000)), Some(110.0));
        assert_eq!(health_percent(Some(0), Some(45_000)), None);
        assert_eq!(health_percent(None, Some(45_000)), None);
        assert_eq!(health_percent(Some(50_000), Some(0)), None);
        assert_eq!(grade(95.0), Grade::Excellent);
        assert_eq!(grade(90.0), Grade::Excellent);
        assert_eq!(grade(85.0), Grade::Good);
        assert_eq!(grade(72.0), Grade::Fair);
        assert_eq!(grade(61.0), Grade::Worn);
        assert_eq!(grade(40.0), Grade::Replace);
        assert!(explain(87.0, Some(312)).contains("87 %"));
        assert!(explain(87.0, Some(312)).contains("312 charge cycles"));
        assert!(!explain(87.0, None).contains("cycles"));
    }

    #[test]
    fn trend_over_two_years() {
        let day = 86_400;
        let points: Vec<HealthPoint> = (0..=730)
            .step_by(30)
            .map(|d| HealthPoint {
                t: 1_700_000_000 + d * day,
                health_pct: 100.0 - 5.0 * d as f32 / 365.25,
                cycles: Some((d / 2) as u32),
            })
            .collect();
        let t = wear_trend(&points).unwrap();
        assert!((t.pct_per_year - 5.0).abs() < 0.05, "{t:?}");
        // 0.5 cycles a day: 100 cycles in 200 days, 2.74 % per 100 cycles.
        assert!((t.pct_per_100_cycles.unwrap() - 2.738).abs() < 0.05);
        // Now at 90 %: 10 % to go at 5 % a year, two years.
        assert!((t.months_to_80.unwrap() - 24.0).abs() < 0.5);
        assert!(explain_trend(&t).contains("5.0 % of its capacity per year"));
    }

    #[test]
    fn trend_needs_span_and_handles_flat() {
        let p = |d: i64, h: f32| HealthPoint {
            t: d * 86_400,
            health_pct: h,
            cycles: None,
        };
        assert!(wear_trend(&[p(0, 90.0), p(5, 89.0)]).is_none());
        assert!(wear_trend(&[p(0, 90.0)]).is_none());
        assert!(wear_trend(&[]).is_none());
        let flat = wear_trend(&[p(0, 90.0), p(100, 90.0)]).unwrap();
        assert_eq!(flat.months_to_80, None);
        assert!(flat.pct_per_100_cycles.is_none());
        assert!(explain_trend(&flat).contains("steady"));
        // Already below 80 %: no countdown.
        let low = wear_trend(&[p(0, 79.0), p(100, 75.0)]).unwrap();
        assert_eq!(low.months_to_80, None);
    }

    #[test]
    fn runtime() {
        assert_eq!(runtime_secs(50_000, 10_000), Some(5 * 3600));
        assert_eq!(runtime_secs(50_000, 0), None);
        assert_eq!(runtime_secs(0, 10_000), None);
    }
}
