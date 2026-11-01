// System Tray (StatusNotifierItem / KDE Plasma / SNI) indicator & context menu for Arctis Nova 7.
//
// Features:
// - Procedurally drawn 32x32 ARGB32 headset icon matching device state:
//   * Disconnected: Slate/Dark Grey ([255, 110, 115, 120])
//   * Charging: Blue ([255, 52, 152, 219])
//   * Connected on Battery > 25%: Green ([255, 46, 204, 113])
//   * Connected on Battery 11..=25%: Yellow ([255, 241, 196, 15])
//   * Connected on Battery <= 10%: Red ([255, 231, 76, 60])
// - Left-click opens Web Control Panel in default browser via xdg-open.
// - Cloudflare WARP-style right-click context menu:
//   * 'Open Control Panel'
//   * Telemetry section (Connection, Battery %, ChatMix)
//   * 'Mode' submenu (2.4G / Bluetooth / Microphone EQ Presets with active checkmark)
//   * 'Quit' to shut down daemon.

use crate::eq::{EqChannel, ParametricEqProfile};
use crate::state::{DeviceCommand, DeviceWorkerHandle, HeadsetState};
use anyhow::{Context, Result};
use ksni::blocking::TrayMethods;
use ksni::menu::{CheckmarkItem, MenuItem, StandardItem, SubMenu};
use ksni::{Icon, ToolTip, Tray};
use log::{debug, info, warn};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub const COLOR_DISCONNECTED: [u8; 4] = [255, 110, 115, 120];
pub const COLOR_CHARGING: [u8; 4] = [255, 52, 152, 219];
pub const COLOR_BATTERY_GREEN: [u8; 4] = [255, 46, 204, 113];
pub const COLOR_BATTERY_YELLOW: [u8; 4] = [255, 241, 196, 15];
pub const COLOR_BATTERY_RED: [u8; 4] = [255, 231, 76, 60];

const ICON_SIZE: usize = 32;

/// Formats the web URL for opening in the browser, normalizing `0.0.0.0` to `127.0.0.1`.
pub fn format_web_url(bind: &str) -> String {
    let trimmed = bind.trim();
    let host_port = if let Some(rest) = trimmed.strip_prefix("0.0.0.0:") {
        format!("127.0.0.1:{}", rest)
    } else {
        trimmed.to_string()
    };

    if host_port.starts_with("http://") || host_port.starts_with("https://") {
        host_port
    } else {
        format!("http://{}", host_port)
    }
}

/// Spawns default browser to open the given URL via `xdg-open`.
pub fn open_browser(url: &str) {
    info!("Opening web browser at: {}", url);
    let bin = if std::path::Path::new("/usr/bin/xdg-open").exists() {
        "/usr/bin/xdg-open"
    } else {
        "xdg-open"
    };

    match std::process::Command::new(bin).arg(url).spawn() {
        Ok(_) => debug!("Spawned {} for {}", bin, url),
        Err(e) => warn!("Failed to launch browser ({}) for {}: {}", bin, url, e),
    }
}

/// Helper to set an ARGB32 pixel on the 32x32 buffer.
#[inline]
fn set_pixel(buf: &mut [u8], x: usize, y: usize, color: [u8; 4]) {
    if x < ICON_SIZE && y < ICON_SIZE {
        let idx = (y * ICON_SIZE + x) * 4;
        buf[idx..idx + 4].copy_from_slice(&color);
    }
}

