// Stage 2: Thread-Safe USB Device Worker, Shared HeadsetState & SSE Event Bus.
//
// Prevents race conditions on the USB HID interface between continuous ChatMix/telemetry
// polling (`read_loop`) and incoming Web UI / API commands (EQ, sidetone, mic gain,
// volume limiter, inactivity timer, mute LED brightness, BT settings, flash save, status poll).

use crate::config::{AppConfig, ConfigManager, HardwareConfig};
use crate::device::{
    build_bt_call_behavior_report, build_bt_power_default_report, build_inactivity_timer_report,
    build_mic_gain_report, build_mute_led_brightness_report, build_save_to_flash_report,
    build_sidetone_level_report, build_sidetone_report, build_status_request_report,
    build_volume_limiter_report, sidetone_bucket, BtCallBehavior, HID_MSG_SIZE,
    MAX_INACTIVITY_MINUTES, MAX_MIC_GAIN_LEVEL, MAX_MUTE_LED_BRIGHTNESS, MAX_SIDETONE_LEVEL,
    MAX_VOLUME_PERCENT,
};
use crate::eq::ParametricEqProfile;
use crate::telemetry::HeadsetTelemetry;
use crate::usb::{
    hidapi_request_status, hidapi_send_report, hidapi_send_reports, rusb_send_hid_report,
};
use anyhow::Result;
use log::{debug, info, warn};
use rusb::{DeviceHandle, UsbContext};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const USB_INTER_REPORT_DELAY: Duration = Duration::from_millis(15);

/// Commands sent from the HTTP API or CLI to the single-threaded USB `DeviceWorker`.
#[derive(Debug)]
pub enum DeviceCommand {
    /// Apply a 10-band Parametric EQ profile (`0x33`) and optionally persist it as active.
    ApplyEq {
        profile: ParametricEqProfile,
        save_profile_to_disk: bool,
        reply: Sender<Result<()>>,
    },
    /// Activate an existing EQ preset/profile by name (`0x33`).
    ActivatePreset {
        name: String,
        reply: Sender<Result<ParametricEqProfile>>,
    },
    /// Set hardware sidetone by level (`0..=3`, `0x39`) or percentage (`0..=100`).
    SetSidetone {
        enabled: bool,
        level: Option<u8>,
        percent: Option<u8>,
        reply: Sender<Result<u8>>,
    },
    /// Update one or more hardware settings (`0x37`, `0x39`, `0x3A`, `0xA3`, `0xAE`, `0xB2`, `0xB3`).
    UpdateSettings {
        patch: HardwareSettingsPatch,
        reply: Sender<Result<HardwareConfig>>,
    },
    /// Persist active DSP and hardware settings to headset onboard flash (`0x09`).
    SaveToFlash {
        reply: Sender<Result<()>>,
    },
    /// Trigger an immediate `0xB0` status/telemetry poll.
    RequestStatus {
        reply: Sender<Result<()>>,
    },
}

/// Partial update payload for `/api/settings`.
#[derive(Debug, Clone, Default)]
pub struct HardwareSettingsPatch {
    pub sidetone_enabled: Option<bool>,
    pub sidetone_level: Option<u8>,
    pub mic_gain: Option<u8>,
    pub volume_limiter: Option<bool>,
    pub inactivity_timer_mins: Option<u8>,
    pub mute_led_brightness: Option<u8>,
    pub bt_power_on_default: Option<bool>,
    pub bt_call_behavior: Option<BtCallBehavior>,
    pub auto_apply_on_connect: Option<bool>,
    pub save_to_flash: Option<bool>,
    pub poll_status_interval_secs: Option<u64>,
}

/// Complete snapshot of the headset, active EQ, and configuration state shared with the Web Server.
#[derive(Debug, Clone)]
pub struct HeadsetState {
    pub usb_connected: bool,
    pub product_id: Option<u16>,
    pub telemetry: HeadsetTelemetry,
    pub config: AppConfig,
    pub active_eq: ParametricEqProfile,
    pub available_presets: Vec<ParametricEqProfile>,
    pub last_updated_ms: u64,
}

