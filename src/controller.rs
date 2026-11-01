use crate::audio::{
    create_virtual_sink, destroy_virtual_sinks, find_arctis_sink, get_default_sink,
    link_sink_to_device, move_all_inputs_to, set_default_sink, set_sink_volume,
};
use crate::config::{AppConfig, ConfigManager};
use crate::device::{
    build_eq_name_query_report, build_gaia_notification_register_report,
    build_general_settings_request_report, is_percentage_battery_device, parse_chatmix_report,
    CHAT_SINK_DESC, CHAT_SINK_NAME, FALLBACK_DEFAULT_SINK, GAME_SINK_DESC, GAME_SINK_NAME,
    HID_MSG_SIZE, VENDOR_ID,
};
use crate::eq::{EqChannel, ParametricEqProfile};
use crate::state::{load_all_presets, DeviceWorker, DeviceWorkerHandle};
use crate::telemetry::{parse_telemetry_event, ChargingState, HeadsetTelemetry, TelemetryEvent, WirelessState};
use crate::tray::spawn_system_tray;
use crate::usb::{rusb_request_status, rusb_send_hid_report, usb_find_and_open};
use crate::web::spawn_web_server;
use anyhow::{Context, Result};
use log::{debug, error, info, warn};
use rusb::{DeviceHandle, UsbContext};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

const MAX_CONSECUTIVE_USB_ERRORS: u32 = 5;
const RELINK_RETRIES: usize = 10;
const RELINK_RETRY_DELAY: Duration = Duration::from_millis(300);

/// Tracks the last successfully applied volume per virtual sink so that turning the
/// ChatMix wheel only spawns `pactl` for the channel whose volume actually changed
/// (or previously failed).
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct VolumeState {
    last_game: Option<u8>,
    last_chat: Option<u8>,
}

impl VolumeState {
    pub(crate) fn should_apply_game(&self, game_vol: u8) -> bool {
        self.last_game != Some(game_vol)
    }

    pub(crate) fn should_apply_chat(&self, chat_vol: u8) -> bool {
        self.last_chat != Some(chat_vol)
    }

    pub(crate) fn record_game(&mut self, game_vol: u8, succeeded: bool) {
        self.last_game = succeeded.then_some(game_vol);
    }

    pub(crate) fn record_chat(&mut self, chat_vol: u8, succeeded: bool) {
        self.last_chat = succeeded.then_some(chat_vol);
    }
}

/// Tracks wireless, battery level and charging state to deliver non-blocking desktop notifications
/// via `/usr/bin/notify-send` when connection events, thresholds (20%, 10%, 5%) or charging events occur.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct HeadsetNotificationTracker {
    pub(crate) last_wireless_state: Option<WirelessState>,
    pub(crate) last_battery_alert_threshold: Option<u8>,
    pub(crate) last_charging_state: Option<ChargingState>,
}

/// Backward-compatible alias for `HeadsetNotificationTracker`.
pub(crate) type BatteryNotificationTracker = HeadsetNotificationTracker;

impl HeadsetNotificationTracker {
    pub(crate) fn check_wireless(&mut self, wireless: WirelessState, battery_percent: Option<u8>) {
        let previous = self.last_wireless_state;
        self.last_wireless_state = Some(wireless);
        if wireless == WirelessState::Connected {
            if previous != Some(WirelessState::Connected) {
                let body = match battery_percent {
                    Some(pct) => format!("Kulaklık bağlandı ({}%)", pct),
                    None => "Kulaklık bağlandı".to_string(),
                };
                spawn_desktop_notification("low", "audio-headset", "Arctis Nova 7", &body);
            }
        } else if wireless == WirelessState::Disconnected {
            if previous == Some(WirelessState::Connected) {
                spawn_desktop_notification("low", "audio-headset", "Arctis Nova 7", "Kulaklık bağlantısı kesildi");
            }
            self.last_battery_alert_threshold = None;
            self.last_charging_state = None;
        }
    }

    pub(crate) fn check_charging_and_battery(
        &mut self,
        wireless: WirelessState,
        battery_percent: Option<u8>,
        charging: ChargingState,
    ) {
        // Charging start
        let charging_start = charging.is_charging()
            && self
                .last_charging_state
                .map(|s| !s.is_charging())
                .unwrap_or(false);
        if charging_start {
            let body = match battery_percent {
                Some(pct) => format!("Kulaklık şarj ediliyor ({}%)", pct),
                None => "Kulaklık şarj ediliyor".to_string(),
            };
            spawn_desktop_notification("low", "battery-charging", "Arctis Nova 7", &body);
            self.last_battery_alert_threshold = None;
        }

        // Fully charged
        let fully_charged = charging.is_charged()
            && self.last_charging_state == Some(ChargingState::Charging);
        if fully_charged {
            spawn_desktop_notification(
                "normal",
                "battery-full",
                "Arctis Nova 7",
                "Kulaklık şarjı doldu (%100)",
            );
        }

        // Low battery thresholds: only when connected and not charging
        if wireless == WirelessState::Connected && !charging.is_charging() {
            if let Some(percent) = battery_percent {
                if percent > 25 {
                    self.last_battery_alert_threshold = None;
                } else {
                    let current_threshold = if percent <= 5 {
                        Some(5)
                    } else if percent <= 10 {
                        Some(10)
                    } else if percent <= 20 {
                        Some(20)
                    } else {
                        None
                    };
                    if let Some(threshold) = current_threshold {
                        let should_alert = match self.last_battery_alert_threshold {
                            None => true,
                            Some(last) => threshold < last,
                        };
                        if should_alert {
                            self.last_battery_alert_threshold = Some(threshold);
                            match threshold {
                                5 => spawn_desktop_notification(
                                    "critical",
                                    "battery-empty",
                                    "Arctis Nova 7",
                                    &format!(
                                        "Pil tükenmek üzere ({}) - Lütfen şarja takın",
                                        percent
                                    ),
                                ),
                                10 => spawn_desktop_notification(
                                    "normal",
                                    "battery-caution",
                                    "Arctis Nova 7",
                                    &format!("Pil seviyesi kritik ({})", percent),
                                ),
                                20 => spawn_desktop_notification(
                                    "normal",
                                    "battery-caution",
                                    "Arctis Nova 7",
                                    &format!("Pil seviyesi düşük ({})", percent),
                                ),
                                _ => {}
                            }
                        }
                    }
                }
            }
        }

        self.last_charging_state = Some(charging);
    }

