// Release builds on Windows are GUI applications without a console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Instant;

use data_traffic_manager::app::{self, AppCore, Command, ViewModel};
use data_traffic_manager::desktop::{self, Instance};
use data_traffic_manager::format::RateUnit;
use data_traffic_manager::settings::ThemeMode;
use slint::{Model, ModelRc, SharedString, VecModel};

slint::include_modules!();

/// Messages from the UI thread to the sampling thread.
enum Message {
    Command(Command),
    Shutdown,
}

fn main() -> Result<(), slint::PlatformError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == desktop::QUIT_FLAG) {
        // Used by the installer: the running instance saves and exits.
        desktop::request_quit();
        return Ok(());
    }
    let start_minimized = args.iter().any(|arg| arg == desktop::MINIMIZED_FLAG);

    // Set up the window before claiming the instance, so the activation
    // callback of a second launch has something to show.
    let ui = AppWindow::new()?;
    let ui_weak = ui.as_weak();
    let on_activate = move || {
        let ui = ui_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui.upgrade() {
                show_window(&ui);
            }
        });
    };
    let on_quit = || {
        let _ = slint::invoke_from_event_loop(|| {
            let _ = slint::quit_event_loop();
        });
    };
    let _instance = match desktop::single_instance(on_activate, on_quit) {
        Instance::Primary(guard) => guard,
        Instance::Secondary => return Ok(()),
    };

    let core = AppCore::load_default();
    init_window(&ui, &core);

    let tray = AppTray::new()?;
    tray.set_active(core.settings().keep_in_tray);
    {
        let ui = ui.as_weak();
        tray.on_show_window(move || {
            if let Some(ui) = ui.upgrade() {
                show_window(&ui);
            }
        });
    }
    tray.on_quit(|| {
        let _ = slint::quit_event_loop();
    });

    let (sender, receiver) = mpsc::channel();
    connect_callbacks(&ui, &tray, &sender);

    let worker = {
        let ui = ui.as_weak();
        let tray = tray.as_weak();
        thread::Builder::new()
            .name("sampler".into())
            .spawn(move || run_sampler(core, receiver, ui, tray))
            .expect("failed to start the sampling thread")
    };

    // When minimized to the tray the window stays hidden; the tray icon keeps
    // the event loop alive. Without a working tray, show the window anyway.
    if !(start_minimized && tray.get_active()) {
        ui.show()?;
    }
    let result = slint::run_event_loop();

    let _ = sender.send(Message::Shutdown);
    let _ = worker.join();
    result
}

fn show_window(ui: &AppWindow) {
    let window = ui.window();
    window.set_minimized(false);
    let _ = window.show();
}

/// Initial values that do not change on every update.
fn init_window(ui: &AppWindow, core: &AppCore) {
    let settings = core.settings();
    if let Some(family) = japanese_ui_font() {
        ui.set_font_family(family.into());
    }
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    ui.set_data_dir(core.data_dir().display().to_string().into());
    ui.set_interval_index(settings.interval_index() as i32);
    ui.set_unit_index(match settings.rate_unit {
        RateUnit::Bytes => 0,
        RateUnit::Bits => 1,
    });
    ui.set_show_hidden(settings.show_hidden);
    ui.set_billing_day(settings.billing_day as i32);
    ui.set_count_offline(settings.count_offline);
    ui.set_keep_in_tray(settings.keep_in_tray);
    ui.set_theme_index(settings.theme.index());
    ui.set_autostart_supported(desktop::autostart_supported());
    ui.set_autostart(desktop::autostart_enabled());
    if desktop::autostart_enabled() {
        // Keep the entry pointing at this executable if it was moved.
        let _ = desktop::set_autostart(true);
    }

    ui.set_interfaces(ModelRc::new(VecModel::<InterfaceItem>::default()));
    ui.set_graph_points(ModelRc::new(VecModel::<GraphPoint>::default()));
    ui.set_days(ModelRc::new(VecModel::<DayBar>::default()));
    ui.set_months(ModelRc::new(VecModel::<MonthRow>::default()));
}

/// Picks an installed Japanese UI font on Windows. Without an explicit
/// family, Japanese text may be drawn with a Chinese fallback font.
fn japanese_ui_font() -> Option<&'static str> {
    if !cfg!(windows) {
        return None;
    }
    let fonts = std::path::Path::new(&std::env::var_os("WINDIR")?).join("Fonts");
    [("YuGothR.ttc", "Yu Gothic UI"), ("meiryo.ttc", "Meiryo UI"), ("msgothic.ttc", "MS UI Gothic")]
        .into_iter()
        .find(|(file, _)| fonts.join(file).exists())
        .map(|(_, family)| family)
}