/// Procedurally draws a 32x32 ARGB32 headset icon with the specified color.
pub fn draw_headset_icon(color: [u8; 4]) -> Icon {
    let mut data = vec![0u8; ICON_SIZE * ICON_SIZE * 4];

    let cx = 15.5f32;
    let cy = 14.0f32;

    // 1. Headband arc (upper arch)
    for y in 3..=13 {
        for x in 3..=28 {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist >= 8.5 && dist <= 11.2 {
                set_pixel(&mut data, x, y, color);
            }
        }
    }

    // 2. Headband hangers connecting arc to earcups
    for y in 11..=14 {
        set_pixel(&mut data, 6, y, color);
        set_pixel(&mut data, 25, y, color);
    }

    // 3. Left earcup (pill shape)
    for y in 14..=23 {
        for x in 4..=8 {
            if (y == 14 || y == 23) && (x == 4 || x == 8) {
                continue; // rounded corners
            }
            set_pixel(&mut data, x, y, color);
        }
    }

    // 4. Right earcup (pill shape)
    for y in 14..=23 {
        for x in 23..=27 {
            if (y == 14 || y == 23) && (x == 23 || x == 27) {
                continue; // rounded corners
            }
            set_pixel(&mut data, x, y, color);
        }
    }

    // 5. Microphone boom (curves out from bottom-inner left earcup towards center)
    let mic_boom = [
        (6, 23),
        (6, 24),
        (7, 25),
        (8, 26),
        (9, 26),
        (10, 26),
        (11, 26),
        (12, 26),
    ];
    for (x, y) in mic_boom {
        set_pixel(&mut data, x, y, color);
    }

    // 6. Microphone capsule at tip
    for y in 25..=27 {
        for x in 13..=15 {
            set_pixel(&mut data, x, y, color);
        }
    }

    Icon {
        width: ICON_SIZE as i32,
        height: ICON_SIZE as i32,
        data,
    }
}

/// Evaluates state to determine which headset icon color should be rendered.
pub fn determine_icon_color(state: &HeadsetState) -> [u8; 4] {
    let connected = state.usb_connected && state.telemetry.wireless.is_connected();
    if !connected {
        return COLOR_DISCONNECTED;
    }

    if state.telemetry.charging.is_charging() {
        return COLOR_CHARGING;
    }

    match state.telemetry.battery_percent {
        Some(pct) if pct <= 10 => COLOR_BATTERY_RED,
        Some(pct) if pct <= 25 => COLOR_BATTERY_YELLOW,
        _ => COLOR_BATTERY_GREEN,
    }
}

/// Formats human-readable status string for connection and battery.
pub fn format_connection_status(state: &HeadsetState) -> String {
    let connected = state.usb_connected && state.telemetry.wireless.is_connected();
    if !connected {
        return "Status: Disconnected".to_string();
    }

    if state.telemetry.charging.is_charging() {
        match state.telemetry.battery_percent {
            Some(pct) => format!("Status: Charging ({}%)", pct),
            None => "Status: Charging".to_string(),
        }
    } else {
        match state.telemetry.battery_percent {
            Some(pct) => format!("Status: Connected ({}%)", pct),
            None => "Status: Connected".to_string(),
        }
    }
}

/// Formats human-readable ChatMix volume status string.
pub fn format_chatmix_status(state: &HeadsetState) -> String {
    let game = state.telemetry.game_volume.unwrap_or(100);
    let chat = state.telemetry.chat_volume.unwrap_or(100);
    format!("ChatMix: Game {}% / Chat {}%", game, chat)
}

/// Builds preset selection items for a given audio channel submenu.
fn build_preset_items(
    presets: &[ParametricEqProfile],
    active_name: &str,
    channel: EqChannel,
    worker_handle: &DeviceWorkerHandle,
) -> Vec<MenuItem<ArctisTray>> {
    presets
        .iter()
        .map(|preset| {
            let is_checked = preset.name.eq_ignore_ascii_case(active_name);
            let handle = worker_handle.clone();
            let preset_name = preset.name.clone();
            CheckmarkItem {
                label: preset.name.clone(),
                checked: is_checked,
                activate: Box::new(move |_| {
                    let (tx, _rx) = std::sync::mpsc::channel();
                    let _ = handle.send_command(DeviceCommand::ActivatePreset {
                        channel,
                        name: preset_name.clone(),
                        reply: tx,
                    });
                }),
                ..Default::default()
            }
            .into()
        })
        .collect()
}

