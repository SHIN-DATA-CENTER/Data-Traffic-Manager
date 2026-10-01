//! Turns successive counter samples into transfer rates and usage.

use std::collections::{BTreeMap, VecDeque};
use std::time::Instant;

use chrono::NaiveDate;

use crate::platform::InterfaceSample;
use crate::usage::{CounterMark, Traffic, UsageStore, counter_delta};

/// Transfer rate in bytes per second.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rate {
    pub rx: f64,
    pub tx: f64,
}

/// Live state of an interface during this session.
#[derive(Debug, Clone)]
pub struct LiveInterface {
    pub sample: InterfaceSample,
    /// Whether the interface was present in the latest sample.
    pub present: bool,
    pub rate: Rate,
    /// Highest rates seen this session.
    pub peak: Rate,
    /// Most recent rates, oldest first. Always `history_len` long.
    pub history: VecDeque<Rate>,
    /// Traffic since the application started (or the session was reset).
    pub session: Traffic,
}

/// Per-tick information needed to update the usage history.
pub struct Accounting<'a> {
    pub store: &'a mut UsageStore,
    /// Local date the traffic is booked on.
    pub today: NaiveDate,
    /// Current system uptime, see [`CounterMark`].
    pub uptime_secs: Option<u64>,
    /// Book traffic that happened while the application was not running.
    pub count_offline: bool,
}

pub struct Monitor {
    interfaces: BTreeMap<String, LiveInterface>,
    history_len: usize,
    last_tick: Option<Instant>,
}

impl Monitor {
    pub fn new(history_len: usize) -> Self {
        Monitor { interfaces: BTreeMap::new(), history_len: history_len.max(2), last_tick: None }
    }

    pub fn history_len(&self) -> usize {
        self.history_len
    }

    pub fn get(&self, key: &str) -> Option<&LiveInterface> {
        self.interfaces.get(key)
    }

    pub fn interfaces(&self) -> impl Iterator<Item = &LiveInterface> {
        self.interfaces.values()
    }

    /// Clears the graphs, e.g. after the sampling interval changed.
    pub fn clear_history(&mut self) {
        for live in self.interfaces.values_mut() {
            live.history.iter_mut().for_each(|r| *r = Rate::default());
        }
    }

    /// Resets the per-session totals and peaks.
    pub fn reset_session(&mut self) {
        for live in self.interfaces.values_mut() {
            live.session = Traffic::ZERO;
            live.peak = Rate::default();
        }
    }

    /// Forgets an interface that is no longer present.
    pub fn forget(&mut self, key: &str) {
        if self.interfaces.get(key).is_some_and(|live| !live.present) {
            self.interfaces.remove(key);
        }
    }

    /// Processes a new set of samples taken at `now`.
    pub fn update(&mut self, samples: Vec<InterfaceSample>, now: Instant, acct: &mut Accounting) {
        let elapsed =
            self.last_tick.map(|last| now.saturating_duration_since(last).as_secs_f64()).filter(|secs| *secs > 0.0);
        self.last_tick = Some(now);

        let mut seen = std::collections::HashSet::with_capacity(samples.len());
        for sample in samples {
            seen.insert(sample.key.clone());
            let record = acct.store.record_mut(&sample.key);
            record.description.clone_from(&sample.description);
            record.kind = Some(sample.kind);
            record.hidden = !sample.visible;
            let previous_mark = record.counters.replace(CounterMark {
                rx: sample.rx_bytes,
                tx: sample.tx_bytes,
                uptime_secs: acct.uptime_secs.unwrap_or(0),
            });

            match self.interfaces.get_mut(&sample.key) {
                Some(live) => {
                    let delta = Traffic::new(
                        counter_delta(live.sample.rx_bytes, sample.rx_bytes),
                        counter_delta(live.sample.tx_bytes, sample.tx_bytes),
                    );
                    // No rate right after an interface (re)appeared: the
                    // delta may span an unknown amount of time.
                    let rate = match elapsed {
                        Some(secs) if live.present => Rate { rx: delta.rx as f64 / secs, tx: delta.tx as f64 / secs },
                        _ => Rate::default(),
                    };
                    live.session += delta;
                    live.rate = rate;
                    live.peak.rx = live.peak.rx.max(rate.rx);
                    live.peak.tx = live.peak.tx.max(rate.tx);
                    live.present = true;
                    live.sample = sample;
                    push_history(&mut live.history, self.history_len, rate);
                    acct.store.add(&live.sample.key, acct.today, delta);
                }
                None => {
                    // First sighting in this session: book what happened
                    // since the counters were last saved (app not running).
                    if acct.count_offline
                        && let Some(mark) = previous_mark
                    {
                        let offline = mark.delta_to(sample.rx_bytes, sample.tx_bytes, acct.uptime_secs);
                        acct.store.add(&sample.key, acct.today, offline);
                    }
                    let mut history = VecDeque::with_capacity(self.history_len);
                    history.resize(self.history_len, Rate::default());
                    self.interfaces.insert(
                        sample.key.clone(),
                        LiveInterface {
                            sample,
                            present: true,
                            rate: Rate::default(),
                            peak: Rate::default(),
                            history,
                            session: Traffic::ZERO,
                        },
                    );
                }
            }
        }

        for live in self.interfaces.values_mut() {
            if !seen.contains(&live.sample.key) {
                live.present = false;
                live.rate = Rate::default();
                push_history(&mut live.history, self.history_len, Rate::default());
            }
        }
    }
}