impl HeadsetState {
    pub fn new(
        config: AppConfig,
        active_eq: ParametricEqProfile,
        available_presets: Vec<ParametricEqProfile>,
    ) -> Self {
        Self {
            usb_connected: false,
            product_id: None,
            telemetry: HeadsetTelemetry::default(),
            config,
            active_eq,
            available_presets,
            last_updated_ms: now_epoch_ms(),
        }
    }

    pub fn touch(&mut self) {
        self.last_updated_ms = now_epoch_ms();
    }

    /// Serializes the full status snapshot to JSON for `GET /api/status` and SSE `status` events.
    pub fn to_status_json(&self) -> String {
        let wireless_str = match self.telemetry.wireless {
            crate::telemetry::WirelessState::Unpaired => "unpaired",
            crate::telemetry::WirelessState::Searching => "searching",
            crate::telemetry::WirelessState::Disconnected => "disconnected",
            crate::telemetry::WirelessState::Connected => "connected",
            crate::telemetry::WirelessState::Unknown(_) => "unknown",
        };
        let wireless_connected = self.telemetry.wireless.is_connected();
        let charging_str = match self.telemetry.charging {
            crate::telemetry::ChargingState::NotCharging => "not_charging",
            crate::telemetry::ChargingState::Charging => "charging",
            crate::telemetry::ChargingState::Charged => "charged",
            crate::telemetry::ChargingState::Unknown(_) => "unknown",
        };
        let battery_pct = self
            .telemetry
            .battery_percent
            .map(|v| v.to_string())
            .unwrap_or_else(|| "null".to_string());
        let battery_raw = self
            .telemetry
            .battery_raw
            .map(|v| v.to_string())
            .unwrap_or_else(|| "null".to_string());
        let mic_muted = self
            .telemetry
            .mic_muted
            .map(|v| v.to_string())
            .unwrap_or_else(|| "null".to_string());
        let game_vol = self.telemetry.game_volume.unwrap_or(100);
        let chat_vol = self.telemetry.chat_volume.unwrap_or(100);
        let pid_str = self
            .product_id
            .map(|pid| format!("\"0x{:04x}\"", pid))
            .unwrap_or_else(|| "null".to_string());

        format!(
            concat!(
                "{{",
                "\"usb_connected\":{},",
                "\"product_id\":{},",
                "\"wireless_state\":\"{}\",",
                "\"wireless_connected\":{},",
                "\"battery_percent\":{},",
                "\"battery_raw\":{},",
                "\"charging_state\":\"{}\",",
                "\"mic_muted\":{},",
                "\"chatmix\":{{\"game\":{},\"chat\":{}}},",
                "\"active_profile\":\"{}\",",
                "\"eq\":{},",
                "\"settings\":{},",
                "\"last_updated_ms\":{}",
                "}}"
            ),
            self.usb_connected,
            pid_str,
            wireless_str,
            wireless_connected,
            battery_pct,
            battery_raw,
            charging_str,
            mic_muted,
            game_vol,
            chat_vol,
            escape_json(&self.config.active_profile),
            eq_profile_to_json(&self.active_eq),
            config_to_settings_json(&self.config),
            self.last_updated_ms
        )
    }
}

pub fn eq_profile_to_json(profile: &ParametricEqProfile) -> String {
    let mut bands_json = String::with_capacity(512);
    bands_json.push('[');
    for (idx, band) in profile.bands.iter().enumerate() {
        if idx > 0 {
            bands_json.push(',');
        }
        bands_json.push_str(&format!(
            "{{\"frequency_hz\":{},\"gain_db\":{:.2},\"q_factor\":{:.3},\"filter_type\":\"{}\"}}",
            band.frequency_hz,
            band.gain_db,
            band.q_factor,
            band.filter_type.as_str()
        ));
    }
    bands_json.push(']');

    let gains_csv = profile
        .gains_db()
        .iter()
        .map(|g| format!("{:.2}", g))
        .collect::<Vec<_>>()
        .join(",");

    format!(
        "{{\"name\":\"{}\",\"description\":\"{}\",\"gains_db\":[{}],\"bands\":{}}}",
        escape_json(&profile.name),
        escape_json(&profile.description),
        gains_csv,
        bands_json
    )
}

