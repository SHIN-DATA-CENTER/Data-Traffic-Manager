//! Geometry for the charts drawn by the UI.
//!
//! The live graphs are rendered by Slint `Path` elements from SVG path
//! commands. Coordinates use a view box of `(len - 1) x 100` where y grows
//! downwards, so the UI only has to set the matching `viewbox-*` values.

use std::collections::VecDeque;
use std::fmt::Write;

use chrono::{Datelike, Days, NaiveDate};

use crate::format::{self, RateUnit};
use crate::monitor::Rate;
use crate::usage::Traffic;

/// Height of the path view box.
pub const VIEWBOX_HEIGHT: f32 = 100.0;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RateGraph {
    pub rx_area: String,
    pub rx_line: String,
    pub tx_area: String,
    pub tx_line: String,
    /// Width of the view box (number of samples - 1).
    pub viewbox_width: f32,
    /// Label of the top grid line.
    pub max_label: String,
    /// Label of the middle grid line.
    pub mid_label: String,
    /// Per-sample values for the hover tooltip.
    pub points: Vec<GraphPoint>,
}

/// One sample of the live graph, for the hover tooltip.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GraphPoint {
    /// Received rate relative to the axis maximum (0..=1).
    pub rx: f32,
    /// Transmitted rate relative to the axis maximum (0..=1).
    pub tx: f32,
    pub label: String,
}

/// Builds the live graph for the given rate history, sampled every
/// `interval_ms` milliseconds.
pub fn rate_graph(history: &VecDeque<Rate>, unit: RateUnit, interval_ms: u64) -> RateGraph {
    let peak = history.iter().map(|r| r.rx.max(r.tx)).fold(0.0, f64::max);
    let scale = nice_scale(peak, unit);
    let rx: Vec<f64> = history.iter().map(|r| r.rx).collect();
    let tx: Vec<f64> = history.iter().map(|r| r.tx).collect();
    let last = history.len().saturating_sub(1);
    let points = history
        .iter()
        .enumerate()
        .map(|(i, r)| GraphPoint {
            rx: (r.rx / scale).clamp(0.0, 1.0) as f32,
            tx: (r.tx / scale).clamp(0.0, 1.0) as f32,
            label: format!(
                "{}  ↓ {}  ↑ {}",
                time_ago((last - i) as u64 * interval_ms),
                format::rate(r.rx, unit),
                format::rate(r.tx, unit)
            ),
        })
        .collect();
    RateGraph {
        rx_area: area_path(&rx, scale),
        rx_line: line_path(&rx, scale),
        tx_area: area_path(&tx, scale),
        tx_line: line_path(&tx, scale),
        viewbox_width: history.len().saturating_sub(1).max(1) as f32,
        max_label: format::rate(scale, unit),
        mid_label: format::rate(scale / 2.0, unit),
        points,
    }
}

fn time_ago(ms: u64) -> String {
    if ms == 0 {
        return "現在".into();
    }
    let secs = ms / 1000;
    let tenths = ms % 1000 / 100;
    if secs >= 60 {
        format!("{} 分 {} 秒前", secs / 60, secs % 60)
    } else if tenths != 0 {
        format!("{secs}.{tenths} 秒前")
    } else {
        format!("{secs} 秒前")
    }
}

/// Small line charts for the interface list: (rx, tx) path commands.
pub fn sparkline(history: &VecDeque<Rate>) -> (String, String) {
    let peak = history.iter().map(|r| r.rx.max(r.tx)).fold(0.0, f64::max);
    // A tiny floor keeps an idle interface flat instead of amplifying noise.
    let scale = peak.max(1024.0) * 1.1;
    let rx: Vec<f64> = history.iter().map(|r| r.rx).collect();
    let tx: Vec<f64> = history.iter().map(|r| r.tx).collect();
    (line_path(&rx, scale), line_path(&tx, scale))
}