/// StatusNotifierItem Tray implementation for Arctis Nova 7.
pub struct ArctisTray {
    worker_handle: DeviceWorkerHandle,
    running: Arc<AtomicBool>,
    web_url: String,
}

impl ArctisTray {
    pub fn new(
        worker_handle: DeviceWorkerHandle,
        running: Arc<AtomicBool>,
        web_bind: &str,
    ) -> Self {
        let web_url = format_web_url(web_bind);
        Self {
            worker_handle,
            running,
            web_url,
        }
    }

    fn current_state(&self) -> Option<HeadsetState> {
        self.worker_handle.state.read().ok().map(|g| g.clone())
    }
}

impl Tray for ArctisTray {
    fn id(&self) -> String {
        "arctis-nova-7-chatmix".into()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::Hardware
    }

    fn title(&self) -> String {
        "Arctis Nova 7 ChatMix".into()
    }

    fn status(&self) -> ksni::Status {
        ksni::Status::Active
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        let color = match self.current_state() {
            Some(st) => determine_icon_color(&st),
            None => COLOR_DISCONNECTED,
        };
        vec![draw_headset_icon(color)]
    }

    fn tool_tip(&self) -> ToolTip {
        let (status_str, chatmix_str, color) = match self.current_state() {
            Some(st) => (
                format_connection_status(&st),
                format_chatmix_status(&st),
                determine_icon_color(&st),
            ),
            None => (
                "Status: Disconnected".to_string(),
                "ChatMix: Game 100% / Chat 100%".to_string(),
                COLOR_DISCONNECTED,
            ),
        };

        ToolTip {
            title: "Arctis Nova 7".into(),
            description: format!("{}\n{}", status_str, chatmix_str),
            icon_pixmap: vec![draw_headset_icon(color)],
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        open_browser(&self.web_url);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let state = self.current_state();

        let (status_label, chatmix_label, presets, active_24g, active_bt, active_mic) = match state {
            Some(st) => (
                format_connection_status(&st),
                format_chatmix_status(&st),
                st.available_presets.clone(),
                st.config.active_profile_24g.clone(),
                st.config.active_profile_bt.clone(),
                st.config.active_profile_mic.clone(),
            ),
            None => (
                "Status: Disconnected".to_string(),
                "ChatMix: Game 100% / Chat 100%".to_string(),
                Vec::new(),
                "flat".to_string(),
                "harman".to_string(),
                "flat".to_string(),
            ),
        };

        vec![
            // 1. Open Control Panel (Left or Right Click primary entry)
            StandardItem {
                label: "Open Control Panel".into(),
                activate: Box::new({
                    let url = self.web_url.clone();
                    move |_| open_browser(&url)
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            // 2. Live Telemetry & Status section (Disabled items)
            StandardItem {
                label: status_label,
                enabled: false,
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: chatmix_label,
                enabled: false,
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            // 3. Mode SubMenu (2.4G, Bluetooth, Microphone)
            SubMenu {
                label: "Mode".into(),
                submenu: vec![
                    SubMenu {
                        label: "2.4G".into(),
                        submenu: build_preset_items(
                            &presets,
                            &active_24g,
                            EqChannel::Speaker24G,
                            &self.worker_handle,
                        ),
                        ..Default::default()
                    }
                    .into(),
                    SubMenu {
                        label: "Bluetooth".into(),
                        submenu: build_preset_items(
                            &presets,
                            &active_bt,
                            EqChannel::SpeakerBt,
                            &self.worker_handle,
                        ),
                        ..Default::default()
                    }
                    .into(),
                    SubMenu {
                        label: "Microphone".into(),
                        submenu: build_preset_items(
                            &presets,
                            &active_mic,
                            EqChannel::Microphone,
                            &self.worker_handle,
                        ),
                        ..Default::default()
                    }
                    .into(),
                ],
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            // 4. Quit
            StandardItem {
                label: "Quit".into(),
                activate: Box::new({
                    let running = self.running.clone();
                    move |_| {
                        info!("Quit requested via system tray menu; stopping service...");
                        running.store(false, Ordering::SeqCst);
                    }
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Spawns the system tray service in a background thread and keeps tray state in sync.
pub fn spawn_system_tray(
    worker_handle: DeviceWorkerHandle,
    running: Arc<AtomicBool>,
    web_bind: &str,
) -> Result<ksni::blocking::Handle<ArctisTray>> {
    let tray = ArctisTray::new(worker_handle, running.clone(), web_bind);
    let handle = tray.assume_sni_available(true).spawn()?;

    let updater_handle = handle.clone();
    let updater_running = running;
    std::thread::Builder::new()
        .name("tray-updater".into())
        .spawn(move || {
            while updater_running.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_secs(1));
                if !updater_running.load(Ordering::SeqCst) {
                    break;
                }
                updater_handle.update(|_| {});
            }
        })
        .context("Failed to spawn tray-updater thread")?;

    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::telemetry::{ChargingState, WirelessState};

    #[test]
    fn test_format_web_url() {
        assert_eq!(format_web_url("127.0.0.1:3927"), "http://127.0.0.1:3927");
        assert_eq!(format_web_url("0.0.0.0:3927"), "http://127.0.0.1:3927");
        assert_eq!(
            format_web_url("http://192.168.1.10:3927"),
            "http://192.168.1.10:3927"
        );
    }

    #[test]
    fn test_draw_headset_icon_dimensions_and_pixels() {
        let icon = draw_headset_icon(COLOR_BATTERY_GREEN);
        assert_eq!(icon.width, 32);
        assert_eq!(icon.height, 32);
        assert_eq!(icon.data.len(), 32 * 32 * 4);

        // Verify there is at least one transparent pixel and at least one green pixel
        let mut has_transparent = false;
        let mut has_color = false;
        for chunk in icon.data.chunks_exact(4) {
            if chunk == [0, 0, 0, 0] {
                has_transparent = true;
            } else if chunk == COLOR_BATTERY_GREEN {
                has_color = true;
            }
        }
        assert!(has_transparent, "Icon should have transparent background");
        assert!(has_color, "Icon should have foreground headset pixels");
    }

    #[test]
    fn test_determine_icon_color_rules() {
        let mut state = HeadsetState::new(
            AppConfig::default(),
            ParametricEqProfile::flat(),
            ParametricEqProfile::harman(),
            ParametricEqProfile::flat(),
            vec![],
        );

        // 1. Disconnected
        state.usb_connected = false;
        assert_eq!(determine_icon_color(&state), COLOR_DISCONNECTED);

        state.usb_connected = true;
        state.telemetry.wireless = WirelessState::Disconnected;
        assert_eq!(determine_icon_color(&state), COLOR_DISCONNECTED);

        // 2. Charging
        state.telemetry.wireless = WirelessState::Connected;
        state.telemetry.charging = ChargingState::Charging;
        assert_eq!(determine_icon_color(&state), COLOR_CHARGING);

        // 3. On battery > 25% -> Green
        state.telemetry.charging = ChargingState::NotCharging;
        state.telemetry.battery_percent = Some(85);
        assert_eq!(determine_icon_color(&state), COLOR_BATTERY_GREEN);

        state.telemetry.battery_percent = Some(26);
        assert_eq!(determine_icon_color(&state), COLOR_BATTERY_GREEN);

        // 4. On battery 11..=25% -> Yellow
        state.telemetry.battery_percent = Some(25);
        assert_eq!(determine_icon_color(&state), COLOR_BATTERY_YELLOW);

        state.telemetry.battery_percent = Some(11);
        assert_eq!(determine_icon_color(&state), COLOR_BATTERY_YELLOW);

        // 5. On battery <= 10% -> Red
        state.telemetry.battery_percent = Some(10);
        assert_eq!(determine_icon_color(&state), COLOR_BATTERY_RED);

        state.telemetry.battery_percent = Some(1);
        assert_eq!(determine_icon_color(&state), COLOR_BATTERY_RED);
    }

    #[test]
    fn test_format_connection_status_and_chatmix() {
        let mut state = HeadsetState::new(
            AppConfig::default(),
            ParametricEqProfile::flat(),
            ParametricEqProfile::harman(),
            ParametricEqProfile::flat(),
            vec![],
        );

        // Disconnected
        assert_eq!(format_connection_status(&state), "Status: Disconnected");

        // Connected
        state.usb_connected = true;
        state.telemetry.wireless = WirelessState::Connected;
        state.telemetry.battery_percent = Some(85);
        state.telemetry.charging = ChargingState::NotCharging;
        assert_eq!(
            format_connection_status(&state),
            "Status: Connected (85%)"
        );

        // Charging
        state.telemetry.charging = ChargingState::Charging;
        assert_eq!(format_connection_status(&state), "Status: Charging (85%)");

        // ChatMix
        state.telemetry.game_volume = Some(50);
        state.telemetry.chat_volume = Some(50);
        assert_eq!(
            format_chatmix_status(&state),
            "ChatMix: Game 50% / Chat 50%"
        );
    }

    #[test]
    fn test_tray_menu_and_tooltip_generation() {
        let config_mgr = crate::config::ConfigManager::new();
        let (_worker, handle) = crate::state::DeviceWorker::create(config_mgr, AppConfig::default());
        let running = Arc::new(AtomicBool::new(true));
        let tray = ArctisTray::new(handle.clone(), running.clone(), "127.0.0.1:3927");

        assert_eq!(tray.id(), "arctis-nova-7-chatmix");
        assert_eq!(tray.title(), "Arctis Nova 7 ChatMix");

        // Tooltip
        let tooltip = tray.tool_tip();
        assert_eq!(tooltip.title, "Arctis Nova 7");
        assert!(tooltip.description.contains("Status: Disconnected"));
        assert!(!tooltip.icon_pixmap.is_empty());

        // Pixmap
        let pixmap = tray.icon_pixmap();
        assert_eq!(pixmap.len(), 1);
        assert_eq!(pixmap[0].width, 32);
        assert_eq!(pixmap[0].height, 32);

        // Menu items
        let menu_items = tray.menu();
        assert_eq!(menu_items.len(), 8);

        // 1. Open Control Panel
        match &menu_items[0] {
            MenuItem::Standard(item) => assert_eq!(item.label, "Open Control Panel"),
            other => panic!("Expected Standard item, got {:?}", other),
        }

        // 2. Separator
        assert!(matches!(&menu_items[1], MenuItem::Separator));

        // 3. Status
        match &menu_items[2] {
            MenuItem::Standard(item) => {
                assert_eq!(item.label, "Status: Disconnected");
                assert!(!item.enabled);
            }
            other => panic!("Expected Standard item, got {:?}", other),
        }

        // 4. ChatMix
        match &menu_items[3] {
            MenuItem::Standard(item) => {
                assert!(item.label.starts_with("ChatMix: Game"));
                assert!(!item.enabled);
            }
            other => panic!("Expected Standard item, got {:?}", other),
        }

        // 5. Separator
        assert!(matches!(&menu_items[4], MenuItem::Separator));

        // 6. Mode SubMenu
        match &menu_items[5] {
            MenuItem::SubMenu(mode_sub) => {
                assert_eq!(mode_sub.label, "Mode");
                assert_eq!(mode_sub.submenu.len(), 3);
            }
            other => panic!("Expected SubMenu item, got {:?}", other),
        }

        // 7. Separator
        assert!(matches!(&menu_items[6], MenuItem::Separator));

        // 8. Quit
        match &menu_items[7] {
            MenuItem::Standard(item) => assert_eq!(item.label, "Quit"),
            other => panic!("Expected Standard item, got {:?}", other),
        }
    }
}