pub fn config_to_settings_json(config: &AppConfig) -> String {
    let hw = &config.hardware;
    format!(
        concat!(
            "{{",
            "\"sidetone_enabled\":{},",
            "\"sidetone_level\":{},",
            "\"mic_gain\":{},",
            "\"volume_limiter\":{},",
            "\"inactivity_timer_mins\":{},",
            "\"auto_off_minutes\":{},",
            "\"mute_led_brightness\":{},",
            "\"mic_mute_led\":{},",
            "\"bt_power_on_default\":{},",
            "\"bt_auto_power_on\":{},",
            "\"bt_call_behavior\":\"{}\",",
            "\"bt_call_attenuation\":\"{}\",",
            "\"auto_apply_on_connect\":{},",
            "\"save_to_flash\":{},",
            "\"poll_status_interval_secs\":{},",
            "\"web_bind\":\"{}\"",
            "}}"
        ),
        hw.sidetone_enabled,
        hw.sidetone_level,
        hw.mic_gain,
        hw.volume_limiter,
        hw.inactivity_timer_mins,
        hw.inactivity_timer_mins,
        hw.mute_led_brightness,
        hw.mute_led_brightness,
        hw.bt_power_on_default,
        hw.bt_power_on_default,
        hw.bt_call_behavior.as_str(),
        hw.bt_call_behavior.as_str(),
        config.auto_apply_on_connect,
        config.save_to_flash,
        config.poll_status_interval_secs,
        escape_json(&config.web_bind)
    )
}