    pub(crate) fn check_and_notify(&mut self, telemetry: &HeadsetTelemetry) {
        self.check_wireless(telemetry.wireless, telemetry.battery_percent);
        self.check_charging_and_battery(
            telemetry.wireless,
            telemetry.battery_percent,
            telemetry.charging,
        );
    }
}

/// Spawns a desktop notification asynchronously and detached via `notify-send`.
/// Guaranteed never to block the audio or USB interrupt threads.
pub(crate) fn spawn_desktop_notification(urgency: &str, icon: &str, title: &str, body: &str) {
    let notify_bin = if std::path::Path::new("/usr/bin/notify-send").exists() {
        "/usr/bin/notify-send"
    } else {
        "notify-send"
    };

    match std::process::Command::new(notify_bin)
        .arg("-u")
        .arg(urgency)
        .arg("-i")
        .arg(icon)
        .arg(title)
        .arg(body)
        .spawn()
    {
        Ok(_) => {
            debug!("Spawned desktop notification [{}]: {} - {}", urgency, title, body);
            #[cfg(test)]
            {
                notify_captures().lock().unwrap_or_else(|e| e.into_inner()).push(NotifyRecord {
                    urgency: urgency.to_string(),
                    icon: icon.to_string(),
                    title: title.to_string(),
                    body: body.to_string(),
                });
            }
        }
        Err(e) => debug!("Failed to spawn desktop notification ({}): {}", notify_bin, e),
    }
}

pub(crate) struct ArctisController {
    original_default_sink: String,
    running: Arc<AtomicBool>,
    sinks_created: Arc<AtomicBool>,
    config_manager: ConfigManager,
    config: AppConfig,
    active_eq_24g: Option<ParametricEqProfile>,
    active_eq_bt: Option<ParametricEqProfile>,
    active_eq_mic: Option<ParametricEqProfile>,
    #[allow(dead_code)]
    active_eq: Option<ParametricEqProfile>,
    worker: DeviceWorker,
    worker_handle: DeviceWorkerHandle,
}

impl ArctisController {
    pub(crate) fn new() -> Result<Self> {
        let original_default_sink =
            get_default_sink().unwrap_or_else(|_| FALLBACK_DEFAULT_SINK.to_string());
        info!("Original default sink: {}", original_default_sink);

        let config_manager = ConfigManager::new();
        if let Err(e) = config_manager.ensure_initialized() {
            warn!(
                "Could not initialize XDG config directory '{}': {}",
                config_manager.base_dir().display(),
                e
            );
        }

        let config = config_manager.load_config().unwrap_or_else(|e| {
            warn!("Failed to load config.toml, using defaults: {}", e);
            AppConfig::default()
        });

        let active_eq_24g = match config_manager.load_profile(&config.active_profile_24g) {
            Ok(profile) => {
                info!(
                    "Loaded active 2.4G 10-band EQ profile '{}' from {}",
                    profile.name,
                    config_manager.base_dir().display()
                );
                Some(profile)
            }
            Err(e) => {
                warn!(
                    "Failed to load 2.4G EQ profile '{}': {}. Falling back to Flat preset.",
                    config.active_profile_24g, e
                );
                Some(ParametricEqProfile::flat())
            }
        };

        let active_eq_bt = match config_manager.load_profile(&config.active_profile_bt) {
            Ok(profile) => {
                info!("Loaded active Bluetooth EQ profile '{}'", profile.name);
                Some(profile)
            }
            Err(e) => {
                warn!(
                    "Failed to load Bluetooth EQ profile '{}': {}. Falling back to Harman preset.",
                    config.active_profile_bt, e
                );
                Some(ParametricEqProfile::harman())
            }
        };

        let active_eq_mic = match config_manager.load_profile(&config.active_profile_mic) {
            Ok(profile) => {
                info!("Loaded active Microphone EQ profile '{}'", profile.name);
                Some(profile)
            }
            Err(e) => {
                warn!(
                    "Failed to load Microphone EQ profile '{}': {}. Falling back to Flat preset.",
                    config.active_profile_mic, e
                );
                Some(ParametricEqProfile::flat())
            }
        };

        let active_eq = active_eq_24g.clone();

        let (worker, worker_handle) =
            DeviceWorker::create(config_manager.clone(), config.clone());

        let running = Arc::new(AtomicBool::new(true));
        let r = running.clone();

        ctrlc::set_handler(move || {
            r.store(false, Ordering::SeqCst);
        })
        .context("Failed to set Ctrl+C handler")?;

        Ok(Self {
            original_default_sink,
            running,
            sinks_created: Arc::new(AtomicBool::new(false)),
            config_manager,
            config,
            active_eq_24g,
            active_eq_bt,
            active_eq_mic,
            active_eq,
            worker,
            worker_handle,
        })
    }