/// Rounds `peak` (bytes/s) up to a "nice" axis maximum (1, 2 or 5 times a
/// power of ten in the display unit), with a minimum of 1 KB/s or 10 kbps.
pub fn nice_scale(peak: f64, unit: RateUnit) -> f64 {
    let (factor, step, minimum) = match unit {
        RateUnit::Bytes => (1.0, 1024.0, 1024.0),
        RateUnit::Bits => (8.0, 1000.0, 10_000.0),
    };
    let value = (peak * factor * 1.05).max(minimum);
    // Express the value in the unit it will be displayed in (KB, MB, ...).
    let mut magnitude = 1.0;
    while value / magnitude >= 1000.0 {
        magnitude *= step;
    }
    let mantissa = value / magnitude;
    let decade = 10f64.powf(mantissa.log10().floor());
    let nice =
        [1.0, 2.0, 5.0, 10.0].into_iter().map(|m| m * decade).find(|n| *n >= mantissa - 1e-9).unwrap_or(10.0 * decade);
    if nice >= 1000.0 {
        // "1000 KB" would be displayed as "0.98 MB": use one whole next unit.
        return magnitude * step / factor;
    }
    nice * magnitude / factor
}

fn y(value: f64, scale: f64) -> f32 {
    let ratio = if scale > 0.0 { (value / scale).clamp(0.0, 1.0) } else { 0.0 };
    VIEWBOX_HEIGHT - (ratio as f32) * VIEWBOX_HEIGHT
}

fn line_path(values: &[f64], scale: f64) -> String {
    let mut path = String::with_capacity(values.len() * 12);
    for (i, value) in values.iter().enumerate() {
        let cmd = if i == 0 { 'M' } else { 'L' };
        let _ = write!(path, "{cmd}{i} {:.2} ", y(*value, scale));
    }
    path.truncate(path.trim_end().len());
    path
}

fn area_path(values: &[f64], scale: f64) -> String {
    if values.is_empty() {
        return String::new();
    }
    let last = values.len() - 1;
    let mut path = format!("M0 {VIEWBOX_HEIGHT} ");
    for (i, value) in values.iter().enumerate() {
        let _ = write!(path, "L{i} {:.2} ", y(*value, scale));
    }
    let _ = write!(path, "L{last} {VIEWBOX_HEIGHT} Z");
    path
}

/// One bar of the daily usage chart.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DayBar {
    pub label: String,
    /// Height of the received part relative to the chart (0..=1).
    pub rx_ratio: f32,
    /// Height of the transmitted part relative to the chart (0..=1).
    pub tx_ratio: f32,
    pub tooltip: String,
    pub today: bool,
    pub weekend: bool,
    pub show_label: bool,
}

const WEEKDAYS: [&str; 7] = ["月", "火", "水", "木", "金", "土", "日"];

/// Daily usage chart data.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DailyChart {
    /// Oldest first.
    pub bars: Vec<DayBar>,
    /// Label of the top grid line.
    pub max_label: String,
    /// Label of the middle grid line.
    pub mid_label: String,
}

