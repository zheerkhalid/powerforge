//! Locally recorded battery history: one capacity record per day and a
//! charge/rate sample log, kept in the data folder.

use crate::battery::BatteryInfo;
use crate::batteryreport::BatteryReport;
use crate::health::{health_percent, HealthPoint};
use crate::util;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// One capacity record (at most one per local day).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CapacityRecord {
    pub t: i64,
    pub full_mwh: u32,
    pub design_mwh: u32,
    pub cycles: Option<u32>,
}

/// A charge sample (kept for 30 days, at most one per 5 minutes).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ChargeSample {
    pub t: i64,
    pub percent: f32,
    /// Milliwatts, + charging / - discharging.
    pub rate_mw: Option<i32>,
    pub ac: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    pub capacity: Vec<CapacityRecord>,
    pub charge: Vec<ChargeSample>,
}

const SAMPLE_EVERY: i64 = 300;
const KEEP_CHARGE: i64 = 30 * 86_400;
const MAX_CAPACITY: usize = 5 * 366;

fn local_day(t: i64) -> i64 {
    (t + util::local_offset(t)).div_euclid(86_400)
}

impl History {
    /// Loads `history.json` from the data folder; empty if missing or damaged.
    pub fn load(dir: &Path) -> History {
        std::fs::read(dir.join("history.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Writes `history.json` atomically (temp file + rename).
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join("history.json.tmp");
        let bytes = serde_json::to_vec(self).map_err(std::io::Error::other)?;
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, dir.join("history.json"))
    }

    /// Records a reading. Returns true when something new was stored.
    pub fn record(&mut self, info: &BatteryInfo, now: i64) -> bool {
        let mut changed = false;
        if let (Some(full), Some(design)) = (info.full_mwh, info.design_mwh) {
            let new_day = self
                .capacity
                .last()
                .is_none_or(|last| local_day(last.t) != local_day(now));
            let moved = self.capacity.last().is_some_and(|last| {
                (last.full_mwh as f64 - full as f64).abs() > last.full_mwh as f64 * 0.01
            });
            if new_day || moved {
                if !new_day {
                    self.capacity.pop();
                }
                self.capacity.push(CapacityRecord {
                    t: now,
                    full_mwh: full,
                    design_mwh: design,
                    cycles: info.cycle_count,
                });
                if self.capacity.len() > MAX_CAPACITY {
                    let cut = self.capacity.len() - MAX_CAPACITY;
                    self.capacity.drain(..cut);
                }
                changed = true;
            }
        }
        let due = self.charge.last().is_none_or(|last| {
            now - last.t >= SAMPLE_EVERY || last.ac != info.ac_online || now < last.t
        });
        if due {
            self.charge.push(ChargeSample {
                t: now,
                percent: info.percent,
                rate_mw: info.rate_mw,
                ac: info.ac_online,
            });
            self.charge.retain(|s| now - s.t <= KEEP_CHARGE);
            changed = true;
        }
        changed
    }

    /// Health points from local records merged with the report's history,
    /// sorted by time, one per day at most (local records win).
    pub fn health_points(&self, report: Option<&BatteryReport>) -> Vec<HealthPoint> {
        let mut points: Vec<(i64, HealthPoint)> = Vec::new();
        for r in &self.capacity {
            if let Some(h) = health_percent(Some(r.design_mwh), Some(r.full_mwh)) {
                points.push((
                    local_day(r.t),
                    HealthPoint {
                        t: r.t,
                        health_pct: h,
                        cycles: r.cycles,
                    },
                ));
            }
        }
        if let Some(rep) = report {
            for p in &rep.history {
                let day = local_day(p.end);
                if points.iter().any(|(d, _)| *d == day) {
                    continue;
                }
                if let Some(h) = health_percent(p.design_mwh, p.full_mwh) {
                    points.push((
                        day,
                        HealthPoint {
                            t: p.end,
                            health_pct: h,
                            cycles: p.cycle_count,
                        },
                    ));
                }
            }
        }
        points.sort_by_key(|(_, p)| p.t);
        points.dedup_by_key(|(d, _)| *d);
        points.into_iter().map(|(_, p)| p).collect()
    }

    /// Average discharge in milliwatts over on-battery samples of the last
    /// `days` days (usage estimate).
    pub fn average_drain_mw(&self, now: i64, days: u32) -> Option<u32> {
        let from = now - days as i64 * 86_400;
        let rates: Vec<f64> = self
            .charge
            .iter()
            .filter(|s| s.t >= from && !s.ac)
            .filter_map(|s| s.rate_mw)
            .filter(|r| *r < -100)
            .map(|r| -r as f64)
            .collect();
        if rates.len() < 3 {
            return None;
        }
        Some((rates.iter().sum::<f64>() / rates.len() as f64) as u32)
    }

    /// Charge samples within the last `secs` seconds, oldest first.
    pub fn recent_charge(&self, now: i64, secs: i64) -> Vec<&ChargeSample> {
        self.charge.iter().filter(|s| now - s.t <= secs).collect()
    }
}

/// Invented history for `--demo`, consistent with `battery::demo()`.
pub fn demo(now: i64) -> History {
    let mut h = History::default();
    for d in (0..90).rev() {
        let t = now - d * 86_400;
        let f = (90 - d) as f32 / 90.0;
        let full = 49_450.0 - 730.0 * f + ((d as f32) * 0.21).sin() * 30.0;
        h.capacity.push(CapacityRecord {
            t,
            full_mwh: full as u32,
            design_mwh: 56_000,
            cycles: Some(290 + (22.0 * f) as u32),
        });
    }
    if let Some(last) = h.capacity.last_mut() {
        last.full_mwh = 48_720;
        last.cycles = Some(312);
    }
    // A week of samples every 5 minutes: work on battery in the day,
    // charging in the evening, asleep at night.
    let mut pct = 80.0f32;
    let mut t = now - 7 * 86_400;
    while t <= now {
        let (_, _, _, hour, _, _) = util::local_parts(t);
        let (ac, rate) = match hour {
            9..=16 => (false, -(8_000 + ((t / 300) % 7) as i32 * 450)),
            17..=19 => (true, 24_000),
            _ => (true, 0),
        };
        let dpct = rate as f32 / 48_720.0 * 100.0 / 12.0;
        pct = (pct + dpct).clamp(8.0, 100.0);
        let rate = if ac && pct >= 99.9 { 0 } else { rate };
        h.charge.push(ChargeSample {
            t,
            percent: pct,
            rate_mw: Some(rate),
            ac,
        });
        t += 300;
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batteryreport::HistoryPeriod;

    fn info(full: u32, pct: f32, ac: bool, rate: i32) -> BatteryInfo {
        BatteryInfo {
            percent: pct,
            ac_online: ac,
            rate_mw: Some(rate),
            full_mwh: Some(full),
            design_mwh: Some(50_000),
            cycle_count: Some(100),
            ..Default::default()
        }
    }

    #[test]
    fn records_daily_and_samples() {
        util::set_fixed_offset(Some(0));
        let mut h = History::default();
        let day0 = 20_000 * 86_400;
        assert!(h.record(&info(45_000, 50.0, false, -9_000), day0 + 3600));
        assert_eq!(h.capacity.len(), 1);
        assert_eq!(h.charge.len(), 1);
        // Same day, same capacity, 1 minute later: nothing.
        assert!(!h.record(&info(45_000, 49.0, false, -9_000), day0 + 3660));
        // Plugging in records a sample at once.
        assert!(h.record(&info(45_000, 49.0, true, 20_000), day0 + 3700));
        assert_eq!(h.charge.len(), 2);
        // Capacity moved by more than 1 % on the same day: replaced.
        h.record(&info(44_000, 60.0, true, 20_000), day0 + 4000);
        assert_eq!(h.capacity.len(), 1);
        assert_eq!(h.capacity[0].full_mwh, 44_000);
        // Next day: a new record.
        h.record(&info(44_000, 60.0, true, 20_000), day0 + 86_400 + 10);
        assert_eq!(h.capacity.len(), 2);
        // Samples older than 30 days are dropped.
        h.record(&info(44_000, 60.0, true, 0), day0 + 40 * 86_400);
        assert!(h.charge.iter().all(|s| s.t >= day0 + 10 * 86_400));
        util::set_fixed_offset(None);
    }

    #[test]
    fn save_and_load() {
        let dir = std::env::temp_dir().join(format!("powerforge-history-{}", std::process::id()));
        let h = demo(1_790_000_000);
        h.save(&dir).unwrap();
        assert_eq!(History::load(&dir), h);
        std::fs::write(dir.join("history.json"), b"{broken").unwrap();
        assert_eq!(History::load(&dir), History::default());
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(History::load(&dir), History::default());
    }

    #[test]
    fn merges_with_report() {
        util::set_fixed_offset(Some(0));
        let mut h = History::default();
        h.capacity.push(CapacityRecord {
            t: 100 * 86_400 + 50,
            full_mwh: 45_000,
            design_mwh: 50_000,
            cycles: Some(100),
        });
        let report = BatteryReport {
            history: vec![
                HistoryPeriod {
                    start: 0,
                    end: 50 * 86_400,
                    design_mwh: Some(50_000),
                    full_mwh: Some(48_000),
                    cycle_count: Some(50),
                    ..Default::default()
                },
                // Same day as the local record: the local one wins.
                HistoryPeriod {
                    start: 50 * 86_400,
                    end: 100 * 86_400 + 9_000,
                    design_mwh: Some(50_000),
                    full_mwh: Some(40_000),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let p = h.health_points(Some(&report));
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].health_pct, 96.0);
        assert_eq!(p[1].health_pct, 90.0);
        assert_eq!(h.health_points(None).len(), 1);
        util::set_fixed_offset(None);
    }

    #[test]
    fn drain_average() {
        let mut h = History::default();
        for (i, r) in [-8_000, -10_000, -12_000, 20_000].iter().enumerate() {
            h.charge.push(ChargeSample {
                t: 1000 + i as i64,
                percent: 50.0,
                rate_mw: Some(*r),
                ac: *r > 0,
            });
        }
        assert_eq!(h.average_drain_mw(2000, 1), Some(10_000));
        assert_eq!(h.average_drain_mw(1000 + 10 * 86_400, 1), None);
        let d = demo(1_790_000_000);
        assert!(d.average_drain_mw(1_790_000_000, 7).is_some());
        assert_eq!(d.capacity.len(), 90);
    }
}