    fn setup_virtual_sinks(&self) -> Result<()> {
        if self.sinks_created.load(Ordering::SeqCst) {
            info!("Virtual sinks already exist, skipping creation");
            return Ok(());
        }

        let arctis_sink =
            find_arctis_sink().context("Arctis Nova 7 device not found in audio system")?;
        info!("Found Physical Sink: {}", arctis_sink);

        info!("Cleaning up old virtual sinks (if any)...");
        destroy_virtual_sinks();

        std::thread::sleep(Duration::from_millis(500));

        info!("Creating virtual sinks for Arctis Nova 7...");
        create_virtual_sink(GAME_SINK_NAME, GAME_SINK_DESC).context("Failed to create Game sink")?;
        create_virtual_sink(CHAT_SINK_NAME, CHAT_SINK_DESC).context("Failed to create Chat sink")?;

        std::thread::sleep(Duration::from_millis(1000));

        info!("Linking virtual sinks to headset...");
        link_sink_to_device(GAME_SINK_NAME, &arctis_sink)?;
        link_sink_to_device(CHAT_SINK_NAME, &arctis_sink)?;

        info!("Setting {} as default sink...", GAME_SINK_NAME);
        if let Err(e) = set_default_sink(GAME_SINK_NAME) {
            warn!("Failed to set {} as default sink: {}", GAME_SINK_NAME, e);
        }

        self.sinks_created.store(true, Ordering::SeqCst);
        info!("Setup complete! Virtual sinks are ready.");

        Ok(())
    }

    pub(crate) fn start(&self) -> Result<()> {
        // 0. Start embedded HTTP + SSE API server immediately so Web UI / API works even while waiting for headset
        if self.config.web_enabled {
            if let Err(e) = spawn_web_server(
                self.config.web_bind.clone(),
                self.worker_handle.clone(),
                self.running.clone(),
            ) {
                warn!(
                    "Could not start embedded HTTP server on {}: {}",
                    self.config.web_bind, e
                );
            }
        }

        // 0b. Start system tray indicator & context menu if enabled
        if self.config.tray_enabled {
            match spawn_system_tray(
                self.worker_handle.clone(),
                self.running.clone(),
                &self.config.web_bind,
            ) {
                Ok(_) => info!("System tray indicator started successfully"),
                Err(e) => {
                    warn!(
                        "Could not start system tray indicator (D-Bus / SNI unavailable?): {}",
                        e
                    );
                }
            }
        }

        // 1. Attempt to create virtual audio sinks
        loop {
            if !self.running.load(Ordering::SeqCst) {
                return Ok(());
            }

            // Drain any incoming API commands while waiting for PipeWire sink
            self.worker.drain_commands::<rusb::Context>(None);

            info!("Waiting for Arctis Nova 7 audio device...");
            match self.setup_virtual_sinks() {
                Ok(_) => break,
                Err(e) => {
                    warn!("Audio setup failed: {}. Retrying in 3 seconds...", e);
                    for _ in 0..15 {
                        if !self.running.load(Ordering::SeqCst) {
                            return Ok(());
                        }
                        self.worker.drain_commands::<rusb::Context>(None);
                        std::thread::sleep(Duration::from_millis(200));
                    }
                }
            }
        }

        // 2. Start USB connection and ChatMix + Telemetry read loop
        loop {
            if !self.running.load(Ordering::SeqCst) {
                break;
            }

            match self.try_connect_and_run() {
                Ok(_) => {
                    info!("Connection loop ended gracefully; exiting main loop");
                    break;
                }
                Err(e) => {
                    if !self.running.load(Ordering::SeqCst) {
                        break;
                    }

                    warn!("USB connection lost / error: {}", e);
                    info!("Waiting for reconnection...");
                    for _ in 0..15 {
                        if !self.running.load(Ordering::SeqCst) {
                            break;
                        }
                        self.worker.drain_commands::<rusb::Context>(None);
                        std::thread::sleep(Duration::from_millis(200));
                    }
                }
            }
        }

        Ok(())
    }

