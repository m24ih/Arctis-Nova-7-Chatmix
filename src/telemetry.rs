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
    CHARGING_STATUS_OPCODE, CHATMIX_OPCODE, INACTIVITY_TIMER_OPCODE, MAX_INACTIVITY_MINUTES,
    MAX_MIC_GAIN_LEVEL, MAX_MUTE_LED_BRIGHTNESS, MAX_SIDETONE_LEVEL, MAX_VOLUME_PERCENT,
    MIC_GAIN_OPCODE, MIC_MUTE_OPCODE, MUTE_LED_OPCODE, SIDETONE_OPCODE, STATUS_OPCODE,
    VOLUME_LIMITER_OPCODE, WIRELESS_STATUS_OPCODE,
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
            2 | 3 => Self::Charged,
            other => Self::Unknown(other),
        }
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

/// Discrete telemetry or sync event decoded from an incoming HID packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelemetryEvent {
    ChatMix { game: u8, chat: u8 },
    GeneralStatus(GeneralStatusReport),
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
        match *event {
            TelemetryEvent::ChatMix { game, chat } => {
                self.game_volume = Some(game);
                self.chat_volume = Some(chat);
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
            TelemetryEvent::BatteryLevel { raw, percent } => {
                self.battery_raw = Some(raw);
                self.battery_percent = Some(percent);
            }
            TelemetryEvent::WirelessConnection(state) => {
                self.wireless = state;
            }
            TelemetryEvent::ChargingStatus(state) => {
                self.charging = state;
            }
            TelemetryEvent::MicMute(muted) => {
                self.mic_muted = Some(muted);
            }
            TelemetryEvent::MicGain(level) => {
                self.mic_gain = Some(level);
            }
            TelemetryEvent::Sidetone(level) => {
                self.sidetone_level = Some(level);
            }
            TelemetryEvent::VolumeLimiter(enabled) => {
                self.volume_limiter = Some(enabled);
            }
            TelemetryEvent::InactivityTimer(mins) => {
                self.inactivity_timer_mins = Some(mins);
            }
            TelemetryEvent::MuteLedBrightness(level) => {
                self.mute_led_brightness = Some(level);
            }
            TelemetryEvent::BtPowerDefault(enabled) => {
                self.bt_power_default = Some(enabled);
            }
            TelemetryEvent::BtCallBehavior(behavior) => {
                self.bt_call_behavior = Some(behavior);
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

/// Parses an incoming USB HID interrupt or response packet into a `TelemetryEvent`.
pub fn parse_telemetry_event(buf: &[u8], is_percentage_device: bool) -> Option<TelemetryEvent> {
    let payload = normalize_payload(buf);
    if payload.len() < 2 {
        return None;
    }

    match payload[0] {
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
            payload[1].min(MAX_MIC_GAIN_LEVEL),
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