/// Bars for the `days` days ending with `today`.
pub fn daily_bars(today: NaiveDate, days: usize, usage: impl Fn(NaiveDate) -> Traffic) -> DailyChart {
    let first = today - Days::new(days.saturating_sub(1) as u64);
    let values: Vec<(NaiveDate, Traffic)> =
        (0..days as u64).map(|i| first + Days::new(i)).map(|date| (date, usage(date))).collect();
    let max = values.iter().map(|(_, t)| t.total()).max().unwrap_or(0);
    // Same 1-2-5 rounding as the rate axis (bytes, 1024 based).
    let scale = nice_scale(max as f64, RateUnit::Bytes);

    let bars = values
        .iter()
        .enumerate()
        .map(|(i, (date, traffic))| {
            let weekday = date.weekday().num_days_from_monday() as usize;
            let days_ago = days - 1 - i;
            DayBar {
                label: format!("{}/{}", date.month(), date.day()),
                rx_ratio: (traffic.rx as f64 / scale) as f32,
                tx_ratio: (traffic.tx as f64 / scale) as f32,
                tooltip: format!(
                    "{}/{} ({})  ↓ {}  ↑ {}  計 {}",
                    date.month(),
                    date.day(),
                    WEEKDAYS[weekday],
                    format::bytes(traffic.rx),
                    format::bytes(traffic.tx),
                    format::bytes(traffic.total())
                ),
                today: days_ago == 0,
                weekend: weekday >= 5,
                show_label: days_ago.is_multiple_of(7),
            }
        })
        .collect();
    DailyChart { bars, max_label: format::bytes(scale as u64), mid_label: format::bytes((scale / 2.0) as u64) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_scales() {
        assert_eq!(nice_scale(0.0, RateUnit::Bytes), 1024.0);
        assert_eq!(nice_scale(1500.0, RateUnit::Bytes), 2048.0);
        assert_eq!(nice_scale(3.0 * 1024.0, RateUnit::Bytes), 5.0 * 1024.0);
        assert_eq!(nice_scale(30.0 * 1024.0, RateUnit::Bytes), 50.0 * 1024.0);
        assert_eq!(nice_scale(600.0 * 1024.0, RateUnit::Bytes), 1024.0 * 1024.0);
        assert_eq!(nice_scale(600_000.0 / 8.0, RateUnit::Bits), 1_000_000.0 / 8.0);
        assert_eq!(nice_scale(1.5 * 1024.0 * 1024.0, RateUnit::Bytes), 2.0 * 1024.0 * 1024.0);
        // 1 Mbps -> axis at 2 Mbps (a little headroom is added).
        assert_eq!(nice_scale(125_000.0, RateUnit::Bits), 2_000_000.0 / 8.0);
        assert_eq!(nice_scale(0.0, RateUnit::Bits), 10_000.0 / 8.0);
        assert_eq!(format::rate(nice_scale(90.0 * 1024.0, RateUnit::Bytes), RateUnit::Bytes), "100 KB/s");
    }

    #[test]
    fn graph_paths() {
        let history: VecDeque<Rate> = [0.0, 512.0, 1024.0].iter().map(|&v| Rate { rx: v, tx: v / 2.0 }).collect();
        let graph = rate_graph(&history, RateUnit::Bytes, 1000);
        assert_eq!(graph.rx_line, "M0 100.00 L1 75.00 L2 50.00");
        assert_eq!(graph.rx_area, "M0 100 L0 100.00 L1 75.00 L2 50.00 L2 100 Z");
        assert_eq!(graph.tx_line, "M0 100.00 L1 87.50 L2 75.00");
        assert_eq!(graph.viewbox_width, 2.0);
        assert_eq!(graph.max_label, "2.00 KB/s");
        assert_eq!(graph.mid_label, "1.00 KB/s");
        assert_eq!(graph.points.len(), 3);
        assert_eq!((graph.points[2].rx, graph.points[2].tx), (0.5, 0.25));
        assert_eq!(graph.points[2].label, "現在  ↓ 1.00 KB/s  ↑ 512 B/s");
        assert_eq!(graph.points[0].label, "2 秒前  ↓ 0 B/s  ↑ 0 B/s");
        assert_eq!(time_ago(1500), "1.5 秒前");
        assert_eq!(time_ago(125_000), "2 分 5 秒前");

        let (rx, _) = sparkline(&history);
        assert!(rx.starts_with("M0 100.00 L1"));
    }

    #[test]
    fn daily_bar_layout() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(); // Thursday
        let chart = daily_bars(today, 8, |d| {
            if d == today { Traffic::new(300 * 1024, 100 * 1024) } else { Traffic::new(100, 100) }
        });
        let bars = &chart.bars;
        assert_eq!(bars.len(), 8);
        // 400 KB peak -> axis rounded up to 500 KB.
        assert_eq!((chart.max_label.as_str(), chart.mid_label.as_str()), ("500 KB", "250 KB"));
        let last = bars.last().unwrap();
        assert!(last.today);
        assert_eq!(last.label, "10/1");
        assert_eq!((last.rx_ratio, last.tx_ratio), (0.6, 0.2));
        assert_eq!(last.tooltip, "10/1 (木)  ↓ 300 KB  ↑ 100 KB  計 400 KB");
        assert!(last.show_label && bars[0].show_label && !bars[1].show_label);
        assert_eq!(bars[0].label, "9/24");
        assert!(bars[2].weekend && bars[3].weekend && !bars[4].weekend); // 9/26 Sat, 9/27 Sun
    }
}
