// Live Battery & Hardware Telemetry Engine for SteelSeries Arctis Nova 7.
//
// Supported Opcodes:
// - `0xB0`: General Status Snapshot (connection status, battery level, charging state, ChatMix, mute)
// - `0xB7`: Live Battery Level Sync Event (discrete 0..=4 or percentage 0..=100)
// - `0xB9`: 2.4G Wireless Connection State (0: Unpaired, 1: Searching, 2: Disconnected, 3: Connected)
// - `0xBB`: Charging Status (0: Disconnected/NotCharging, 1: Charging, 2/3: Charged)
// - `0x52`: Microphone Mute Status (0: Unmuted, 1: Muted)
// - `0x45`: ChatMix Potentiometer Event (game 0..=100, chat 0..=100)
// - `0x37` / `0x39` / `0x3A` / `0xA3` / `0xAE` / `0xB2` / `0xB3`: Hardware setting sync events

use crate::device::{
    BtCallBehavior, BATTERY_LEVEL_OPCODE, BT_CALL_BEHAVIOR_OPCODE, BT_POWER_DEFAULT_OPCODE,
    CHARGING_STATUS_OPCODE, CHATMIX_OPCODE, GAIA_EVENT_NOTIFICATION_CMD,
    GAIA_GENERAL_SETTINGS_CMD, GAIA_GET_EQ_NAME_CMD, GAIA_REGISTER_NOTIFICATION_CMD,
    GAIA_SET_BT_CALL_BEHAVIOR_CMD, GAIA_SET_BT_POWER_DEFAULT_CMD,
    GAIA_SET_INACTIVITY_TIMER_CMD, GAIA_SET_MIC_GAIN_CMD, GAIA_SET_MUTE_LED_CMD,
    GAIA_SET_SIDETONE_CMD, GAIA_SET_VOLUME_LIMITER_CMD, GENERAL_SETTINGS_OPCODE,
    GET_EQ_NAME_OPCODE, INACTIVITY_TIMER_OPCODE, MAX_INACTIVITY_MINUTES, MAX_MIC_VOLUME_LEVEL,
    MAX_MUTE_LED_BRIGHTNESS, MAX_SIDETONE_LEVEL, MAX_VOLUME_PERCENT, MIC_GAIN_OPCODE,
    MIC_MUTE_OPCODE, MUTE_LED_OPCODE, SIDETONE_OPCODE, STATUS_OPCODE, VOLUME_LIMITER_OPCODE,
    WIRELESS_STATUS_OPCODE,
};
use std::fmt;

/// 2.4GHz Wireless Connection State (`0xB9` and `0xB0` status byte).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WirelessState {
    Unpaired,
    Searching,
    Disconnected,
    Connected,
    Unknown(u8),
}

impl Default for WirelessState {
    fn default() -> Self {
        Self::Disconnected
    }
}

impl WirelessState {
    pub fn from_byte(raw: u8) -> Self {
        match raw {
            0 => Self::Unpaired,
            1 => Self::Searching,
            2 => Self::Disconnected,
            3 => Self::Connected,
            other => Self::Unknown(other),
        }
    }

    #[allow(dead_code)]
    pub fn is_connected(self) -> bool {
        matches!(self, Self::Connected)
    }
}

impl fmt::Display for WirelessState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unpaired => write!(f, "Unpaired (0)"),
            Self::Searching => write!(f, "Searching (1)"),
            Self::Disconnected => write!(f, "Disconnected (2)"),
            Self::Connected => write!(f, "Connected (3)"),
            Self::Unknown(v) => write!(f, "Unknown ({})", v),
        }
    }
}

/// Battery charging state (`0xBB` and `0xB0` status byte).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChargingState {
    NotCharging,
    Charging,
    Charged,
    Unknown(u8),
}

impl Default for ChargingState {
    fn default() -> Self {
        Self::NotCharging
    }
}

impl ChargingState {
    pub fn from_byte(raw: u8) -> Self {
        match raw {
            0 => Self::NotCharging,
            1 => Self::Charging,
            2 => Self::Charged,
            3 => Self::NotCharging,
            other => Self::Unknown(other),
        }
    }

    pub fn is_charging(&self) -> bool {
        matches!(self, Self::Charging)
    }

