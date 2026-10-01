//! UI-independent application state: owns the settings, the usage history
//! and the live monitor, reacts to user commands and renders a plain-data
//! [`ViewModel`] that the Slint layer only has to copy into the window.

use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrono::{Datelike, NaiveDate, NaiveDateTime};

use crate::chart;
use crate::format::{self, GIB, RateUnit};
use crate::monitor::{Accounting, LiveInterface, Monitor, Rate};
use crate::platform::{self, InterfaceKind, InterfaceSample};
use crate::settings::{INTERVAL_CHOICES_MS, Settings, ThemeMode};
use crate::storage;
use crate::usage::{Period, Traffic, UsageStore};

/// Number of samples kept for the live graph.
pub const HISTORY_LEN: usize = 120;
/// Days shown in the daily usage chart.
pub const DAILY_BARS: usize = 30;
/// Billing periods shown in the monthly table.
pub const MONTH_ROWS: u32 = 12;
/// How often the usage history is written to disk while running.
const SAVE_EVERY: Duration = Duration::from_secs(30);

/// User actions forwarded from the UI.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Select(String),
    SetIntervalIndex(usize),
    SetRateUnit(RateUnit),
    SetShowHidden(bool),
    SetBillingDay(u32),
    SetCountOffline(bool),
    SetKeepInTray(bool),
    SetTheme(ThemeMode),
    /// Monthly limit for an interface in GB; 0 removes the limit.
    SetLimitGb {
        key: String,
        gb: u32,
    },
    ResetSession,
    DeleteHistory(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Up,
    Down,
    /// Not currently present; only usage history exists.
    Absent,
}