fn connect_callbacks(ui: &AppWindow, tray: &AppTray, sender: &Sender<Message>) {
    let send = {
        let sender = sender.clone();
        move |command: Command| {
            let _ = sender.send(Message::Command(command));
        }
    };

    ui.on_select_interface({
        let send = send.clone();
        move |key| send(Command::Select(key.into()))
    });
    ui.on_interval_changed({
        let send = send.clone();
        move |index| send(Command::SetIntervalIndex(index.max(0) as usize))
    });
    ui.on_unit_changed({
        let send = send.clone();
        move |index| send(Command::SetRateUnit(if index == 1 { RateUnit::Bits } else { RateUnit::Bytes }))
    });
    ui.on_show_hidden_changed({
        let send = send.clone();
        move |show| send(Command::SetShowHidden(show))
    });
    ui.on_billing_day_changed({
        let send = send.clone();
        move |day| send(Command::SetBillingDay(day.clamp(1, 28) as u32))
    });
    ui.on_count_offline_changed({
        let send = send.clone();
        move |on| send(Command::SetCountOffline(on))
    });
    ui.on_theme_changed({
        let send = send.clone();
        move |index| send(Command::SetTheme(ThemeMode::from_index(index)))
    });
    ui.on_keep_in_tray_changed({
        let send = send.clone();
        let tray = tray.as_weak();
        move |on| {
            if let Some(tray) = tray.upgrade() {
                tray.set_active(on);
            }
            send(Command::SetKeepInTray(on));
        }
    });
    ui.on_autostart_changed({
        let ui = ui.as_weak();
        move |on| {
            let Some(ui) = ui.upgrade() else { return };
            if let Err(err) = desktop::set_autostart(on) {
                ui.set_notice_text(format!("自動起動の設定を変更できませんでした: {err}").into());
            }
            ui.set_autostart(desktop::autostart_enabled());
        }
    });
    ui.on_limit_changed({
        let send = send.clone();
        move |key, gb| send(Command::SetLimitGb { key: key.into(), gb: gb.max(0) as u32 })
    });
    ui.on_reset_session({
        let send = send.clone();
        move || send(Command::ResetSession)
    });
    ui.on_delete_history({
        let send = send.clone();
        move |key| send(Command::DeleteHistory(key.into()))
    });
    ui.on_open_data_dir({
        let ui = ui.as_weak();
        move || {
            let Some(ui) = ui.upgrade() else { return };
            let dir = data_traffic_manager::storage::data_dir();
            if let Err(err) = desktop::open_folder(&dir) {
                ui.set_notice_text(format!("フォルダーを開けませんでした: {err}").into());
            }
        }
    });
}

/// Samples the counters on a background thread and pushes view updates to
/// the UI thread.
fn run_sampler(mut core: AppCore, receiver: Receiver<Message>, ui: slint::Weak<AppWindow>, tray: slint::Weak<AppTray>) {
    let mut next_tick = Instant::now();
    loop {
        match receiver.recv_timeout(next_tick.saturating_duration_since(Instant::now())) {
            Ok(Message::Command(command)) => {
                core.apply(command);
                // A shorter interval takes effect immediately.
                next_tick = next_tick.min(Instant::now() + core.interval());
                publish(&core, &ui, &tray);
            }
            Ok(Message::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                let now = Instant::now();
                core.tick(now);
                publish(&core, &ui, &tray);
                next_tick += core.interval();
                if next_tick <= now {
                    // We fell behind (e.g. the system was suspended).
                    next_tick = now + core.interval();
                }
            }
        }
    }
    core.save();
}

fn publish(core: &AppCore, ui: &slint::Weak<AppWindow>, tray: &slint::Weak<AppTray>) {
    let view = core.view(chrono::Local::now().naive_local());
    let tray = tray.clone();
    let _ = ui.upgrade_in_event_loop(move |ui| {
        if let Some(tray) = tray.upgrade() {
            tray.set_tip(tray_tooltip(&view).into());
        }
        apply_view(&ui, view);
    });
}

fn tray_tooltip(view: &ViewModel) -> String {
    match &view.detail {
        Some(d) if d.status == "up" => {
            format!("Data Traffic Manager\n{}\n↓ {}  ↑ {}", d.name, d.rx_rate, d.tx_rate)
        }
        Some(d) => format!("Data Traffic Manager\n{} ({})", d.name, d.status_label),
        None => "Data Traffic Manager".into(),
    }
}

