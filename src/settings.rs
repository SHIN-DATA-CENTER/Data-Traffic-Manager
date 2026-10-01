//! User settings persisted between runs.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::format::RateUnit;
use crate::storage;

/// Colour scheme of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    /// Follow the Windows setting.
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeMode {
    pub fn index(self) -> i32 {
        match self {
            ThemeMode::System => 0,
            ThemeMode::Light => 1,
            ThemeMode::Dark => 2,
        }
    }

    pub fn from_index(index: i32) -> Self {
        match index {
            1 => ThemeMode::Light,
            2 => ThemeMode::Dark,
            _ => ThemeMode::System,
        }
    }
}

/// Update intervals offered in the settings (milliseconds).
pub const INTERVAL_CHOICES_MS: [u64; 4] = [500, 1000, 2000, 5000];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Sampling interval in milliseconds.
    pub interval_ms: u64,
    pub rate_unit: RateUnit,
    /// Also list loopback, filter and other hidden interfaces.
    pub show_hidden: bool,
    /// Day of month (1..=28) on which a billing period starts.
    pub billing_day: u32,
    /// Add the traffic that happened while the app was closed (estimated
    /// from the interface counters) to the usage history.
    pub count_offline: bool,
    /// Monthly data limit per interface key, in bytes.
    pub limits: BTreeMap<String, u64>,
    /// Keep running in the system tray when the window is closed.
    pub keep_in_tray: bool,
    pub theme: ThemeMode,
    /// Interface selected when the app was closed.
    pub selected: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            interval_ms: 1000,
            rate_unit: RateUnit::Bytes,
            show_hidden: false,
            billing_day: 1,
            count_offline: true,
            limits: BTreeMap::new(),
            // Only Windows has a tray that is guaranteed to be visible.
            keep_in_tray: cfg!(windows),
            theme: ThemeMode::System,
            selected: None,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> io::Result<Self> {
        Ok(storage::read_json::<Settings>(path)?.unwrap_or_default().sanitized())
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        storage::write_json_atomic(path, self)
    }

    /// Clamps values edited by hand into their valid ranges.
    pub fn sanitized(mut self) -> Self {
        self.interval_ms = self.interval_ms.clamp(250, 60_000);
        self.billing_day = self.billing_day.clamp(1, 28);
        self.limits.retain(|_, limit| *limit > 0);
        self
    }

    /// Index into [`INTERVAL_CHOICES_MS`] closest to the current interval.
    pub fn interval_index(&self) -> usize {
        INTERVAL_CHOICES_MS
            .iter()
            .enumerate()
            .min_by_key(|(_, ms)| ms.abs_diff(self.interval_ms))
            .map(|(i, _)| i)
            .unwrap_or(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tests::TempDir;

    #[test]
    fn missing_fields_use_defaults() {
        let s: Settings = serde_json::from_str(r#"{ "billing_day": 99, "rate_unit": "bits" }"#).unwrap();
        let s = s.sanitized();
        assert_eq!(s.billing_day, 28);
        assert_eq!(s.rate_unit, RateUnit::Bits);
        assert_eq!(s.interval_ms, 1000);
        assert!(s.count_offline);
        assert_eq!(s.theme, ThemeMode::System);
        assert_eq!(ThemeMode::from_index(ThemeMode::Dark.index()), ThemeMode::Dark);
    }

    #[test]
    fn roundtrip() {
        let dir = TempDir::new("settings");
        let path = dir.0.join("settings.json");
        assert_eq!(Settings::load(&path).unwrap(), Settings::default());

        let mut s = Settings { interval_ms: 2000, show_hidden: true, ..Settings::default() };
        s.limits.insert("wg0".into(), 10);
        s.selected = Some("wg0".into());
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), s);
        assert_eq!(s.interval_index(), 2);
    }
}