    pub fn is_charged(&self) -> bool {
        matches!(self, Self::Charged)
    }
}

impl fmt::Display for ChargingState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotCharging => write!(f, "Discharging / Not Charging"),
            Self::Charging => write!(f, "Charging"),
            Self::Charged => write!(f, "Charged (Full)"),
            Self::Unknown(v) => write!(f, "Unknown ({})", v),
        }
    }
}

/// Normalizes a raw battery byte into a percentage (`0..=100`).
///
/// - Gen 1 firmware reports discrete steps `0..=4` -> mapped to `0%, 25%, 50%, 75%, 100%`.
///   (If a Gen 1 PID has updated firmware reporting `> 4`, falls back to direct `0..=100%`.)
/// - Gen 2 firmware (`0x22A1`, `0x227C`, `0x227E`, etc.) reports direct percentage `0..=100%`.
pub fn normalize_battery_percent(raw: u8, is_percentage_device: bool) -> u8 {
    if is_percentage_device || raw > 4 {
        raw.min(100)
    } else {
        match raw {
            0 => 0,
            1 => 25,
            2 => 50,
            3 => 75,
            _ => 100,
        }
    }
}

/// Parsed snapshot from general status report (`0xB0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeneralStatusReport {
    pub wireless: WirelessState,
    pub battery_raw: u8,
    pub battery_percent: u8,
    pub charging: ChargingState,
    pub game_volume: Option<u8>,
    pub chat_volume: Option<u8>,
    pub mic_muted: Option<bool>,
}

/// Complete hardware settings snapshot decoded from opcode `0x11` / GAIA `0x0911`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeneralSettingsReport {
    pub mic_volume: u8,
    pub sidetone_level: u8,
    pub volume_limiter: bool,
    pub inactivity_timer_mins: u8,
    pub mute_led_brightness: u8,
    pub wireless: Option<WirelessState>,
    pub battery_raw: Option<u8>,
    pub battery_percent: Option<u8>,
    pub charging: Option<ChargingState>,
    pub game_volume: Option<u8>,
    pub chat_volume: Option<u8>,
    pub bt_power_default: Option<bool>,
    pub bt_call_behavior: Option<BtCallBehavior>,
    pub mic_muted: Option<bool>,
}

/// Discrete telemetry or sync event decoded from an incoming HID packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelemetryEvent {
    ChatMix { game: u8, chat: u8 },
    GeneralStatus(GeneralStatusReport),
    GeneralSettings(GeneralSettingsReport),
    BatteryLevel { raw: u8, percent: u8 },
    WirelessConnection(WirelessState),
    ChargingStatus(ChargingState),
    MicMute(bool),
    MicGain(u8),
    Sidetone(u8),
    VolumeLimiter(bool),
    InactivityTimer(u8),
    MuteLedBrightness(u8),
    BtPowerDefault(bool),
    BtCallBehavior(BtCallBehavior),
    EqPresetName { channel_id: Option<u8>, custom: bool, name: String },
}

/// Live telemetry state of the connected Arctis Nova 7 headset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeadsetTelemetry {
    pub wireless: WirelessState,
    pub battery_raw: Option<u8>,
    pub battery_percent: Option<u8>,
    pub charging: ChargingState,
    pub mic_muted: Option<bool>,
    pub game_volume: Option<u8>,
    pub chat_volume: Option<u8>,
    pub sidetone_level: Option<u8>,
    pub mic_gain: Option<u8>,
    pub volume_limiter: Option<bool>,
    pub inactivity_timer_mins: Option<u8>,
    pub mute_led_brightness: Option<u8>,
    pub bt_power_default: Option<bool>,
    pub bt_call_behavior: Option<BtCallBehavior>,
}