pub fn escape_json(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Multi-subscriber Server-Sent Events (SSE) broadcaster for `/api/events`.
#[derive(Default)]
pub struct SseBroadcaster {
    subscribers: Mutex<Vec<Sender<String>>>,
}

impl SseBroadcaster {
    pub fn new() -> Self {
        Self {
            subscribers: Mutex::new(Vec::new()),
        }
    }

    pub fn subscribe(&self) -> Receiver<String> {
        let (tx, rx) = mpsc::channel();
        if let Ok(mut subs) = self.subscribers.lock() {
            subs.push(tx);
        }
        rx
    }

    /// Broadcasts an SSE event frame (`event: <name>\ndata: <json>\n\n`) to all connected web clients,
    /// pruning any disconnected receivers automatically.
    pub fn broadcast(&self, event_name: &str, json_data: &str) {
        let payload = format!("event: {}\ndata: {}\n\n", event_name, json_data);
        if let Ok(mut subs) = self.subscribers.lock() {
            subs.retain(|tx| tx.send(payload.clone()).is_ok());
        }
    }
}

/// Thread-safe handle shared by the HTTP server and controller to read state, subscribe to SSE,
/// and enqueue serialized USB hardware commands.
#[derive(Clone)]
pub struct DeviceWorkerHandle {
    cmd_tx: Sender<DeviceCommand>,
    pub state: Arc<RwLock<HeadsetState>>,
    pub broadcaster: Arc<SseBroadcaster>,
    pub config_manager: ConfigManager,
}

impl DeviceWorkerHandle {
    pub fn send_command(&self, cmd: DeviceCommand) -> Result<()> {
        self.cmd_tx
            .send(cmd)
            .map_err(|_| anyhow::anyhow!("DeviceWorker command channel is closed"))
    }

    pub fn broadcast_state(&self, event_name: &str) {
        if let Ok(guard) = self.state.read() {
            let json = guard.to_status_json();
            self.broadcaster.broadcast(event_name, &json);
        }
    }
}

/// Single-owner worker side that drains `DeviceCommand` requests synchronously inside the USB loop
/// (or while waiting for USB connection) so USB/hidapi access never races.
pub struct DeviceWorker {
    cmd_rx: Receiver<DeviceCommand>,
    handle: DeviceWorkerHandle,
}

impl DeviceWorker {
    pub fn create(config_manager: ConfigManager, initial_config: AppConfig) -> (Self, DeviceWorkerHandle) {
        let active_eq = config_manager
            .load_profile(&initial_config.active_profile)
            .unwrap_or_else(|_| ParametricEqProfile::harman());
        let presets = load_all_presets(&config_manager);
        let state = Arc::new(RwLock::new(HeadsetState::new(
            initial_config,
            active_eq,
            presets,
        )));
        let broadcaster = Arc::new(SseBroadcaster::new());
        let (cmd_tx, cmd_rx) = mpsc::channel();

        let handle = DeviceWorkerHandle {
            cmd_tx,
            state,
            broadcaster,
            config_manager,
        };
        let worker = Self {
            cmd_rx,
            handle: handle.clone(),
        };
        (worker, handle)
    }

    #[allow(dead_code)]
    pub fn handle(&self) -> &DeviceWorkerHandle {
        &self.handle
    }

    /// Drains all pending `DeviceCommand` items from the queue.
    /// If `usb_handle` is `Some((handle, interface_num))`, commands are transmitted over the active USB
    /// connection (trying `hidapi` first and falling back to `rusb_send_hid_report` on the claimed handle).
    pub fn drain_commands<T: UsbContext>(&self, mut usb_ctx: Option<(&DeviceHandle<T>, u8)>) {
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            self.execute_command(cmd, usb_ctx.as_mut().map(|(h, i)| (*h, *i)));
        }
    }

    fn execute_command<T: UsbContext>(
        &self,
        cmd: DeviceCommand,
        usb_handle: Option<(&DeviceHandle<T>, u8)>,
    ) {
        match cmd {
            DeviceCommand::ApplyEq {
                profile,
                save_profile_to_disk,
                reply,
            } => {
                let res = (|| -> Result<()> {
                    if save_profile_to_disk {
                        self.handle.config_manager.save_profile(&profile)?;
                    }

                    let report = profile.build_hid_report();
                    if let Ok(mut st) = self.handle.state.write() {
                        st.config.active_profile = profile.name.clone();
                        st.active_eq = profile;
                        let _ = self.handle.config_manager.save_config(&st.config);
                        st.available_presets = load_all_presets(&self.handle.config_manager);
                        st.touch();
                    }

                    let save_to_flash = self.handle.state.read().map(|s| s.config.save_to_flash).unwrap_or(false);
                    send_report_if_connected(&report, usb_handle, save_to_flash)?;
                    Ok(())
                })();
                self.handle.broadcast_state("eq");
                let _ = reply.send(res);
            }
            DeviceCommand::ActivatePreset { name, reply } => {
                let res = (|| -> Result<ParametricEqProfile> {
                    let profile = self.handle.config_manager.load_profile(&name)?;
                    let report = profile.build_hid_report();

                    if let Ok(mut st) = self.handle.state.write() {
                        st.config.active_profile = profile.name.clone();
                        st.active_eq = profile.clone();
                        let _ = self.handle.config_manager.save_config(&st.config);
                        st.available_presets = load_all_presets(&self.handle.config_manager);
                        st.touch();
                    }

                    let save_to_flash = self.handle.state.read().map(|s| s.config.save_to_flash).unwrap_or(false);
                    send_report_if_connected(&report, usb_handle, save_to_flash)?;
                    Ok(profile)
                })();
                self.handle.broadcast_state("eq");
                let _ = reply.send(res);
            }
            DeviceCommand::SetSidetone {
                enabled,
                level,
                percent,
                reply,
            } => {
                let res = (|| -> Result<u8> {
                    let (prev_level, save_to_flash) = self
                        .handle
                        .state
                        .read()
                        .map(|s| (s.config.hardware.sidetone_level, s.config.save_to_flash))
                        .unwrap_or((0, false));
                    let resolved_level = if !enabled {
                        0
                    } else if let Some(lvl) = level {
                        lvl.min(MAX_SIDETONE_LEVEL)
                    } else if let Some(pct) = percent {
                        sidetone_bucket(pct.min(MAX_VOLUME_PERCENT))
                    } else {
                        prev_level.max(1).min(MAX_SIDETONE_LEVEL)
                    };

                    let report = if let Some(pct) = percent {
                        if enabled {
                            build_sidetone_report(pct)
                        } else {
                            build_sidetone_level_report(0)
                        }
                    } else {
                        build_sidetone_level_report(resolved_level)
                    };

                    if let Ok(mut st) = self.handle.state.write() {
                        st.config.hardware.sidetone_enabled = resolved_level > 0;
                        if !enabled && level.is_none() && percent.is_none() {
                            st.config.hardware.sidetone_level = prev_level;
                        } else {
                            st.config.hardware.sidetone_level = resolved_level;
                        }
                        st.telemetry.sidetone_level = Some(resolved_level);
                        let _ = self.handle.config_manager.save_config(&st.config);
                        st.touch();
                    }

                    send_report_if_connected(&report, usb_handle, save_to_flash)?;
                    Ok(resolved_level)
                })();
                self.handle.broadcast_state("sidetone");
                let _ = reply.send(res);
            }
            DeviceCommand::UpdateSettings { patch, reply } => {
                let res = (|| -> Result<HardwareConfig> {
                    let mut reports: Vec<[u8; HID_MSG_SIZE]> = Vec::new();
                    let updated_hw = {
                        let mut st = self
                            .handle
                            .state
                            .write()
                            .map_err(|_| anyhow::anyhow!("State RwLock poisoned"))?;

                        if let Some(enabled) = patch.sidetone_enabled {
                            st.config.hardware.sidetone_enabled = enabled;
                        }
                        if let Some(lvl) = patch.sidetone_level {
                            st.config.hardware.sidetone_level = lvl.min(MAX_SIDETONE_LEVEL);
                        }
                        if patch.sidetone_enabled.is_some() || patch.sidetone_level.is_some() {
                            let effective = if st.config.hardware.sidetone_enabled {
                                st.config.hardware.sidetone_level
                            } else {
                                0
                            };
                            reports.push(build_sidetone_level_report(effective));
                            st.telemetry.sidetone_level = Some(effective);
                        }

                        if let Some(gain) = patch.mic_gain {
                            let clamped = gain.min(MAX_MIC_GAIN_LEVEL);
                            st.config.hardware.mic_gain = clamped;
                            st.telemetry.mic_gain = Some(clamped);
                            reports.push(build_mic_gain_report(clamped));
                        }

                        if let Some(limiter) = patch.volume_limiter {
                            st.config.hardware.volume_limiter = limiter;
                            st.telemetry.volume_limiter = Some(limiter);
                            reports.push(build_volume_limiter_report(limiter));
                        }

                        if let Some(mins) = patch.inactivity_timer_mins {
                            let clamped = mins.min(MAX_INACTIVITY_MINUTES);
                            st.config.hardware.inactivity_timer_mins = clamped;
                            st.telemetry.inactivity_timer_mins = Some(clamped);
                            reports.push(build_inactivity_timer_report(clamped));
                        }

                        if let Some(led) = patch.mute_led_brightness {
                            let clamped = led.min(MAX_MUTE_LED_BRIGHTNESS);
                            st.config.hardware.mute_led_brightness = clamped;
                            st.telemetry.mute_led_brightness = Some(clamped);
                            reports.push(build_mute_led_brightness_report(clamped));
                        }

                        if let Some(bt_pwr) = patch.bt_power_on_default {
                            st.config.hardware.bt_power_on_default = bt_pwr;
                            st.telemetry.bt_power_default = Some(bt_pwr);
                            reports.push(build_bt_power_default_report(bt_pwr));
                            // Bluetooth power-on default must be written to onboard flash (0x09)
                            // to persist across headset power cycles.
                            reports.push(build_save_to_flash_report());
                        }

                        if let Some(bt_call) = patch.bt_call_behavior {
                            st.config.hardware.bt_call_behavior = bt_call;
                            st.telemetry.bt_call_behavior = Some(bt_call);
                            reports.push(build_bt_call_behavior_report(bt_call));
                        }

                        if let Some(auto_apply) = patch.auto_apply_on_connect {
                            st.config.auto_apply_on_connect = auto_apply;
                        }
                        if let Some(save_flash) = patch.save_to_flash {
                            st.config.save_to_flash = save_flash;
                        }
                        if let Some(poll_secs) = patch.poll_status_interval_secs {
                            st.config.poll_status_interval_secs = poll_secs.max(2);
                        }

                        let _ = self.handle.config_manager.save_config(&st.config);
                        let save_to_flash_flag = st.config.save_to_flash;
                        send_reports_if_connected(&reports, usb_handle, save_to_flash_flag)?;
                        st.touch();
                        st.config.hardware.clone()
                    };

                    if !reports.is_empty() {
                        let save_to_flash = self.handle.state.read().map(|s| s.config.save_to_flash).unwrap_or(false);
                        send_reports_if_connected(&reports, usb_handle, save_to_flash)?;
                    }

                    Ok(updated_hw)
                })();
                self.handle.broadcast_state("settings");
                let _ = reply.send(res);
            }
            DeviceCommand::SaveToFlash { reply } => {
                let report = build_save_to_flash_report();
                let res = send_report_serialized(&report, usb_handle);
                if res.is_ok() {
                    info!("Saved active DSP & hardware settings to onboard flash (0x09)");
                }
                self.handle.broadcast_state("flash_saved");
                let _ = reply.send(res);
            }
            DeviceCommand::RequestStatus { reply } => {
                let res = (|| -> Result<()> {
                    if let Ok(Some(event)) = hidapi_request_status() {
                        if let Ok(mut st) = self.handle.state.write() {
                            st.telemetry.apply_event(&event);
                            st.touch();
                        }
                        return Ok(());
                    }
                    let req = build_status_request_report();
                    send_report_serialized(&req, usb_handle)
                })();
                self.handle.broadcast_state("telemetry");
                let _ = reply.send(res);
            }
        }
    }
}