/// Copies the view model into the window. Models are updated in place so
/// that list scroll positions and hover states survive the refresh.
fn apply_view(ui: &AppWindow, view: ViewModel) {
    ui.set_error_text(view.error.into());

    let items: Vec<InterfaceItem> = view.interfaces.into_iter().map(interface_item).collect();
    update_model(&ui.get_interfaces(), items);

    let Some(d) = view.detail else {
        ui.set_detail(Detail::default());
        update_model(&ui.get_graph_points(), Vec::new());
        update_model(&ui.get_days(), Vec::new());
        update_model(&ui.get_months(), Vec::new());
        return;
    };

    if ui.get_limit_key() != d.key.as_str() {
        ui.set_limit_key(d.key.as_str().into());
        ui.set_limit_edit(d.limit_gb.min(i32::MAX as u32) as i32);
    }

    let points =
        d.graph.points.iter().map(|p| GraphPoint { rx: p.rx, tx: p.tx, label: p.label.as_str().into() }).collect();
    update_model(&ui.get_graph_points(), points);

    let days = d
        .daily
        .bars
        .iter()
        .map(|b| DayBar {
            label: b.label.as_str().into(),
            rx_ratio: b.rx_ratio,
            tx_ratio: b.tx_ratio,
            tooltip: b.tooltip.as_str().into(),
            today: b.today,
            weekend: b.weekend,
            show_label: b.show_label,
        })
        .collect();
    update_model(&ui.get_days(), days);

    let months = d
        .months
        .iter()
        .map(|m| MonthRow {
            label: m.label.as_str().into(),
            rx: m.traffic.rx.as_str().into(),
            tx: m.traffic.tx.as_str().into(),
            total: m.traffic.total.as_str().into(),
            ratio: m.ratio,
            current: m.current,
        })
        .collect();
    update_model(&ui.get_months(), months);

    ui.set_detail(Detail {
        valid: true,
        key: d.key.into(),
        name: d.name.into(),
        description: d.description.into(),
        kind: d.kind.into(),
        kind_label: d.kind_label.into(),
        status: d.status.into(),
        status_label: d.status_label.into(),
        link_speed: d.link_speed.into(),
        rx_rate: d.rx_rate.into(),
        tx_rate: d.tx_rate.into(),
        rx_peak: d.rx_peak.into(),
        tx_peak: d.tx_peak.into(),
        graph_rx_area: d.graph.rx_area.into(),
        graph_rx_line: d.graph.rx_line.into(),
        graph_tx_area: d.graph.tx_area.into(),
        graph_tx_line: d.graph.tx_line.into(),
        graph_viewbox_width: d.graph.viewbox_width,
        graph_max: d.graph.max_label.into(),
        graph_mid: d.graph.mid_label.into(),
        graph_span: d.graph_span.into(),
        graph_start: d.graph_start.into(),
        session: traffic_text(d.session),
        today: traffic_text(d.today),
        period: traffic_text(d.period),
        period_label: d.period_label.into(),
        forecast: d.forecast.into(),
        all_time: traffic_text(d.all_time),
        all_time_since: d.all_time_since.into(),
        limit_gb: d.limit_gb.min(i32::MAX as u32) as i32,
        limit_ratio: d.limit_ratio,
        limit_level: d.limit_level,
        limit_text: d.limit_text.into(),
        days_max: d.daily.max_label.into(),
        days_mid: d.daily.mid_label.into(),
    });
}

fn interface_item(i: app::InterfaceView) -> InterfaceItem {
    InterfaceItem {
        key: i.key.into(),
        name: i.name.into(),
        description: i.description.into(),
        kind: i.kind.into(),
        kind_label: i.kind_label.into(),
        status: i.status.into(),
        rx_rate: i.rx_rate.into(),
        tx_rate: i.tx_rate.into(),
        today_total: i.today_total.into(),
        spark_rx: i.spark_rx.into(),
        spark_tx: i.spark_tx.into(),
        selected: i.selected,
    }
}

fn traffic_text(t: app::TrafficText) -> TrafficText {
    TrafficText { rx: SharedString::from(t.rx), tx: t.tx.into(), total: t.total.into() }
}

/// Replaces the contents of a `VecModel`, touching only rows that changed.
fn update_model<T: Clone + PartialEq + 'static>(model: &ModelRc<T>, rows: Vec<T>) {
    let Some(vec_model) = model.as_any().downcast_ref::<VecModel<T>>() else {
        return;
    };
    if vec_model.row_count() != rows.len() {
        vec_model.set_vec(rows);
        return;
    }
    for (index, row) in rows.into_iter().enumerate() {
        if vec_model.row_data(index).as_ref() != Some(&row) {
            vec_model.set_row_data(index, row);
        }
    }
}