impl Status {
    pub fn id(self) -> &'static str {
        match self {
            Status::Up => "up",
            Status::Down => "down",
            Status::Absent => "absent",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::Up => "接続中",
            Status::Down => "切断",
            Status::Absent => "未接続（履歴のみ）",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ViewModel {
    pub interfaces: Vec<InterfaceView>,
    pub detail: Option<DetailView>,
    /// Error from the last collection attempt, if any.
    pub error: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct InterfaceView {
    pub key: String,
    pub name: String,
    pub description: String,
    pub kind: String,
    pub kind_label: String,
    pub status: String,
    pub rx_rate: String,
    pub tx_rate: String,
    pub today_total: String,
    pub spark_rx: String,
    pub spark_tx: String,
    pub selected: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DetailView {
    pub key: String,
    pub name: String,
    pub description: String,
    pub kind: String,
    pub kind_label: String,
    pub status: String,
    pub status_label: String,
    pub link_speed: String,
    pub rx_rate: String,
    pub tx_rate: String,
    pub rx_peak: String,
    pub tx_peak: String,
    pub graph: chart::RateGraph,
    pub graph_span: String,
    /// Label of the left end of the time axis, e.g. "2 分前".
    pub graph_start: String,
    pub session: TrafficText,
    pub today: TrafficText,
    pub period: TrafficText,
    pub period_label: String,
    pub forecast: String,
    pub all_time: TrafficText,
    pub all_time_since: String,
    pub limit_gb: u32,
    pub limit_ratio: f32,
    pub limit_level: i32,
    pub limit_text: String,
    pub daily: chart::DailyChart,
    pub months: Vec<MonthView>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrafficText {
    pub rx: String,
    pub tx: String,
    pub total: String,
}

impl From<Traffic> for TrafficText {
    fn from(t: Traffic) -> Self {
        TrafficText { rx: format::bytes(t.rx), tx: format::bytes(t.tx), total: format::bytes(t.total()) }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MonthView {
    pub label: String,
    pub traffic: TrafficText,
    pub ratio: f32,
    pub current: bool,
}

pub struct AppCore {
    settings: Settings,
    store: UsageStore,
    monitor: Monitor,
    data_dir: PathBuf,
    error: String,
    last_save: Instant,
}

impl AppCore {
    /// Loads settings and usage history from `data_dir`.
    pub fn load(data_dir: PathBuf) -> Self {
        let mut errors = Vec::new();
        let settings = Settings::load(&data_dir.join("settings.json")).unwrap_or_else(|e| {
            errors.push(format!("設定を読み込めませんでした: {e}"));
            Settings::default()
        });
        let store = UsageStore::load(&data_dir.join("usage.json")).unwrap_or_else(|e| {
            errors.push(format!("使用量の履歴を読み込めませんでした: {e}"));
            UsageStore::default()
        });
        AppCore {
            settings,
            store,
            monitor: Monitor::new(HISTORY_LEN),
            data_dir,
            error: errors.join(" / "),
            last_save: Instant::now(),
        }
    }

    /// Uses the default data directory.
    pub fn load_default() -> Self {
        Self::load(storage::data_dir())
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    pub fn interval(&self) -> Duration {
        Duration::from_millis(self.settings.interval_ms)
    }

    /// Samples the OS counters and updates the state.
    pub fn tick(&mut self, now: Instant) {
        let uptime = platform::uptime().map(|d| d.as_secs());
        let today = chrono::Local::now().date_naive();
        self.tick_with(platform::collect(), now, today, uptime);
        if now.duration_since(self.last_save) >= SAVE_EVERY {
            self.save_usage();
        }
    }

    /// [`AppCore::tick`] with injected inputs (used by tests).
    pub fn tick_with(
        &mut self,
        samples: io::Result<Vec<InterfaceSample>>,
        now: Instant,
        today: NaiveDate,
        uptime_secs: Option<u64>,
    ) {
        match samples {
            Ok(samples) => {
                self.error.clear();
                let mut acct = Accounting {
                    store: &mut self.store,
                    today,
                    uptime_secs,
                    count_offline: self.settings.count_offline,
                };
                self.monitor.update(samples, now, &mut acct);
            }
            Err(err) => self.error = format!("インターフェース情報を取得できませんでした: {err}"),
        }
    }

    pub fn apply(&mut self, command: Command) {
        match command {
            Command::Select(key) => self.settings.selected = Some(key),
            Command::SetIntervalIndex(index) => {
                let ms = INTERVAL_CHOICES_MS[index.min(INTERVAL_CHOICES_MS.len() - 1)];
                if ms != self.settings.interval_ms {
                    self.settings.interval_ms = ms;
                    self.monitor.clear_history();
                }
            }
            Command::SetRateUnit(unit) => self.settings.rate_unit = unit,
            Command::SetShowHidden(show) => self.settings.show_hidden = show,
            Command::SetBillingDay(day) => self.settings.billing_day = day.clamp(1, 28),
            Command::SetCountOffline(on) => self.settings.count_offline = on,
            Command::SetKeepInTray(on) => self.settings.keep_in_tray = on,
            Command::SetTheme(theme) => self.settings.theme = theme,
            Command::SetLimitGb { key, gb } => {
                if gb == 0 {
                    self.settings.limits.remove(&key);
                } else {
                    self.settings.limits.insert(key, u64::from(gb) * GIB);
                }
            }
            Command::ResetSession => self.monitor.reset_session(),
            Command::DeleteHistory(key) => {
                self.store.remove(&key);
                self.settings.limits.remove(&key);
                self.monitor.forget(&key);
                self.save_usage();
            }
        }
        self.save_settings();
    }

    /// Writes everything to disk (called on exit).
    pub fn save(&mut self) {
        self.save_usage();
        self.save_settings();
    }

    fn save_usage(&mut self) {
        self.last_save = Instant::now();
        if let Err(e) = self.store.save(&self.data_dir.join("usage.json")) {
            self.error = format!("使用量の履歴を保存できませんでした: {e}");
        }
    }

    fn save_settings(&mut self) {
        if let Err(e) = self.settings.save(&self.data_dir.join("settings.json")) {
            self.error = format!("設定を保存できませんでした: {e}");
        }
    }

    /// Builds the data shown by the window.
    pub fn view(&self, now: NaiveDateTime) -> ViewModel {
        let today = now.date();
        let unit = self.settings.rate_unit;
        let mut rows: Vec<Row> = Vec::new();

        for live in self.monitor.interfaces() {
            let s = &live.sample;
            let status = if !live.present {
                Status::Absent
            } else if s.is_up {
                Status::Up
            } else {
                Status::Down
            };
            let used = s.rx_bytes > 0 || s.tx_bytes > 0 || self.store.has_usage(&s.key);
            let shown = self.settings.show_hidden
                || (s.visible && (status == Status::Up || used))
                || (status == Status::Absent && s.visible && self.store.has_usage(&s.key));
            if shown {
                rows.push(Row {
                    key: &s.key,
                    name: &s.name,
                    kind: s.kind,
                    description: &s.description,
                    status,
                    live: Some(live),
                });
            }
        }
        // Interfaces that only exist in the history (e.g. a WireGuard tunnel
        // that is currently disconnected).
        for (key, record) in &self.store.interfaces {
            let shown = self.settings.show_hidden || !record.hidden;
            if self.monitor.get(key).is_none() && !record.daily.is_empty() && shown {
                rows.push(Row {
                    key,
                    name: key,
                    kind: record.kind.unwrap_or(InterfaceKind::Other),
                    description: &record.description,
                    status: Status::Absent,
                    live: None,
                });
            }
        }
        rows.sort_by(|a, b| {
            (a.status, kind_order(a.kind), a.name.to_lowercase()).cmp(&(
                b.status,
                kind_order(b.kind),
                b.name.to_lowercase(),
            ))
        });

        let selected = self
            .settings
            .selected
            .as_deref()
            .filter(|key| rows.iter().any(|r| r.key == *key))
            .or_else(|| rows.first().map(|r| r.key));

        let interfaces = rows
            .iter()
            .map(|row| {
                let (rx_rate, tx_rate, spark_rx, spark_tx) = match row.live {
                    Some(live) if row.status != Status::Absent => {
                        let (srx, stx) = chart::sparkline(&live.history);
                        (format::rate(live.rate.rx, unit), format::rate(live.rate.tx, unit), srx, stx)
                    }
                    _ => ("—".into(), "—".into(), String::new(), String::new()),
                };
                InterfaceView {
                    key: row.key.to_string(),
                    name: row.name.to_string(),
                    description: row.description.to_string(),
                    kind: row.kind.id().into(),
                    kind_label: row.kind.label().into(),
                    status: row.status.id().into(),
                    rx_rate,
                    tx_rate,
                    today_total: format::bytes(self.store.day(row.key, today).total()),
                    spark_rx,
                    spark_tx,
                    selected: Some(row.key) == selected,
                }
            })
            .collect();

        let detail = selected.and_then(|key| rows.iter().find(|r| r.key == key)).map(|row| self.detail(row, now));

        ViewModel { interfaces, detail, error: self.error.clone() }
    }

    fn detail(&self, row: &Row, now: NaiveDateTime) -> DetailView {
        let today = now.date();
        let unit = self.settings.rate_unit;
        let key = row.key;
        let live = row.live.filter(|_| row.status != Status::Absent);

        let rate = live.map(|l| l.rate).unwrap_or_default();
        let peak = live.map(|l| l.peak).unwrap_or_default();
        let empty_history;
        let history = match row.live {
            Some(l) => &l.history,
            None => {
                empty_history = std::iter::repeat_n(Rate::default(), HISTORY_LEN).collect();
                &empty_history
            }
        };
        let graph = chart::rate_graph(history, unit, self.settings.interval_ms);
        let span_secs = self.settings.interval_ms * self.monitor.history_len() as u64 / 1000;
        let span = if span_secs >= 120 && span_secs.is_multiple_of(60) {
            format!("{} 分", span_secs / 60)
        } else {
            format!("{span_secs} 秒")
        };
        let graph_span = format!("直近 {span}");
        let graph_start = format!("{span}前");

        let period = Period::containing(today, self.settings.billing_day);
        let period_traffic = self.store.sum(key, period.start, period.end);
        let forecast = forecast(period, period_traffic, now);

        let limit = self.settings.limits.get(key).copied().unwrap_or(0);
        let (limit_ratio, limit_level, limit_text) = limit_status(period_traffic.total(), limit);

        let first_day = self.store.record(key).and_then(|r| r.daily.keys().next().copied());
        let all_time_since = first_day
            .map(|d| format!("{}/{}/{} から", d.year(), d.month(), d.day()))
            .unwrap_or_else(|| "記録なし".into());

        let daily = chart::daily_bars(today, DAILY_BARS, |date| self.store.day(key, date));

        let month_totals: Vec<(Period, Traffic)> = (0..MONTH_ROWS)
            .map(|i| {
                let p = period.previous(i);
                (p, self.store.sum(key, p.start, p.end))
            })
            .collect();
        let month_max = month_totals.iter().map(|(_, t)| t.total()).max().unwrap_or(0).max(1);
        let months = month_totals
            .into_iter()
            .enumerate()
            .filter(|(i, (_, t))| *i == 0 || !t.is_zero())
            .map(|(i, (p, t))| MonthView {
                label: p.label(),
                traffic: t.into(),
                ratio: t.total() as f32 / month_max as f32,
                current: i == 0,
            })
            .collect();

        let link_speed =
            live.and_then(|l| l.sample.link_speed_bps).map(format::link_speed).unwrap_or_else(|| "—".into());

        DetailView {
            key: key.to_string(),
            name: row.name.to_string(),
            description: row.description.to_string(),
            kind: row.kind.id().into(),
            kind_label: row.kind.label().into(),
            status: row.status.id().into(),
            status_label: row.status.label().into(),
            link_speed,
            rx_rate: if live.is_some() { format::rate(rate.rx, unit) } else { "—".into() },
            tx_rate: if live.is_some() { format::rate(rate.tx, unit) } else { "—".into() },
            rx_peak: format::rate(peak.rx, unit),
            tx_peak: format::rate(peak.tx, unit),
            graph,
            graph_span,
            graph_start,
            session: live.map(|l| l.session).unwrap_or_default().into(),
            today: self.store.day(key, today).into(),
            period: period_traffic.into(),
            period_label: period.label(),
            forecast,
            all_time: self.store.all_time(key).into(),
            all_time_since,
            limit_gb: (limit / GIB).min(u64::from(u32::MAX)) as u32,
            limit_ratio,
            limit_level,
            limit_text,
            daily,
            months,
        }
    }
}

struct Row<'a> {
    key: &'a str,
    name: &'a str,
    kind: InterfaceKind,
    description: &'a str,
    status: Status,
    live: Option<&'a LiveInterface>,
}

/// VPN adapters first: they are what this application is mostly used for.
fn kind_order(kind: InterfaceKind) -> u8 {
    match kind {
        InterfaceKind::Vpn => 0,
        InterfaceKind::Ethernet => 1,
        InterfaceKind::Wireless => 2,
        InterfaceKind::Cellular => 3,
        InterfaceKind::Other => 4,
        InterfaceKind::Loopback => 5,
    }
}

/// Projects the usage at the end of the billing period from the average so
/// far. Only shown after a full day, before that the estimate is noise.
fn forecast(period: Period, used: Traffic, now: NaiveDateTime) -> String {
    let start = period.start.and_hms_opt(0, 0, 0).expect("midnight exists");
    let elapsed_days = (now - start).num_seconds() as f64 / 86_400.0;
    if elapsed_days < 1.0 || used.is_zero() {
        return String::new();
    }
    let total_days = f64::from(period.len_days());
    let projected = used.total() as f64 / elapsed_days.min(total_days) * total_days;
    format!("このペースだと期間末に約 {}", format::bytes(projected as u64))
}

/// (ratio, level 0=ok 1=warning 2=over, description) for a monthly limit.
fn limit_status(used: u64, limit: u64) -> (f32, i32, String) {
    if limit == 0 {
        return (0.0, 0, "上限は設定されていません".into());
    }
    let ratio = used as f64 / limit as f64;
    let level = if ratio >= 1.0 {
        2
    } else if ratio >= 0.8 {
        1
    } else {
        0
    };
    let text = if used > limit {
        format!("上限 {} を {} 超過しています", format::bytes(limit), format::bytes(used - limit))
    } else {
        format!(
            "上限 {} の {:.0}% を使用（残り {}）",
            format::bytes(limit),
            (ratio * 100.0).floor(),
            format::bytes(limit - used)
        )
    };
    (ratio.min(1.0) as f32, level, text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tests::TempDir;

    fn sample(key: &str, kind: InterfaceKind, up: bool, rx: u64, tx: u64) -> InterfaceSample {
        InterfaceSample {
            key: key.into(),
            name: key.into(),
            description: format!("{key} adapter"),
            kind,
            is_up: up,
            visible: kind != InterfaceKind::Loopback,
            rx_bytes: rx,
            tx_bytes: tx,
            link_speed_bps: Some(1_000_000_000),
        }
    }

    fn at(day: u32, h: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, day).unwrap().and_hms_opt(h, 0, 0).unwrap()
    }

    #[test]
    fn view_lists_and_selects_interfaces() {
        let dir = TempDir::new("core-view");
        let mut core = AppCore::load(dir.0.clone());
        let t0 = Instant::now();
        let today = at(1, 12).date();
        let set = |wg: u64, eth: u64| {
            vec![
                sample("Ethernet", InterfaceKind::Ethernet, true, eth, eth / 2),
                sample("office", InterfaceKind::Vpn, true, wg, wg / 4),
                sample("Loopback", InterfaceKind::Loopback, true, 0, 0),
                sample("Wi-Fi Direct", InterfaceKind::Wireless, false, 0, 0),
            ]
        };
        core.tick_with(Ok(set(1000, 5000)), t0, today, Some(100));
        core.tick_with(Ok(set(3048, 9000)), t0 + Duration::from_secs(1), today, Some(101));

        let view = core.view(at(1, 12));
        let names: Vec<_> = view.interfaces.iter().map(|i| i.name.as_str()).collect();
        // VPN first, hidden loopback and the unused, disconnected adapter omitted.
        assert_eq!(names, ["office", "Ethernet"]);
        assert!(view.interfaces[0].selected);
        assert_eq!(view.interfaces[0].rx_rate, "2.00 KB/s");

        let detail = view.detail.as_ref().unwrap();
        assert_eq!(detail.key, "office");
        assert_eq!(detail.today.rx, "2.00 KB");
        assert_eq!(detail.status, "up");
        assert_eq!(detail.link_speed, "1.00 Gbps");
        assert_eq!(detail.graph_span, "直近 2 分");
        assert_eq!(detail.daily.bars.len(), DAILY_BARS);
        assert_eq!(detail.months.len(), 1);

        core.apply(Command::SetShowHidden(true));
        core.apply(Command::Select("Ethernet".into()));
        core.apply(Command::SetRateUnit(RateUnit::Bits));
        let view = core.view(at(1, 12));
        assert_eq!(view.interfaces.len(), 4);
        let detail = view.detail.unwrap();
        assert_eq!(detail.key, "Ethernet");
        assert_eq!(detail.rx_rate, "32.0 kbps");

        // Settings were persisted.
        let reloaded = Settings::load(&dir.0.join("settings.json")).unwrap();
        assert!(reloaded.show_hidden);
        assert_eq!(reloaded.selected.as_deref(), Some("Ethernet"));
    }

    #[test]
    fn absent_interfaces_keep_their_history() {
        let dir = TempDir::new("core-absent");
        let t0 = Instant::now();
        let today = at(5, 9).date();
        {
            let mut core = AppCore::load(dir.0.clone());
            core.tick_with(Ok(vec![sample("home", InterfaceKind::Vpn, true, 0, 0)]), t0, today, Some(10));
            core.tick_with(
                Ok(vec![sample("home", InterfaceKind::Vpn, true, 3 * GIB, GIB)]),
                t0 + Duration::from_secs(1),
                today,
                Some(11),
            );
            core.apply(Command::SetLimitGb { key: "home".into(), gb: 5 });
            core.save();
        }

        // Next run: the tunnel is not connected.
        let mut core = AppCore::load(dir.0.clone());
        core.tick_with(Ok(vec![]), t0, today, Some(20));
        let view = core.view(at(5, 9));
        assert_eq!(view.interfaces.len(), 1);
        let item = &view.interfaces[0];
        assert_eq!((item.name.as_str(), item.status.as_str(), item.kind.as_str()), ("home", "absent", "vpn"));
        assert_eq!(item.rx_rate, "—");

        let detail = view.detail.unwrap();
        assert_eq!(detail.period.total, "4.00 GB");
        assert_eq!(detail.limit_gb, 5);
        assert_eq!(detail.limit_level, 1);
        assert_eq!(detail.limit_text, "上限 5.00 GB の 80% を使用（残り 1.00 GB）");
        assert_eq!(detail.forecast, "このペースだと期間末に約 28.3 GB");

        core.apply(Command::DeleteHistory("home".into()));
        assert!(core.view(at(5, 9)).interfaces.is_empty());
    }

    #[test]
    fn hidden_interfaces_stay_hidden_when_absent() {
        let dir = TempDir::new("core-hidden");
        let t0 = Instant::now();
        let today = at(5, 9).date();
        let mut filter =
            sample("Ethernet 2-WFP Native MAC Layer LightWeight Filter-0000", InterfaceKind::Ethernet, true, 0, 0);
        filter.visible = false;
        {
            let mut core = AppCore::load(dir.0.clone());
            core.tick_with(Ok(vec![filter.clone()]), t0, today, Some(10));
            filter.rx_bytes = 5000;
            core.tick_with(Ok(vec![filter.clone()]), t0 + Duration::from_secs(1), today, Some(11));
            // Unplugged while running.
            core.tick_with(Ok(vec![]), t0 + Duration::from_secs(2), today, Some(12));
            assert!(core.view(at(5, 9)).interfaces.is_empty());
            core.save();
        }
        let mut core = AppCore::load(dir.0.clone());
        core.tick_with(Ok(vec![]), t0, today, Some(20));
        assert!(core.view(at(5, 9)).interfaces.is_empty());
        core.apply(Command::SetShowHidden(true));
        assert_eq!(core.view(at(5, 9)).interfaces.len(), 1);
    }

    #[test]
    fn collection_errors_are_reported() {
        let dir = TempDir::new("core-error");
        let mut core = AppCore::load(dir.0.clone());
        core.tick_with(Err(io::Error::other("boom")), Instant::now(), at(1, 0).date(), None);
        assert!(core.view(at(1, 0)).error.contains("boom"));
        core.tick_with(Ok(vec![]), Instant::now(), at(1, 0).date(), None);
        assert!(core.view(at(1, 0)).error.is_empty());
    }

    #[test]
    fn limit_texts() {
        assert_eq!(limit_status(0, 0).1, 0);
        let (ratio, level, text) = limit_status(12 * GIB, 10 * GIB);
        assert_eq!((ratio, level), (1.0, 2));
        assert_eq!(text, "上限 10.0 GB を 2.00 GB 超過しています");
    }
}