impl HeadsetTelemetry {
    /// Applies a `TelemetryEvent` to the current state and returns `true` if any field changed.
    pub fn apply_event(&mut self, event: &TelemetryEvent) -> bool {
        let before = self.clone();
        match event {
            TelemetryEvent::ChatMix { game, chat } => {
                self.game_volume = Some(*game);
                self.chat_volume = Some(*chat);
            }
            TelemetryEvent::GeneralStatus(report) => {
                self.wireless = report.wireless;
                self.battery_raw = Some(report.battery_raw);
                self.battery_percent = Some(report.battery_percent);
                self.charging = report.charging;
                if let Some(g) = report.game_volume {
                    self.game_volume = Some(g);
                }
                if let Some(c) = report.chat_volume {
                    self.chat_volume = Some(c);
                }
                if let Some(m) = report.mic_muted {
                    self.mic_muted = Some(m);
                }
            }
            TelemetryEvent::GeneralSettings(report) => {
                self.sidetone_level = Some(report.sidetone_level);
                self.mic_gain = Some(report.mic_volume.min(MAX_MIC_VOLUME_LEVEL));
                self.volume_limiter = Some(report.volume_limiter);
                self.inactivity_timer_mins = Some(report.inactivity_timer_mins);
                self.mute_led_brightness = Some(report.mute_led_brightness);
                if let Some(w) = report.wireless {
                    self.wireless = w;
                }
                if let Some(r) = report.battery_raw {
                    self.battery_raw = Some(r);
                }
                if let Some(p) = report.battery_percent {
                    self.battery_percent = Some(p);
                }
                if let Some(c) = report.charging {
                    self.charging = c;
                }
                if let Some(g) = report.game_volume {
                    self.game_volume = Some(g);
                }
                if let Some(c) = report.chat_volume {
                    self.chat_volume = Some(c);
                }
                if let Some(p) = report.bt_power_default {
                    self.bt_power_default = Some(p);
                }
                if let Some(b) = report.bt_call_behavior {
                    self.bt_call_behavior = Some(b);
                }
                if let Some(m) = report.mic_muted {
                    self.mic_muted = Some(m);
                }
            }
            TelemetryEvent::BatteryLevel { raw, percent } => {
                self.battery_raw = Some(*raw);
                self.battery_percent = Some(*percent);
            }
            TelemetryEvent::WirelessConnection(state) => {
                self.wireless = *state;
            }
            TelemetryEvent::ChargingStatus(state) => {
                self.charging = *state;
            }
            TelemetryEvent::MicMute(muted) => {
                self.mic_muted = Some(*muted);
            }
            TelemetryEvent::MicGain(level) => {
                self.mic_gain = Some((*level).min(MAX_MIC_VOLUME_LEVEL));
            }
            TelemetryEvent::Sidetone(level) => {
                self.sidetone_level = Some((*level).min(MAX_SIDETONE_LEVEL));
            }
            TelemetryEvent::VolumeLimiter(enabled) => {
                self.volume_limiter = Some(*enabled);
            }
            TelemetryEvent::InactivityTimer(mins) => {
                self.inactivity_timer_mins = Some((*mins).min(MAX_INACTIVITY_MINUTES));
            }
            TelemetryEvent::MuteLedBrightness(level) => {
                self.mute_led_brightness = Some((*level).min(MAX_MUTE_LED_BRIGHTNESS));
            }
            TelemetryEvent::BtPowerDefault(enabled) => {
                self.bt_power_default = Some(*enabled);
            }
            TelemetryEvent::BtCallBehavior(behavior) => {
                self.bt_call_behavior = Some(*behavior);
            }
            TelemetryEvent::EqPresetName { .. } => {
                // EQ preset name is handled directly by state/controller synchronization
            }
        }
        *self != before
    }

    pub fn summary_line(&self) -> String {
        let batt = self
            .battery_percent
            .map(|p| format!("{}%", p))
            .unwrap_or_else(|| "N/A".to_string());
        let mute = match self.mic_muted {
            Some(true) => "Muted",
            Some(false) => "Active",
            None => "Unknown",
        };
        let chatmix = match (self.game_volume, self.chat_volume) {
            (Some(g), Some(c)) => format!("Game {}% / Chat {}%", g, c),
            _ => "N/A".to_string(),
        };
        format!(
            "2.4G: {} | Battery: {} ({}) | Mic: {} | ChatMix: {}",
            self.wireless, batt, self.charging, mute, chatmix
        )
    }
}

/// Strips an optional leading `0x00` HID report ID byte if the second byte is a known opcode.
fn normalize_payload(buf: &[u8]) -> &[u8] {
    if buf.len() >= 2 && buf[0] == 0x00 && buf[1] != 0x00 {
        &buf[1..]
    } else {
        buf
    }
}

