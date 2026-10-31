use crate::audio::{
    create_virtual_sink, destroy_virtual_sinks, find_arctis_sink, get_default_sink,
    link_sink_to_device, move_all_inputs_to, set_default_sink, set_sink_volume,
};
use crate::config::{AppConfig, ConfigManager};
use crate::device::{
    is_percentage_battery_device, parse_chatmix_report, CHAT_SINK_DESC, CHAT_SINK_NAME,
    FALLBACK_DEFAULT_SINK, GAME_SINK_DESC, GAME_SINK_NAME, HID_MSG_SIZE, VENDOR_ID,
};
use crate::eq::ParametricEqProfile;
use crate::state::{load_all_presets, DeviceWorker, DeviceWorkerHandle};
use crate::telemetry::{parse_telemetry_event, HeadsetTelemetry, TelemetryEvent};
use crate::usb::{rusb_request_status, usb_find_and_open};
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

pub(crate) struct ArctisController {
    original_default_sink: String,
    running: Arc<AtomicBool>,
    sinks_created: Arc<AtomicBool>,
    config_manager: ConfigManager,
    config: AppConfig,
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

        let active_eq = match config_manager.load_profile(&config.active_profile) {
            Ok(profile) => {
                info!(
                    "Loaded active 10-band EQ profile '{}' from {}",
                    profile.name,
                    config_manager.base_dir().display()
                );
                Some(profile)
            }
            Err(e) => {
                warn!(
                    "Failed to load EQ profile '{}': {}. Falling back to Harman preset.",
                    config.active_profile, e
                );
                Some(ParametricEqProfile::harman())
            }
        };

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
            let latest_eq = self
                .config_manager
                .load_profile(&latest_cfg.active_profile)
                .ok()
                .or_else(|| self.active_eq.clone());

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
                        if let Some(ref eq) = latest_eq {
                            st.active_eq = eq.clone();
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

                    // Query initial general status (0xB0)
                    let _ = rusb_request_status(&handle, interface_num);

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
        let is_percentage_device = is_percentage_battery_device(product_id);
        let mut last_status_poll = Instant::now();

        if let Ok(Some(event)) = crate::usb::hidapi_request_status() {
            let changed = telemetry.apply_event(&event);
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
                    // Broadcast heartbeat on explicit status poll
                    self.sync_telemetry_to_shared_state(&telemetry, "telemetry");
                }
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
                        // Also apply ChatMix volumes if included in a 0xB0 GeneralStatus snapshot
                        if let TelemetryEvent::GeneralStatus(report) = event {
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
                        }

                        let changed = telemetry.apply_event(&event);
                        let event_name = match event {
                            TelemetryEvent::ChatMix { .. } => "chatmix",
                            TelemetryEvent::GeneralStatus(_) => "telemetry",
                            _ => "telemetry",
                        };
                        if changed || matches!(event, TelemetryEvent::ChatMix { .. } | TelemetryEvent::GeneralStatus(_)) {
                            if changed {
                                match event {
                                    TelemetryEvent::ChatMix { .. } => {
                                        debug!("Telemetry ChatMix: {}", telemetry.summary_line());
                                    }
                                    _ => {
                                        info!("Headset Telemetry: {}", telemetry.summary_line());
                                    }
                                }
                            }
                            self.sync_telemetry_to_shared_state(&telemetry, event_name);
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
            st.touch();
        }
        self.worker_handle.broadcast_state(sse_event_name);
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
}
