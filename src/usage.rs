//! Persistent per-interface, per-day traffic totals.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use chrono::{Datelike, Days, Months, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::platform::InterfaceKind;
use crate::storage;

/// Received / transmitted byte counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Traffic {
    pub rx: u64,
    pub tx: u64,
}

impl Traffic {
    pub const ZERO: Traffic = Traffic { rx: 0, tx: 0 };

    pub fn new(rx: u64, tx: u64) -> Self {
        Traffic { rx, tx }
    }

    pub fn total(self) -> u64 {
        self.rx.saturating_add(self.tx)
    }

    pub fn is_zero(self) -> bool {
        self.rx == 0 && self.tx == 0
    }
}

impl std::ops::AddAssign for Traffic {
    fn add_assign(&mut self, other: Traffic) {
        self.rx = self.rx.saturating_add(other.rx);
        self.tx = self.tx.saturating_add(other.tx);
    }
}

/// Last counter values seen for an interface, used to account for traffic
/// that happened while the application was not running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CounterMark {
    pub rx: u64,
    pub tx: u64,
    /// System uptime when the counters were read. A smaller uptime later
    /// means the machine was rebooted and the counters restarted from zero.
    pub uptime_secs: u64,
}

impl CounterMark {
    /// Traffic between this mark and a later reading of the same counters.
    ///
    /// Never over-estimates: if the counters were reset in between, only the
    /// part since the reset is counted.
    pub fn delta_to(&self, rx: u64, tx: u64, uptime_secs: Option<u64>) -> Traffic {
        let rebooted = uptime_secs.is_some_and(|now| now < self.uptime_secs);
        if rebooted {
            Traffic::new(rx, tx)
        } else {
            Traffic::new(counter_delta(self.rx, rx), counter_delta(self.tx, tx))
        }
    }
}

/// Difference between two readings of a cumulative counter. A decreasing
/// value means the counter was reset (adapter re-created or restarted).
pub fn counter_delta(previous: u64, current: u64) -> u64 {
    if current >= previous { current - previous } else { current }
}

/// Everything stored about one interface.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InterfaceRecord {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub kind: Option<InterfaceKind>,
    /// Hidden unless "show hidden interfaces" is on (filter, loopback, ...).
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub daily: BTreeMap<NaiveDate, Traffic>,
    #[serde(default)]
    pub counters: Option<CounterMark>,
}

/// Usage history of all interfaces.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageStore {
    #[serde(default)]
    pub interfaces: BTreeMap<String, InterfaceRecord>,
}

impl UsageStore {
    pub fn load(path: &Path) -> io::Result<Self> {
        Ok(storage::read_json(path)?.unwrap_or_default())
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        storage::write_json_atomic(path, self)
    }

    pub fn record(&self, key: &str) -> Option<&InterfaceRecord> {
        self.interfaces.get(key)
    }

    pub fn record_mut(&mut self, key: &str) -> &mut InterfaceRecord {
        self.interfaces.entry(key.to_string()).or_default()
    }

    /// Adds traffic to the given day.
    pub fn add(&mut self, key: &str, date: NaiveDate, traffic: Traffic) {
        if traffic.is_zero() {
            return;
        }
        *self.record_mut(key).daily.entry(date).or_default() += traffic;
    }

    /// Total traffic in the inclusive date range.
    pub fn sum(&self, key: &str, from: NaiveDate, to: NaiveDate) -> Traffic {
        let mut total = Traffic::ZERO;
        if let Some(record) = self.interfaces.get(key) {
            for (_, traffic) in record.daily.range(from..=to) {
                total += *traffic;
            }
        }
        total
    }

    pub fn day(&self, key: &str, date: NaiveDate) -> Traffic {
        self.sum(key, date, date)
    }

    /// Total traffic ever recorded for the interface.
    pub fn all_time(&self, key: &str) -> Traffic {
        let mut total = Traffic::ZERO;
        if let Some(record) = self.interfaces.get(key) {
            for traffic in record.daily.values() {
                total += *traffic;
            }
        }
        total
    }

    pub fn has_usage(&self, key: &str) -> bool {
        self.interfaces.get(key).is_some_and(|r| !r.daily.is_empty())
    }

    /// Forgets everything about an interface.
    pub fn remove(&mut self, key: &str) -> bool {
        self.interfaces.remove(key).is_some()
    }
}

/// A billing period (inclusive on both ends).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Period {
    pub start: NaiveDate,
    pub end: NaiveDate,
}

impl Period {
    /// The billing period containing `date`, where every period begins on
    /// `start_day` of a month (clamped to 1..=28 so it exists every month).
    pub fn containing(date: NaiveDate, start_day: u32) -> Period {
        let start_day = start_day.clamp(1, 28);
        let this_month = date.with_day(start_day).expect("day 1..=28 always exists");
        let start = if date.day() >= start_day { this_month } else { this_month - Months::new(1) };
        Period::starting_at(start)
    }