fn parse_general_settings_slice(payload: &[u8], is_percentage_device: bool) -> Option<GeneralSettingsReport> {
    if payload.len() < 4 {
        return None;
    }
    let mic_volume = payload.get(1).copied().unwrap_or(0).min(MAX_MIC_VOLUME_LEVEL);
    let sidetone_level = payload.get(2).copied().unwrap_or(0).min(MAX_SIDETONE_LEVEL);
    let volume_limiter = payload.get(3).map(|&b| b != 0).unwrap_or(false);
    let inactivity_timer_mins = payload.get(5).copied().unwrap_or(0).min(MAX_INACTIVITY_MINUTES);
    let mute_led_brightness = payload.get(6).copied().unwrap_or(0).min(MAX_MUTE_LED_BRIGHTNESS);
    let wireless = payload.get(11).map(|&b| WirelessState::from_byte(b));
    let battery_raw = payload.get(12).copied();
    let battery_percent = battery_raw.map(|b| normalize_battery_percent(b, is_percentage_device));
    let charging = payload.get(13).map(|&b| ChargingState::from_byte(b));
    let game_volume = payload.get(14).filter(|&&v| v <= MAX_VOLUME_PERCENT).copied();
    let chat_volume = payload.get(15).filter(|&&v| v <= MAX_VOLUME_PERCENT).copied();
    let bt_power_default = payload.get(16).map(|&b| b != 0);
    let bt_call_behavior = payload.get(17).map(|&b| BtCallBehavior::from_byte(b));
    let mic_muted = payload.get(18).map(|&b| b != 0);

    Some(GeneralSettingsReport {
        mic_volume,
        sidetone_level,
        volume_limiter,
        inactivity_timer_mins,
        mute_led_brightness,
        wireless,
        battery_raw,
        battery_percent,
        charging,
        game_volume,
        chat_volume,
        bt_power_default,
        bt_call_behavior,
        mic_muted,
    })
}

fn parse_eq_name_slice(slice: &[u8]) -> Option<TelemetryEvent> {
    if slice.is_empty() {
        return None;
    }
    // Case 1: [channel_id, custom_flag, name_bytes...]
    let (channel_id, custom, name_bytes) = if slice.len() >= 3
        && slice[0] <= 2
        && slice[1] <= 1
        && (slice[2] == 0 || (slice[2] >= 32 && slice[2] <= 126))
    {
        (Some(slice[0]), slice[1] == 1, &slice[2..])
    } else {
        // Case 2: [custom_flag, name_bytes...]
        let custom = slice[0] == 1;
        (None, custom, &slice[1..])
    };

    let end = name_bytes.iter().position(|&b| b == 0).unwrap_or(name_bytes.len().min(16));
    let valid_ascii: Vec<u8> = name_bytes[..end]
        .iter()
        .copied()
        .filter(|&b| b >= 32 && b <= 126)
        .collect();
    let name = String::from_utf8_lossy(&valid_ascii).trim().to_string();
    if !name.is_empty() {
        Some(TelemetryEvent::EqPresetName {
            channel_id,
            custom,
            name,
        })
    } else {
        None
    }
}