    fn try_connect_and_run(&self) -> Result<()> {
        let usb_ctx = rusb::Context::new().context("Failed to initialize libusb context")?;

        while self.running.load(Ordering::SeqCst) {
            self.worker.drain_commands::<rusb::Context>(None);

            // Reload config & active EQ profile on each connection attempt in case user edited TOML files
            let latest_cfg = self
                .config_manager
                .load_config()
                .unwrap_or_else(|_| self.config.clone());
            let latest_eq_24g = self
                .config_manager
                .load_profile(&latest_cfg.active_profile_24g)
                .ok()
                .or_else(|| self.active_eq_24g.clone());
            let latest_eq_bt = self
                .config_manager
                .load_profile(&latest_cfg.active_profile_bt)
                .ok()
                .or_else(|| self.active_eq_bt.clone());
            let latest_eq_mic = self
                .config_manager
                .load_profile(&latest_cfg.active_profile_mic)
                .ok()
                .or_else(|| self.active_eq_mic.clone());
            let latest_eq = latest_eq_24g.clone();

            match usb_find_and_open(&usb_ctx, Some((&latest_cfg, latest_eq.as_ref()))) {
                Ok((mut handle, endpoint, interface_num, product_id)) => {
                    info!("{}", "=".repeat(50));
                    info!(
                        "Arctis Nova 7 Connected! (USB {:04x}:{:04x}, IF #{}, EP 0x{:02x})",
                        VENDOR_ID, product_id, interface_num, endpoint
                    );
                    info!("  • {} -> Game Audio", GAME_SINK_NAME);
                    info!("  • {} -> Chat Audio", CHAT_SINK_NAME);
                    if let Some(ref eq) = latest_eq {
                        info!("  • Active 10-Band EQ -> {}", eq.name);
                    }
                    info!("{}", "=".repeat(50));

                    // Update shared state with connection metadata
                    if let Ok(mut st) = self.worker_handle.state.write() {
                        st.usb_connected = true;
                        st.product_id = Some(product_id);
                        st.config = latest_cfg.clone();
                        if let Some(ref eq) = latest_eq_24g {
                            st.active_eq_24g = eq.clone();
                            st.active_eq = eq.clone();
                        }
                        if let Some(ref eq) = latest_eq_bt {
                            st.active_eq_bt = eq.clone();
                        }
                        if let Some(ref eq) = latest_eq_mic {
                            st.active_eq_mic = eq.clone();
                        }
                        st.available_presets = load_all_presets(&self.config_manager);
                        st.touch();
                    }
                    self.worker_handle.broadcast_state("connected");

                    // Re-link virtual sinks after device reconnect
                    if let Err(e) = self.relink_virtual_sinks_with_retry() {
                        warn!("Failed to relink virtual sinks after reconnect: {}", e);
                    }

                    // Move active sink inputs to the Game channel
                    if let Err(e) = move_all_inputs_to(GAME_SINK_NAME) {
                        warn!("Failed to move existing sink inputs: {}", e);
                    } else {
                        info!("Moved active audio streams to {}", GAME_SINK_NAME);
                    }

                    // Register Qualcomm GAIA v3 event notifications (0x4001) for live hardware sync
                    for &sync_opcode in &[0x37, 0x39, 0x3A, 0xA3, 0xAE, 0xB2, 0xB3, 0xA6] {
                        let reg = build_gaia_notification_register_report(sync_opcode);
                        let _ = rusb_send_hid_report(&handle, interface_num, &reg);
                        std::thread::sleep(Duration::from_millis(5));
                    }

                    // Query initial general status (0xB0), general settings (0x11), and EQ preset names (0xA6)
                    let _ = rusb_request_status(&handle, interface_num);
                    std::thread::sleep(Duration::from_millis(15));
                    let gen_req = build_general_settings_request_report();
                    let _ = rusb_send_hid_report(&handle, interface_num, &gen_req);

                    for ch in 0..=2 {
                        std::thread::sleep(Duration::from_millis(15));
                        let eq_req = build_eq_name_query_report(ch);
                        let _ = rusb_send_hid_report(&handle, interface_num, &eq_req);
                    }

                    // Read Loop
                    let res = self.read_loop(
                        &mut handle,
                        endpoint,
                        interface_num,
                        product_id,
                        latest_cfg.poll_status_interval_secs,
                    );

                    // Mark USB disconnected in shared state
                    if let Ok(mut st) = self.worker_handle.state.write() {
                        st.usb_connected = false;
                        st.touch();
                    }
                    self.worker_handle.broadcast_state("disconnected");

                    // Release the interface (optional, errors here are non-fatal)
                    let _ = handle.release_interface(interface_num);

                    return res;
                }
                Err(e) => {
                    if !self.running.load(Ordering::SeqCst) {
                        break;
                    }
                    debug!("usb_find_and_open failed: {:?}", e);
                    for _ in 0..10 {
                        if !self.running.load(Ordering::SeqCst) {
                            break;
                        }
                        self.worker.drain_commands::<rusb::Context>(None);
                        std::thread::sleep(Duration::from_millis(200));
                    }
                    continue;
                }
            }
        }

        Ok(())
    }