    fn starting_at(start: NaiveDate) -> Period {
        let end = start + Months::new(1) - Days::new(1);
        Period { start, end }
    }

    /// The period `n` periods before this one.
    pub fn previous(self, n: u32) -> Period {
        Period::starting_at(self.start - Months::new(n))
    }

    pub fn contains(self, date: NaiveDate) -> bool {
        self.start <= date && date <= self.end
    }

    /// Number of days in the period.
    pub fn len_days(self) -> u32 {
        (self.end - self.start).num_days() as u32 + 1
    }

    /// e.g. "2026年10月" for calendar months, "9/25 〜 10/24" otherwise.
    pub fn label(self) -> String {
        if self.start.day() == 1 {
            format!("{}年{}月", self.start.year(), self.start.month())
        } else {
            format!("{}/{} 〜 {}/{}", self.start.month(), self.start.day(), self.end.month(), self.end.day())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tests::TempDir;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn counter_reset_is_handled() {
        assert_eq!(counter_delta(100, 150), 50);
        assert_eq!(counter_delta(100, 30), 30);
        assert_eq!(counter_delta(0, 0), 0);
    }

    #[test]
    fn offline_delta_never_overcounts() {
        let mark = CounterMark { rx: 1000, tx: 500, uptime_secs: 3600 };
        // Same boot, counters kept growing.
        assert_eq!(mark.delta_to(1500, 800, Some(7200)), Traffic::new(500, 300));
        // Same boot, adapter re-created (WireGuard reconnect) -> counted from zero.
        assert_eq!(mark.delta_to(200, 100, Some(7200)), Traffic::new(200, 100));
        // Rebooted: everything since boot happened while we were not running.
        assert_eq!(mark.delta_to(5000, 4000, Some(60)), Traffic::new(5000, 4000));
        // Unknown uptime falls back to the conservative computation.
        assert_eq!(mark.delta_to(1200, 400, None), Traffic::new(200, 400));
    }

    #[test]
    fn sums_and_persistence() {
        let mut store = UsageStore::default();
        store.add("wg0", d(2026, 9, 30), Traffic::new(10, 1));
        store.add("wg0", d(2026, 10, 1), Traffic::new(20, 2));
        store.add("wg0", d(2026, 10, 1), Traffic::new(5, 5));
        store.add("eth0", d(2026, 10, 1), Traffic::new(1000, 1000));
        store.add("idle", d(2026, 10, 1), Traffic::ZERO);

        assert_eq!(store.day("wg0", d(2026, 10, 1)), Traffic::new(25, 7));
        assert_eq!(store.sum("wg0", d(2026, 9, 1), d(2026, 10, 31)), Traffic::new(35, 8));
        assert_eq!(store.all_time("wg0").total(), 43);
        assert_eq!(store.day("missing", d(2026, 10, 1)), Traffic::ZERO);
        assert!(!store.has_usage("idle"));

        store.record_mut("wg0").counters = Some(CounterMark { rx: 1, tx: 2, uptime_secs: 3 });

        let dir = TempDir::new("usage");
        let path = dir.0.join("usage.json");
        store.save(&path).unwrap();
        let loaded = UsageStore::load(&path).unwrap();
        assert_eq!(loaded.day("wg0", d(2026, 10, 1)), Traffic::new(25, 7));
        assert_eq!(loaded.record("wg0").unwrap().counters, store.record("wg0").unwrap().counters);

        let mut loaded = loaded;
        assert!(loaded.remove("wg0"));
        assert!(!loaded.has_usage("wg0"));
    }

    #[test]
    fn billing_periods() {
        let p = Period::containing(d(2026, 10, 1), 1);
        assert_eq!((p.start, p.end), (d(2026, 10, 1), d(2026, 10, 31)));
        assert_eq!(p.label(), "2026年10月");
        assert_eq!(p.len_days(), 31);

        let p = Period::containing(d(2026, 10, 1), 25);
        assert_eq!((p.start, p.end), (d(2026, 9, 25), d(2026, 10, 24)));
        assert_eq!(p.label(), "9/25 〜 10/24");

        let p = Period::containing(d(2026, 3, 15), 10);
        assert_eq!((p.start, p.end), (d(2026, 3, 10), d(2026, 4, 9)));

        let p = Period::containing(d(2027, 1, 5), 31); // clamped to 28
        assert_eq!((p.start, p.end), (d(2026, 12, 28), d(2027, 1, 27)));
        assert_eq!(p.previous(1).start, d(2026, 11, 28));

        let feb = Period::containing(d(2028, 2, 10), 1);
        assert_eq!(feb.end, d(2028, 2, 29));
        assert!(feb.contains(d(2028, 2, 29)));
        assert!(!feb.contains(d(2028, 3, 1)));
        assert_eq!(feb.previous(2).start, d(2027, 12, 1));
    }
}