/// Parses an incoming USB HID interrupt or response packet into a `TelemetryEvent`.
pub fn parse_telemetry_event(buf: &[u8], is_percentage_device: bool) -> Option<TelemetryEvent> {
    // 1. Check for Qualcomm GAIA v3 command / notification frames
    let gaia_candidate = if buf.len() >= 4 && buf[0] == 0x00 && buf[1] == 0x1D {
        Some(&buf[2..])
    } else if buf.len() >= 5 && buf[0] == 0x00 && buf[1] == 0x00 && buf[2] == 0x1D {
        Some(&buf[3..])
    } else {
        None
    };

    if let Some(g) = gaia_candidate {
        if g.len() >= 2 {
            let cmd_id = ((g[0] as u16) << 8) | (g[1] as u16);
            let g_payload = &g[2..];
            match cmd_id {
                GAIA_EVENT_NOTIFICATION_CMD | GAIA_REGISTER_NOTIFICATION_CMD if !g_payload.is_empty() => {
                    let inner_opcode = g_payload[0];
                    let inner_data = &g_payload[1..];
                    match inner_opcode {
                        MIC_GAIN_OPCODE if !inner_data.is_empty() => {
                            return Some(TelemetryEvent::MicGain(inner_data[0].min(MAX_MIC_VOLUME_LEVEL)));
                        }
                        SIDETONE_OPCODE if !inner_data.is_empty() => {
                            return Some(TelemetryEvent::Sidetone(inner_data[0].min(MAX_SIDETONE_LEVEL)));
                        }
                        VOLUME_LIMITER_OPCODE if !inner_data.is_empty() => {
                            return Some(TelemetryEvent::VolumeLimiter(inner_data[0] != 0));
                        }
                        INACTIVITY_TIMER_OPCODE if !inner_data.is_empty() => {
                            return Some(TelemetryEvent::InactivityTimer(inner_data[0].min(MAX_INACTIVITY_MINUTES)));
                        }
                        MUTE_LED_OPCODE if !inner_data.is_empty() => {
                            return Some(TelemetryEvent::MuteLedBrightness(inner_data[0].min(MAX_MUTE_LED_BRIGHTNESS)));
                        }
                        BT_POWER_DEFAULT_OPCODE if !inner_data.is_empty() => {
                            return Some(TelemetryEvent::BtPowerDefault(inner_data[0] != 0));
                        }
                        BT_CALL_BEHAVIOR_OPCODE if !inner_data.is_empty() => {
                            return Some(TelemetryEvent::BtCallBehavior(BtCallBehavior::from_byte(inner_data[0])));
                        }
                        GET_EQ_NAME_OPCODE if !inner_data.is_empty() => {
                            return parse_eq_name_slice(inner_data);
                        }
                        GENERAL_SETTINGS_OPCODE if !inner_data.is_empty() => {
                            let mut full = vec![GENERAL_SETTINGS_OPCODE];
                            full.extend_from_slice(inner_data);
                            return parse_general_settings_slice(&full, is_percentage_device).map(TelemetryEvent::GeneralSettings);
                        }
                        _ => {}
                    }
                }
                GAIA_GENERAL_SETTINGS_CMD => {
                    let mut full = vec![GENERAL_SETTINGS_OPCODE];
                    full.extend_from_slice(g_payload);
                    return parse_general_settings_slice(&full, is_percentage_device).map(TelemetryEvent::GeneralSettings);
                }
                GAIA_GET_EQ_NAME_CMD => {
                    return parse_eq_name_slice(g_payload);
                }
                GAIA_SET_MIC_GAIN_CMD if !g_payload.is_empty() => {
                    return Some(TelemetryEvent::MicGain(g_payload[0].min(MAX_MIC_VOLUME_LEVEL)));
                }
                GAIA_SET_SIDETONE_CMD if !g_payload.is_empty() => {
                    return Some(TelemetryEvent::Sidetone(g_payload[0].min(MAX_SIDETONE_LEVEL)));
                }
                GAIA_SET_VOLUME_LIMITER_CMD if !g_payload.is_empty() => {
                    return Some(TelemetryEvent::VolumeLimiter(g_payload[0] != 0));
                }
                GAIA_SET_INACTIVITY_TIMER_CMD if !g_payload.is_empty() => {
                    return Some(TelemetryEvent::InactivityTimer(g_payload[0].min(MAX_INACTIVITY_MINUTES)));
                }
                GAIA_SET_MUTE_LED_CMD if !g_payload.is_empty() => {
                    return Some(TelemetryEvent::MuteLedBrightness(g_payload[0].min(MAX_MUTE_LED_BRIGHTNESS)));
                }
                GAIA_SET_BT_POWER_DEFAULT_CMD if !g_payload.is_empty() => {
                    return Some(TelemetryEvent::BtPowerDefault(g_payload[0] != 0));
                }
                GAIA_SET_BT_CALL_BEHAVIOR_CMD if !g_payload.is_empty() => {
                    return Some(TelemetryEvent::BtCallBehavior(BtCallBehavior::from_byte(g_payload[0])));
                }
                _ => {}
            }
        }
    }

    // 2. Standard USB HID Opcode packet
    let payload = normalize_payload(buf);
    if payload.len() < 2 {
        return None;
    }

    match payload[0] {
        GENERAL_SETTINGS_OPCODE => {
            parse_general_settings_slice(payload, is_percentage_device).map(TelemetryEvent::GeneralSettings)
        }
        GET_EQ_NAME_OPCODE if payload.len() >= 2 => {
            parse_eq_name_slice(&payload[1..])
        }
        CHATMIX_OPCODE if payload.len() >= 3 => {
            let game = payload[1].min(MAX_VOLUME_PERCENT);
            let chat = payload[2].min(MAX_VOLUME_PERCENT);
            Some(TelemetryEvent::ChatMix { game, chat })
        }
        STATUS_OPCODE if payload.len() >= 4 => {
            // 0xB0 status response layout on Arctis Nova 7:
            // payload[1]: wireless connection status (0..=3)
            // payload[2]: battery level (0..=4 discrete or 0..=100 percentage)
            // payload[3]: charging state (0..=3)
            // payload[4..=5]: optional game/chat mix bytes if non-zero
            // payload[6]: optional mic mute state (0 or 1)
            let wireless = WirelessState::from_byte(payload[1]);
            let battery_raw = payload[2];
            let battery_percent = normalize_battery_percent(battery_raw, is_percentage_device);
            let charging = ChargingState::from_byte(payload[3]);

            let (game_volume, chat_volume) = if payload.len() >= 6
                && (payload[4] > 0 || payload[5] > 0)
                && payload[4] <= MAX_VOLUME_PERCENT
                && payload[5] <= MAX_VOLUME_PERCENT
            {
                (Some(payload[4]), Some(payload[5]))
            } else {
                (None, None)
            };

            let mic_muted = if payload.len() >= 7 && payload[6] <= 1 {
                Some(payload[6] == 1)
            } else {
                None
            };

            Some(TelemetryEvent::GeneralStatus(GeneralStatusReport {
                wireless,
                battery_raw,
                battery_percent,
                charging,
                game_volume,
                chat_volume,
                mic_muted,
            }))
        }
        BATTERY_LEVEL_OPCODE => {
            let raw = payload[1];
            let percent = normalize_battery_percent(raw, is_percentage_device);
            Some(TelemetryEvent::BatteryLevel { raw, percent })
        }
        WIRELESS_STATUS_OPCODE => Some(TelemetryEvent::WirelessConnection(
            WirelessState::from_byte(payload[1]),
        )),
        CHARGING_STATUS_OPCODE => Some(TelemetryEvent::ChargingStatus(ChargingState::from_byte(
            payload[1],
        ))),
        MIC_MUTE_OPCODE => Some(TelemetryEvent::MicMute(payload[1] != 0)),
        MIC_GAIN_OPCODE => Some(TelemetryEvent::MicGain(
            payload[1].min(MAX_MIC_VOLUME_LEVEL),
        )),
        SIDETONE_OPCODE => Some(TelemetryEvent::Sidetone(
            payload[1].min(MAX_SIDETONE_LEVEL),
        )),
        VOLUME_LIMITER_OPCODE => Some(TelemetryEvent::VolumeLimiter(payload[1] != 0)),
        INACTIVITY_TIMER_OPCODE => Some(TelemetryEvent::InactivityTimer(
            payload[1].min(MAX_INACTIVITY_MINUTES),
        )),
        MUTE_LED_OPCODE => Some(TelemetryEvent::MuteLedBrightness(
            payload[1].min(MAX_MUTE_LED_BRIGHTNESS),
        )),
        BT_POWER_DEFAULT_OPCODE => Some(TelemetryEvent::BtPowerDefault(payload[1] != 0)),
        BT_CALL_BEHAVIOR_OPCODE => Some(TelemetryEvent::BtCallBehavior(
            BtCallBehavior::from_byte(payload[1]),
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_general_settings_report() {
        let mut buf = [0u8; 64];
        buf[0] = 0x00; // Report ID
        buf[1] = 0x11; // GENERAL_SETTINGS_OPCODE
        buf[2] = 6;    // micVolume (0..=7)
        buf[3] = 2;    // sidetone (0..=3)
        buf[4] = 1;    // volumeLimiter (1)
        buf[5] = 0;    // reserved
        buf[6] = 45;   // inactivityTimer (45m)
        buf[7] = 3;    // muteBrightness (3)
        buf[12] = 3;   // 2.4G wireless connected (3)
        buf[13] = 88;  // batteryLevel (88%)
        buf[14] = 1;   // chargingStatus (1 = charging)
        buf[15] = 80;  // chatMixGame (80)
        buf[16] = 20;  // chatMixChat (20)
        buf[17] = 1;   // btPowerOnDefault (1)
        buf[18] = 1;   // btCallBehavior (1 = Attenuate12dB)
        buf[19] = 0;   // micMuteState (0 = unmuted)

        let event = parse_telemetry_event(&buf, true).expect("Failed to parse 0x11");
        match event {
            TelemetryEvent::GeneralSettings(report) => {
                assert_eq!(report.mic_volume, 6);
                assert_eq!(report.sidetone_level, 2);
                assert!(report.volume_limiter);
                assert_eq!(report.inactivity_timer_mins, 45);
                assert_eq!(report.mute_led_brightness, 3);
                assert_eq!(report.wireless, Some(WirelessState::Connected));
                assert_eq!(report.battery_percent, Some(88));
                assert_eq!(report.charging, Some(ChargingState::Charging));
                assert_eq!(report.game_volume, Some(80));
                assert_eq!(report.chat_volume, Some(20));
                assert_eq!(report.bt_power_default, Some(true));
                assert_eq!(report.bt_call_behavior, Some(BtCallBehavior::Attenuate12dB));
                assert_eq!(report.mic_muted, Some(false));

                let mut telem = HeadsetTelemetry::default();
                assert!(telem.apply_event(&TelemetryEvent::GeneralSettings(report)));
                assert_eq!(telem.mic_gain, Some(6));
                assert_eq!(telem.sidetone_level, Some(2));
                assert_eq!(telem.battery_percent, Some(88));
                assert_eq!(telem.charging, ChargingState::Charging);
            }
            _ => panic!("Expected GeneralSettings event"),
        }
    }

    #[test]
    fn test_parse_eq_name_report() {
        // [0x00 (report ID), 0xA6, channel (0), custom (1), 'D', 'e', 'a', 'd', 'l', 'o', 'c', 'k', 0, ...]
        let mut buf = [0u8; 64];
        buf[0] = 0x00;
        buf[1] = 0xA6;
        buf[2] = 0x00; // 2.4G channel
        buf[3] = 0x01; // custom = true
        let name_bytes = b"Deadlock";
        buf[4..4 + name_bytes.len()].copy_from_slice(name_bytes);

        let event = parse_telemetry_event(&buf, true).expect("Failed to parse 0xA6");
        match event {
            TelemetryEvent::EqPresetName { channel_id, custom, name } => {
                assert_eq!(channel_id, Some(0));
                assert!(custom);
                assert_eq!(name, "Deadlock");
            }
            _ => panic!("Expected EqPresetName event"),
        }

        // Two-byte format without channel: [0x00, 0xA6, custom (0), 'B', 'a', 'l', 'a', 'n', 'c', 'e', 'd', 0]
        let mut buf2 = [0u8; 64];
        buf2[0] = 0x00;
        buf2[1] = 0xA6;
        buf2[2] = 0x00; // custom = false
        let bal_bytes = b"Balanced";
        buf2[3..3 + bal_bytes.len()].copy_from_slice(bal_bytes);

        let event2 = parse_telemetry_event(&buf2, true).expect("Failed to parse 0xA6");
        match event2 {
            TelemetryEvent::EqPresetName { custom, name, .. } => {
                assert!(!custom);
                assert_eq!(name, "Balanced");
            }
            _ => panic!("Expected EqPresetName event"),
        }
    }

    #[test]
    fn test_parse_gaia_notification_event() {
        // GAIA event frame: [0x00 (report id), 0x00, 0x1D (vendor), 0x40, 0x03 (notif), 0x37 (mic), 0x05 (level 5)]
        let buf = [0x00, 0x00, 0x1D, 0x40, 0x03, 0x37, 0x05];
        let event = parse_telemetry_event(&buf, true).expect("Failed to parse GAIA mic event");
        assert_eq!(event, TelemetryEvent::MicGain(5));

        // GAIA event for sidetone: 0x39 level 3
        let buf_st = [0x00, 0x00, 0x1D, 0x40, 0x03, 0x39, 0x03];
        let event_st = parse_telemetry_event(&buf_st, true).expect("Failed to parse GAIA sidetone event");
        assert_eq!(event_st, TelemetryEvent::Sidetone(3));
    }
}