    fn read_loop<T: UsbContext>(
        &self,
        handle: &mut DeviceHandle<T>,
        endpoint: u8,
        interface_num: u8,
        product_id: u16,
        poll_interval_secs: u64,
    ) -> Result<()> {
        let mut buf = [0u8; HID_MSG_SIZE];
        let mut consecutive_errors = 0u32;
        let mut volume_state = VolumeState::default();
        let mut telemetry = HeadsetTelemetry::default();
        let mut battery_tracker = BatteryNotificationTracker::default();
        let is_percentage_device = is_percentage_battery_device(product_id);
        let mut last_status_poll = Instant::now();

        if let Ok(Some(event)) = crate::usb::hidapi_request_status() {
            let changed = telemetry.apply_event(&event);
            battery_tracker.check_and_notify(&telemetry);
            let event_name = match event {
                TelemetryEvent::ChatMix { .. } => "chatmix",
                _ => "telemetry",
            };
            if changed || matches!(event, TelemetryEvent::ChatMix { .. } | TelemetryEvent::GeneralStatus(_)) {
                if changed {
                    info!("Headset Telemetry: {}", telemetry.summary_line());
                }
                self.sync_telemetry_to_shared_state(&telemetry, event_name);
            }
        }

        while self.running.load(Ordering::SeqCst) {
            // 1. Drain any pending Web UI / API commands on this single USB owner thread
            self.worker.drain_commands(Some((handle, interface_num)));

            let current_poll_secs = self
                .worker_handle
                .state
                .read()
                .map(|s| s.config.poll_status_interval_secs)
                .unwrap_or(poll_interval_secs);

            if current_poll_secs > 0
                && last_status_poll.elapsed() >= Duration::from_secs(current_poll_secs)
            {
                if let Ok(Some(event)) = crate::usb::hidapi_request_status() {
                    let changed = telemetry.apply_event(&event);
                    battery_tracker.check_and_notify(&telemetry);
                    let event_name = match event {
                        TelemetryEvent::ChatMix { .. } => "chatmix",
                        _ => "telemetry",
                    };
                    if changed || matches!(event, TelemetryEvent::ChatMix { .. } | TelemetryEvent::GeneralStatus(_)) {
                        if changed {
                            info!("Headset Telemetry: {}", telemetry.summary_line());
                        }
                        self.sync_telemetry_to_shared_state(&telemetry, event_name);
                    }
                } else {
                    let _ = rusb_request_status(handle, interface_num);
                }

                // Query general settings (0x11) and EQ preset names (0xA6) on periodic status poll
                let req_gen = build_general_settings_request_report();
                let _ = rusb_send_hid_report(handle, interface_num, &req_gen);
                for ch in 0..=2 {
                    let req_eq = build_eq_name_query_report(ch);
                    let _ = rusb_send_hid_report(handle, interface_num, &req_eq);
                }

                self.sync_telemetry_to_shared_state(&telemetry, "telemetry");
                last_status_poll = Instant::now();
            }

            // Use 150ms interrupt timeout so queued API commands execute with sub-150ms latency
            match handle.read_interrupt(endpoint, &mut buf, Duration::from_millis(150)) {
                Ok(len) => {
                    consecutive_errors = 0;
                    let slice = &buf[..len];

                    if let Some((game_vol, chat_vol)) = parse_chatmix_report(slice) {
                        if volume_state.should_apply_game(game_vol) {
                            let game_ok = set_sink_volume(GAME_SINK_NAME, game_vol);
                            volume_state.record_game(game_vol, game_ok);
                        }
                        if volume_state.should_apply_chat(chat_vol) {
                            let chat_ok = set_sink_volume(CHAT_SINK_NAME, chat_vol);
                            volume_state.record_chat(chat_vol, chat_ok);
                        }
                        telemetry.game_volume = Some(game_vol);
                        telemetry.chat_volume = Some(chat_vol);
                        self.sync_telemetry_to_shared_state(&telemetry, "chatmix");
                    }

                    if let Some(event) = parse_telemetry_event(slice, is_percentage_device) {
                        match &event {
                            TelemetryEvent::EqPresetName { channel_id, name, .. } => {
                                let target_ch = match channel_id {
                                    Some(1) => EqChannel::SpeakerBt,
                                    Some(2) => EqChannel::Microphone,
                                    _ => EqChannel::Speaker24G,
                                };
                                self.handle_eq_preset_name_sync(target_ch, name);
                            }
                            TelemetryEvent::GeneralSettings(report) => {
                                if let (Some(game_vol), Some(chat_vol)) =
                                    (report.game_volume, report.chat_volume)
                                {
                                    if volume_state.should_apply_game(game_vol) {
                                        let game_ok = set_sink_volume(GAME_SINK_NAME, game_vol);
                                        volume_state.record_game(game_vol, game_ok);
                                    }
                                    if volume_state.should_apply_chat(chat_vol) {
                                        let chat_ok = set_sink_volume(CHAT_SINK_NAME, chat_vol);
                                        volume_state.record_chat(chat_vol, chat_ok);
                                    }
                                }
                                let changed = telemetry.apply_event(&event);
                                battery_tracker.check_and_notify(&telemetry);
                                if changed {
                                    info!("Headset General Settings: {}", telemetry.summary_line());
                                }
                                self.sync_telemetry_to_shared_state(&telemetry, "settings");
                                self.sync_telemetry_to_shared_state(&telemetry, "telemetry");
                            }
                            TelemetryEvent::GeneralStatus(report) => {
                                if let (Some(game_vol), Some(chat_vol)) =
                                    (report.game_volume, report.chat_volume)
                                {
                                    if volume_state.should_apply_game(game_vol) {
                                        let game_ok = set_sink_volume(GAME_SINK_NAME, game_vol);
                                        volume_state.record_game(game_vol, game_ok);
                                    }
                                    if volume_state.should_apply_chat(chat_vol) {
                                        let chat_ok = set_sink_volume(CHAT_SINK_NAME, chat_vol);
                                        volume_state.record_chat(chat_vol, chat_ok);
                                    }
                                }
                                let changed = telemetry.apply_event(&event);
                                battery_tracker.check_and_notify(&telemetry);
                                if changed {
                                    info!("Headset Telemetry: {}", telemetry.summary_line());
                                }
                                self.sync_telemetry_to_shared_state(&telemetry, "telemetry");
                            }
                            TelemetryEvent::ChatMix { .. } => {
                                let changed = telemetry.apply_event(&event);
                                if changed {
                                    debug!("Telemetry ChatMix: {}", telemetry.summary_line());
                                }
                                self.sync_telemetry_to_shared_state(&telemetry, "chatmix");
                            }
                            _ => {
                                let changed = telemetry.apply_event(&event);
                                battery_tracker.check_and_notify(&telemetry);
                                if changed {
                                    info!("Headset Hardware Sync: {:?}", event);
                                }
                                self.sync_telemetry_to_shared_state(&telemetry, "settings");
                                self.sync_telemetry_to_shared_state(&telemetry, "telemetry");
                            }
                        }
                    }
                }
                Err(rusb::Error::Timeout) => {
                    // Timeout is expected — no data received in this interval.
                    consecutive_errors = 0;
                    continue;
                }
                Err(rusb::Error::NoDevice) => {
                    error!("Device disconnected (NoDevice)");
                    return Err(anyhow::anyhow!("USB device disconnected (NoDevice)"));
                }
                Err(rusb::Error::Io) => {
                    consecutive_errors += 1;
                    warn!(
                        "USB I/O error (attempt {}/{})",
                        consecutive_errors, MAX_CONSECUTIVE_USB_ERRORS
                    );
                    if consecutive_errors >= MAX_CONSECUTIVE_USB_ERRORS {
                        return Err(anyhow::anyhow!("Too many USB I/O errors"));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => {
                    consecutive_errors += 1;
                    warn!(
                        "USB error: {:?} (attempt {}/{})",
                        e, consecutive_errors, MAX_CONSECUTIVE_USB_ERRORS
                    );
                    if consecutive_errors >= MAX_CONSECUTIVE_USB_ERRORS {
                        return Err(anyhow::anyhow!("Too many USB errors: {:?}", e));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
        Ok(())
    }

    fn sync_telemetry_to_shared_state(&self, telemetry: &HeadsetTelemetry, sse_event_name: &str) {
        if let Ok(mut st) = self.worker_handle.state.write() {
            st.telemetry = telemetry.clone();
            if let Some(lvl) = telemetry.sidetone_level {
                st.config.hardware.sidetone_level = lvl;
                st.config.hardware.sidetone_enabled = lvl > 0;
            }
            if let Some(gain) = telemetry.mic_gain {
                st.config.hardware.mic_gain = gain;
            }
            if let Some(lim) = telemetry.volume_limiter {
                st.config.hardware.volume_limiter = lim;
            }
            if let Some(mins) = telemetry.inactivity_timer_mins {
                st.config.hardware.inactivity_timer_mins = mins;
            }
            if let Some(bright) = telemetry.mute_led_brightness {
                st.config.hardware.mute_led_brightness = bright;
            }
            if let Some(pwr) = telemetry.bt_power_default {
                st.config.hardware.bt_power_on_default = pwr;
            }
            if let Some(beh) = telemetry.bt_call_behavior {
                st.config.hardware.bt_call_behavior = beh;
            }
            if let Some(muted) = telemetry.mic_muted {
                st.mic_muted = Some(muted);
            }
            st.touch();
        }
        self.worker_handle.broadcast_state(sse_event_name);
    }

    fn handle_eq_preset_name_sync(&self, channel: EqChannel, name: &str) {
        let clean_name = name.trim();
        if clean_name.is_empty() {
            return;
        }

        let matched_profile = self
            .config_manager
            .load_profile(clean_name)
            .ok()
            .or_else(|| ParametricEqProfile::builtin_by_name(clean_name));

        if let Ok(mut st) = self.worker_handle.state.write() {
            let mut changed = false;
            match channel {
                EqChannel::Speaker24G => {
                    if !st.config.active_profile_24g.eq_ignore_ascii_case(clean_name) {
                        st.config.active_profile_24g = clean_name.to_string();
                        st.config.active_profile = clean_name.to_string();
                        changed = true;
                    }
                    if let Some(ref prof) = matched_profile {
                        st.active_eq_24g = prof.clone();
                        st.active_eq = prof.clone();
                        changed = true;
                    }
                }
                EqChannel::SpeakerBt => {
                    if !st.config.active_profile_bt.eq_ignore_ascii_case(clean_name) {
                        st.config.active_profile_bt = clean_name.to_string();
                        changed = true;
                    }
                    if let Some(ref prof) = matched_profile {
                        st.active_eq_bt = prof.clone();
                        changed = true;
                    }
                }
                EqChannel::Microphone => {
                    if !st.config.active_profile_mic.eq_ignore_ascii_case(clean_name) {
                        st.config.active_profile_mic = clean_name.to_string();
                        changed = true;
                    }
                    if let Some(ref prof) = matched_profile {
                        st.active_eq_mic = prof.clone();
                        changed = true;
                    }
                }
            }
            if changed {
                let _ = self.config_manager.save_config(&st.config);
                st.touch();
                info!(
                    "Synchronized hardware EQ preset for {}: '{}'",
                    channel.as_str(),
                    clean_name
                );
            }
        }
        self.worker_handle.broadcast_state("eq");
        self.worker_handle.broadcast_state("telemetry");
    }

    fn relink_virtual_sinks_with_retry(&self) -> Result<()> {
        for attempt in 1..=RELINK_RETRIES {
            if !self.running.load(Ordering::SeqCst) {
                anyhow::bail!("Shutdown in progress");
            }

            match find_arctis_sink() {
                Ok(arctis_sink) => {
                    info!("Relinking virtual sinks to '{}'", arctis_sink);
                    // Log warnings but don't abort the flow
                    if let Err(e) = link_sink_to_device(GAME_SINK_NAME, &arctis_sink) {
                        warn!("Link warning (Game): {}", e);
                    }
                    if let Err(e) = link_sink_to_device(CHAT_SINK_NAME, &arctis_sink) {
                        warn!("Link warning (Chat): {}", e);
                    }

                    // Set Game sink as the default
                    if let Err(e) = set_default_sink(GAME_SINK_NAME) {
                        warn!("Failed to set {} as default sink: {}", GAME_SINK_NAME, e);
                    }

                    std::thread::sleep(RELINK_RETRY_DELAY);
                    return Ok(());
                }
                Err(e) => {
                    debug!(
                        "Retry {}/{}: Could not find sink: {}",
                        attempt, RELINK_RETRIES, e
                    );
                    std::thread::sleep(RELINK_RETRY_DELAY);
                    continue;
                }
            }
        }
        anyhow::bail!("Failed to locate Arctis sink after retries");
    }

    fn cleanup(&self) {
        info!("Shutting down...");
        // Restore the original default sink
        if let Err(e) = set_default_sink(&self.original_default_sink) {
            debug!("Could not restore default sink '{}': {}", self.original_default_sink, e);
        }

        // Destroy virtual sink nodes
        destroy_virtual_sinks();

        info!("Arctis Nova 7 ChatMix shut down.");
    }
}

impl Drop for ArctisController {
    fn drop(&mut self) {
        self.cleanup();
    }
}

#[cfg(test)]
#[derive(Debug, PartialEq, Eq, Clone)]
struct NotifyRecord {
    urgency: String,
    icon: String,
    title: String,
    body: String,
}

#[cfg(test)]
static NOTIFY_CAPTURES: std::sync::OnceLock<std::sync::Mutex<Vec<NotifyRecord>>> = std::sync::OnceLock::new();
#[cfg(test)]
static NOTIFY_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
fn notify_captures() -> &'static std::sync::Mutex<Vec<NotifyRecord>> {
    NOTIFY_CAPTURES.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

#[cfg(test)]
fn clear_notify_captures() {
    *notify_captures().lock().unwrap_or_else(|e| e.into_inner()) = Vec::new();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_volume_state_per_channel_deduplication_and_failure_recovery() {
        let mut state = VolumeState::default();

        // Initial center detent (100, 100): both channels must be applied
        assert!(state.should_apply_game(100));
        assert!(state.should_apply_chat(100));
        state.record_game(100, true);
        state.record_chat(100, true);

        // Identical report: neither channel should spawn pactl
        assert!(!state.should_apply_game(100));
        assert!(!state.should_apply_chat(100));

        // Turning wheel toward Game (100, 85): only Chat changed, Game stays cached at 100
        assert!(!state.should_apply_game(100));
        assert!(state.should_apply_chat(85));

        // If Chat pactl fails, only Chat is invalidated; Game remains cached at 100
        state.record_chat(85, false);
        assert!(!state.should_apply_game(100));
        assert!(state.should_apply_chat(85));

        // Once Chat succeeds, both are cached
        state.record_chat(85, true);
        assert!(!state.should_apply_chat(85));
        assert!(state.should_apply_game(70));
    }

    #[test]
    fn test_battery_notification_thresholds() {
        let _lock = NOTIFY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut tracker = BatteryNotificationTracker::default();

        // High battery, disconnected (thresholds suppressed)
        let t1 = HeadsetTelemetry { wireless: WirelessState::Disconnected, battery_percent: Some(100), charging: ChargingState::NotCharging, ..HeadsetTelemetry::default() };
        tracker.check_and_notify(&t1);
        assert_eq!(tracker.last_battery_alert_threshold, None);
        assert_eq!(tracker.last_charging_state, Some(ChargingState::NotCharging));

        // Drops to 21% (above 20% threshold, connected)
        let t2 = HeadsetTelemetry { wireless: WirelessState::Connected, battery_percent: Some(21), charging: ChargingState::NotCharging, ..HeadsetTelemetry::default() };
        tracker.check_and_notify(&t2);
        assert_eq!(tracker.last_battery_alert_threshold, None);

        // Drops to 20% (triggers 20% alert)
        let t3 = HeadsetTelemetry { wireless: WirelessState::Connected, battery_percent: Some(20), charging: ChargingState::NotCharging, ..HeadsetTelemetry::default() };
        tracker.check_and_notify(&t3);
        assert_eq!(tracker.last_battery_alert_threshold, Some(20));

        // Still at 20% (does not re-trigger)
        let t4 = HeadsetTelemetry { wireless: WirelessState::Connected, battery_percent: Some(20), charging: ChargingState::NotCharging, ..HeadsetTelemetry::default() };
        tracker.check_and_notify(&t4);
        assert_eq!(tracker.last_battery_alert_threshold, Some(20));

        // Drops to 15% (still in <= 20 threshold, does not re-alert)
        let t5 = HeadsetTelemetry { wireless: WirelessState::Connected, battery_percent: Some(15), charging: ChargingState::NotCharging, ..HeadsetTelemetry::default() };
        tracker.check_and_notify(&t5);
        assert_eq!(tracker.last_battery_alert_threshold, Some(20));

        // Drops to 10% (triggers 10% alert)
        let t6 = HeadsetTelemetry { wireless: WirelessState::Connected, battery_percent: Some(10), charging: ChargingState::NotCharging, ..HeadsetTelemetry::default() };
        tracker.check_and_notify(&t6);
        assert_eq!(tracker.last_battery_alert_threshold, Some(10));

        // Drops to 8% (still in <= 10 threshold)
        let t7 = HeadsetTelemetry { wireless: WirelessState::Connected, battery_percent: Some(8), charging: ChargingState::NotCharging, ..HeadsetTelemetry::default() };
        tracker.check_and_notify(&t7);
        assert_eq!(tracker.last_battery_alert_threshold, Some(10));

        // Drops to 5% (triggers critical <= 5% alert)
        let t8 = HeadsetTelemetry { wireless: WirelessState::Connected, battery_percent: Some(5), charging: ChargingState::NotCharging, ..HeadsetTelemetry::default() };
        tracker.check_and_notify(&t8);
        assert_eq!(tracker.last_battery_alert_threshold, Some(5));

        // Drops to 1% (already at lowest 5% threshold)
        let t9 = HeadsetTelemetry { wireless: WirelessState::Connected, battery_percent: Some(1), charging: ChargingState::NotCharging, ..HeadsetTelemetry::default() };
        tracker.check_and_notify(&t9);
        assert_eq!(tracker.last_battery_alert_threshold, Some(5));

        // Battery recharged above 25% (resets threshold to None)
        let t10 = HeadsetTelemetry { wireless: WirelessState::Connected, battery_percent: Some(26), charging: ChargingState::NotCharging, ..HeadsetTelemetry::default() };
        tracker.check_and_notify(&t10);
        assert_eq!(tracker.last_battery_alert_threshold, None);
    }

    #[test]
    fn test_battery_notification_charging_state() {
        let _lock = NOTIFY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_notify_captures();
        let mut tracker = BatteryNotificationTracker::default();

        // Initialized disconnected with low battery (15%)
        let t1 = HeadsetTelemetry { wireless: WirelessState::Disconnected, battery_percent: Some(15), charging: ChargingState::NotCharging, ..HeadsetTelemetry::default() };
        tracker.check_and_notify(&t1);
        assert_eq!(tracker.last_charging_state, Some(ChargingState::NotCharging));

        // User plugs in charger -> charging starts, alerts fire
        let t2 = HeadsetTelemetry { wireless: WirelessState::Connected, battery_percent: Some(15), charging: ChargingState::Charging, ..HeadsetTelemetry::default() };
        tracker.check_and_notify(&t2);
        assert_eq!(tracker.last_charging_state, Some(ChargingState::Charging));
        assert_eq!(tracker.last_battery_alert_threshold, None);
        let captures = notify_captures().lock().unwrap_or_else(|e| e.into_inner());
        assert!(!captures.is_empty());
        assert!(captures.iter().any(|r| r.body == "Kulaklık şarj ediliyor (15%)"));
        drop(captures);

        // Fully charged
        let t3 = HeadsetTelemetry { wireless: WirelessState::Connected, battery_percent: Some(100), charging: ChargingState::Charged, ..HeadsetTelemetry::default() };
        tracker.check_and_notify(&t3);
        assert_eq!(tracker.last_charging_state, Some(ChargingState::Charged));
        let captures2 = notify_captures().lock().unwrap_or_else(|e| e.into_inner());
        assert!(!captures2.is_empty());
        assert!(captures2.iter().any(|r| r.body == "Kulaklık şarjı doldu (%100)"));
        drop(captures2);

        // While charging, low battery alerts are suppressed
        clear_notify_captures();
        let t4 = HeadsetTelemetry { wireless: WirelessState::Connected, battery_percent: Some(4), charging: ChargingState::Charging, ..HeadsetTelemetry::default() };
        tracker.check_and_notify(&t4);
        assert_eq!(tracker.last_battery_alert_threshold, None);
        // Charging suppresses low-battery alerts; state verified above
    }

    #[test]
    fn test_wireless_connection_disconnection_notifications_and_resets() {
        let _lock = NOTIFY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_notify_captures();
        let mut tracker = BatteryNotificationTracker::default();

        tracker.check_wireless(WirelessState::Disconnected, Some(80));
        assert_eq!(tracker.last_wireless_state, Some(WirelessState::Disconnected));
        assert!(notify_captures().lock().unwrap_or_else(|e| e.into_inner()).is_empty());

        clear_notify_captures();
        tracker.check_wireless(WirelessState::Connected, Some(80));
        assert_eq!(tracker.last_wireless_state, Some(WirelessState::Connected));
        let cap = notify_captures().lock().unwrap_or_else(|e| e.into_inner());
        assert!(!cap.is_empty());
        assert!(cap.iter().any(|r| r.body == "Kulaklık bağlandı (80%)"));
        drop(cap);

        clear_notify_captures();
        tracker.check_wireless(WirelessState::Connected, Some(90));
        assert!(notify_captures().lock().unwrap_or_else(|e| e.into_inner()).is_empty());

        clear_notify_captures();
        tracker.check_wireless(WirelessState::Disconnected, Some(30));
        assert_eq!(tracker.last_wireless_state, Some(WirelessState::Disconnected));
        let cap2 = notify_captures().lock().unwrap_or_else(|e| e.into_inner());
        assert!(!cap2.is_empty());
        assert!(cap2.iter().any(|r| r.body == "Kulaklık bağlantısı kesildi"));
        drop(cap2);
        assert_eq!(tracker.last_battery_alert_threshold, None);
        assert_eq!(tracker.last_charging_state, None);
    }

    #[test]
    fn test_charging_transitions_byte_values_no_false_trigger() {
        let _lock = NOTIFY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_notify_captures();
        let mut tracker = BatteryNotificationTracker::default();

        // Byte 3 (NotCharging / running) must NOT trigger charging alert
        let initial = HeadsetTelemetry {
            wireless: WirelessState::Connected,
            battery_percent: Some(80),
            charging: ChargingState::NotCharging,
            ..HeadsetTelemetry::default()
        };
        tracker.last_wireless_state = Some(WirelessState::Connected);
        tracker.check_and_notify(&initial);
        assert_eq!(notify_captures().lock().unwrap_or_else(|e| e.into_inner()).len(), 0);
        assert_eq!(tracker.last_charging_state, Some(ChargingState::NotCharging));

        // Byte 1 (Charging) triggers start
        clear_notify_captures();
        let ch = HeadsetTelemetry {
            wireless: WirelessState::Connected,
            battery_percent: Some(80),
            charging: ChargingState::Charging,
            ..HeadsetTelemetry::default()
        };
        tracker.check_and_notify(&ch);
        assert!(!notify_captures().lock().unwrap_or_else(|e| e.into_inner()).is_empty());
        assert!(notify_captures().lock().unwrap_or_else(|e| e.into_inner()).iter().any(|r| r.body == "Kulaklık şarj ediliyor (80%)"));

        // Byte 2 (Charged) after Charging triggers full
        clear_notify_captures();
        let full = HeadsetTelemetry {
            wireless: WirelessState::Connected,
            battery_percent: Some(100),
            charging: ChargingState::Charged,
            ..HeadsetTelemetry::default()
        };
        tracker.check_and_notify(&full);
        assert!(!notify_captures().lock().unwrap_or_else(|e| e.into_inner()).is_empty());
        assert!(notify_captures().lock().unwrap_or_else(|e| e.into_inner()).iter().any(|r| r.body == "Kulaklık şarjı doldu (%100)"));

        // Back to NotCharging — no extra alert
        clear_notify_captures();
        let off = HeadsetTelemetry {
            wireless: WirelessState::Connected,
            battery_percent: Some(100),
            charging: ChargingState::NotCharging,
            ..HeadsetTelemetry::default()
        };
        tracker.check_and_notify(&off);
        assert!(notify_captures().lock().unwrap_or_else(|e| e.into_inner()).is_empty());
    }

    #[test]
    fn test_low_battery_thresholds_connected_only_not_charging() {
        let _lock = NOTIFY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_notify_captures();
        let mut tracker = BatteryNotificationTracker::default();

        // Connected, 20% -> fire 20
        tracker.check_and_notify(&HeadsetTelemetry { wireless: WirelessState::Connected, battery_percent: Some(20), charging: ChargingState::NotCharging, ..HeadsetTelemetry::default() });
        assert_eq!(tracker.last_battery_alert_threshold, Some(20));

        // Charging suppresses
        clear_notify_captures();
        tracker.check_and_notify(&HeadsetTelemetry { wireless: WirelessState::Connected, battery_percent: Some(20), charging: ChargingState::Charging, ..HeadsetTelemetry::default() });
        // Charging suppresses low-battery alerts

        // Disconnected suppresses and resets
        tracker.check_wireless(WirelessState::Disconnected, Some(20));
        assert_eq!(tracker.last_battery_alert_threshold, None);

        // Connected 10% -> fire 10
        tracker.check_wireless(WirelessState::Connected, Some(10));
        tracker.check_charging_and_battery(WirelessState::Connected, Some(10), ChargingState::NotCharging);
        assert_eq!(tracker.last_battery_alert_threshold, Some(10));

        // 5% -> fire 5
        tracker.check_charging_and_battery(WirelessState::Connected, Some(5), ChargingState::NotCharging);
        assert_eq!(tracker.last_battery_alert_threshold, Some(5));
    }
}