fn push_history(history: &mut VecDeque<Rate>, len: usize, rate: Rate) {
    history.push_back(rate);
    while history.len() > len {
        history.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::platform::InterfaceKind;

    fn sample(key: &str, rx: u64, tx: u64) -> InterfaceSample {
        InterfaceSample {
            key: key.into(),
            name: key.into(),
            description: "WireGuard Tunnel".into(),
            kind: InterfaceKind::Vpn,
            is_up: true,
            visible: true,
            rx_bytes: rx,
            tx_bytes: tx,
            link_speed_bps: None,
        }
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()
    }

    fn tick(m: &mut Monitor, store: &mut UsageStore, now: Instant, samples: Vec<InterfaceSample>) {
        let mut acct = Accounting { store, today: today(), uptime_secs: Some(1000), count_offline: true };
        m.update(samples, now, &mut acct);
    }

    #[test]
    fn rates_session_and_usage() {
        let mut store = UsageStore::default();
        let mut m = Monitor::new(4);
        let t0 = Instant::now();

        tick(&mut m, &mut store, t0, vec![sample("wg0", 1000, 500)]);
        let live = m.get("wg0").unwrap();
        assert_eq!(live.rate, Rate::default());
        assert_eq!(live.history.len(), 4);
        assert_eq!(store.day("wg0", today()), Traffic::ZERO, "first ever sample is only a baseline");

        tick(&mut m, &mut store, t0 + Duration::from_secs(2), vec![sample("wg0", 3000, 900)]);
        let live = m.get("wg0").unwrap();
        assert_eq!(live.rate, Rate { rx: 1000.0, tx: 200.0 });
        assert_eq!(live.session, Traffic::new(2000, 400));
        assert_eq!(*live.history.back().unwrap(), Rate { rx: 1000.0, tx: 200.0 });
        assert_eq!(store.day("wg0", today()), Traffic::new(2000, 400));

        // Tunnel reconnected: the adapter is re-created and counters restart.
        tick(&mut m, &mut store, t0 + Duration::from_secs(3), vec![sample("wg0", 100, 50)]);
        assert_eq!(m.get("wg0").unwrap().session, Traffic::new(2100, 450));
        assert_eq!(store.day("wg0", today()), Traffic::new(2100, 450));
        assert_eq!(m.get("wg0").unwrap().peak, Rate { rx: 1000.0, tx: 200.0 });

        m.reset_session();
        assert_eq!(m.get("wg0").unwrap().session, Traffic::ZERO);
    }

    #[test]
    fn disappearing_interface() {
        let mut store = UsageStore::default();
        let mut m = Monitor::new(3);
        let t0 = Instant::now();
        tick(&mut m, &mut store, t0, vec![sample("wg0", 10, 10), sample("eth0", 0, 0)]);
        tick(&mut m, &mut store, t0 + Duration::from_secs(1), vec![sample("eth0", 5, 5)]);
        let wg = m.get("wg0").unwrap();
        assert!(!wg.present);
        assert_eq!(wg.rate, Rate::default());

        // Comes back with fresh counters: counted, but no rate spike.
        tick(&mut m, &mut store, t0 + Duration::from_secs(2), vec![sample("wg0", 7, 3)]);
        let wg = m.get("wg0").unwrap();
        assert!(wg.present);
        assert_eq!(wg.rate, Rate::default());
        assert_eq!(wg.session, Traffic::new(7, 3));

        tick(&mut m, &mut store, t0 + Duration::from_secs(3), vec![sample("wg0", 17, 3)]);
        assert_eq!(m.get("wg0").unwrap().rate.rx, 10.0);
        assert_eq!(m.get("wg0").unwrap().history.len(), 3);

        m.forget("wg0");
        assert!(m.get("wg0").is_some(), "present interfaces are kept");
        tick(&mut m, &mut store, t0 + Duration::from_secs(4), vec![]);
        m.forget("wg0");
        assert!(m.get("wg0").is_none());
    }

    #[test]
    fn offline_traffic_is_booked_on_startup() {
        let mut store = UsageStore::default();
        store.record_mut("wg0").counters = Some(CounterMark { rx: 1000, tx: 1000, uptime_secs: 500 });

        let mut m = Monitor::new(3);
        tick(&mut m, &mut store, Instant::now(), vec![sample("wg0", 4000, 1500)]);
        assert_eq!(store.day("wg0", today()), Traffic::new(3000, 500));
        // Not part of this session's numbers.
        assert_eq!(m.get("wg0").unwrap().session, Traffic::ZERO);
        assert_eq!(store.record("wg0").unwrap().counters.unwrap().rx, 4000);

        // Disabled: only the baseline is updated.
        let mut store2 = UsageStore::default();
        store2.record_mut("wg0").counters = Some(CounterMark { rx: 1000, tx: 1000, uptime_secs: 500 });
        let mut m2 = Monitor::new(3);
        let mut acct = Accounting { store: &mut store2, today: today(), uptime_secs: Some(1000), count_offline: false };
        m2.update(vec![sample("wg0", 4000, 1500)], Instant::now(), &mut acct);
        assert_eq!(store2.day("wg0", today()), Traffic::ZERO);
    }
}