fn send_report_if_connected<T: UsbContext>(
    report: &[u8; HID_MSG_SIZE],
    usb_handle: Option<(&DeviceHandle<T>, u8)>,
    save_to_flash: bool,
) -> Result<()> {
    let mut reports = vec![*report];
    if save_to_flash {
        reports.push(build_save_to_flash_report());
    }
    send_reports_if_connected(&reports, usb_handle, false)
}

fn send_reports_if_connected<T: UsbContext>(
    reports: &[[u8; HID_MSG_SIZE]],
    usb_handle: Option<(&DeviceHandle<T>, u8)>,
    save_to_flash: bool,
) -> Result<()> {
    let mut reports_to_send = reports.to_vec();
    if save_to_flash {
        reports_to_send.push(build_save_to_flash_report());
    }

    match hidapi_send_reports(&reports_to_send) {
        Ok(()) => Ok(()),
        Err(hid_err) => {
            if let Some((handle, interface_num)) = usb_handle {
                debug!(
                    "hidapi batch write failed ({}), falling back to rusb control transfers",
                    hid_err
                );
                for (idx, report) in reports_to_send.iter().enumerate() {
                    rusb_send_hid_report(handle, interface_num, report)?;
                    if idx + 1 < reports_to_send.len() {
                        std::thread::sleep(USB_INTER_REPORT_DELAY);
                    }
                }
                Ok(())
            } else {
                warn!(
                    "USB device not connected; state/config updated and will be applied when headset connects (hidapi: {})",
                    hid_err
                );
                Ok(())
            }
        }
    }
}

fn send_report_serialized<T: UsbContext>(
    report: &[u8; HID_MSG_SIZE],
    usb_handle: Option<(&DeviceHandle<T>, u8)>,
) -> Result<()> {
    match hidapi_send_report(report) {
        Ok(()) => Ok(()),
        Err(hid_err) => {
            if let Some((handle, interface_num)) = usb_handle {
                debug!(
                    "hidapi write failed ({}), falling back to rusb control transfer on IF #{}",
                    hid_err, interface_num
                );
                rusb_send_hid_report(handle, interface_num, report)
            } else {
                warn!("USB device not connected (hidapi: {})", hid_err);
                Err(hid_err)
            }
        }
    }
}

// Removed unused function send_reports_serialized to eliminate dead code warning.

pub fn load_all_presets(config_manager: &ConfigManager) -> Vec<ParametricEqProfile> {
    let mut out = Vec::new();
    if let Ok(names) = config_manager.list_profiles() {
        for name in names {
            if let Ok(profile) = config_manager.load_profile(&name) {
                out.push(profile);
            }
        }
    }
    if out.is_empty() {
        out = ParametricEqProfile::builtin_presets();
    }
    out
}

pub fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
