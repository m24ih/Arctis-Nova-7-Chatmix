// Arctis Nova 7 hardware, protocol, and virtual sink definitions.

pub(crate) const VENDOR_ID: u16 = 0x1038;

pub(crate) const SUPPORTED_PRODUCT_IDS: &[u16] = &[
    0x2200, // Arctis Nova 7 Headset / RX (discrete battery: 0-4)
    0x2202, // Arctis Nova 7 TX (discrete battery: 0-4)
    0x2204, // Arctis Nova 7X Headset / RX (discrete battery: 0-4)
    0x2206, // Arctis Nova 7x TX (discrete battery: 0-4)
    0x2208, // Arctis Nova 7P Headset / RX (discrete battery: 0-4)
    0x220a, // Arctis Nova 7P TX (discrete battery: 0-4)
    0x2238, // Arctis Nova 7 Diablo IV RX (discrete battery: 0-4)
    0x223a, // Arctis Nova 7 Diablo IV TX (discrete battery: 0-4, before Jan 2026 update)
    0x2256, // Arctis Nova 7X White RX (percentage battery: 0-100)
    0x2258, // Arctis Nova 7x v2 TX (percentage battery: 0-100)
    0x2278, // Arctis Nova 7 WoW Edition RX (discrete battery: 0-4)
    0x227a, // Arctis Nova 7 WoW Edition TX (discrete battery: 0-4)
    0x227c, // Arctis Nova 7 Gen 2 RX - brisket_rx (percentage battery: 0-100)
    0x227e, // Arctis Nova 7 Wireless Gen 2 TX (percentage battery: 0-100)
    0x2288, // Arctis Nova 7P Gen 2 RX - brisket_ps_rx (percentage battery: 0-100)
    0x228a, // Arctis Nova 7P Gen 2 TX - brisket_ps_tx (percentage battery: 0-100)
    0x229c, // Arctis Nova 7X Gen 2 RX - brisket_xbox_rx (percentage battery: 0-100)
    0x229e, // Arctis Nova 7x v2 TX (percentage battery: 0-100)
    0x22A1, // Arctis Nova 7 Gen 2 Upgrade TX (percentage battery: 0-100, Jan 2026 update)
    0x22a4, // Arctis Nova 7X (discrete battery: 0-4)
    0x22a5, // Arctis Nova 7X (percentage battery: 0-100)
    0x22a9, // Arctis Nova 7 Diablo IV TX (percentage battery: 0-100, after Jan 2026 update)
    0x22ad, // Arctis Nova 7X v2 (percentage battery: 0-100)
];

/// Product IDs that report battery directly as a percentage (`0..=100`) instead of discrete steps (`0..=4`).
pub(crate) const PERCENTAGE_BATTERY_PRODUCT_IDS: &[u16] = &[
    0x2256, 0x2258, 0x227c, 0x227e, 0x2288, 0x228a, 0x229c, 0x229e, 0x22A1, 0x22a5, 0x22a9, 0x22ad,
];

/// Product IDs that report battery in discrete steps (`0..=4`).
#[allow(dead_code)]
pub(crate) const DISCRETE_BATTERY_PRODUCT_IDS: &[u16] = &[
    0x2200, 0x2202, 0x2204, 0x2206, 0x2208, 0x220a, 0x2238, 0x223a, 0x2278, 0x227a, 0x22a4,
];

pub(crate) const HID_MSG_SIZE: usize = 64;
pub(crate) const HID_CLASS_CODE: u8 = 3;

// --- USB HID Opcodes (and Sync Event Opcodes) ---
pub(crate) const SAVE_TO_FLASH_OPCODE: u8 = 0x09;
#[allow(dead_code)]
pub(crate) const EQ_OPCODE: u8 = 0x33;
pub(crate) const EQ_24G_OPCODE: u8 = 0x33;
pub(crate) const EQ_BT_OPCODE: u8 = 0x34;
pub(crate) const EQ_MIC_OPCODE: u8 = 0x32;
pub(crate) const MIC_GAIN_OPCODE: u8 = 0x37;
pub(crate) const SIDETONE_OPCODE: u8 = 0x39;
pub(crate) const VOLUME_LIMITER_OPCODE: u8 = 0x3A;
pub(crate) const CHATMIX_OPCODE: u8 = 0x45;
pub(crate) const MIC_MUTE_OPCODE: u8 = 0x52;
pub(crate) const INACTIVITY_TIMER_OPCODE: u8 = 0xA3;
pub(crate) const MUTE_LED_OPCODE: u8 = 0xAE;
pub(crate) const STATUS_OPCODE: u8 = 0xB0;
pub(crate) const BT_POWER_DEFAULT_OPCODE: u8 = 0xB2;
pub(crate) const BT_CALL_BEHAVIOR_OPCODE: u8 = 0xB3;
pub(crate) const BATTERY_LEVEL_OPCODE: u8 = 0xB7;
pub(crate) const WIRELESS_STATUS_OPCODE: u8 = 0xB9;
pub(crate) const CHARGING_STATUS_OPCODE: u8 = 0xBB;

// --- Bluetooth GAIA v3 Set Command IDs (0x09XX mirrors USB HID opcode 0xXX) ---
#[allow(dead_code)]
pub(crate) const GAIA_VENDOR_STEELSERIES: u16 = 0x001D;
#[allow(dead_code)]
pub(crate) const GAIA_SET_EQ_CMD: u16 = 0x0933;
#[allow(dead_code)]
pub(crate) const GAIA_SET_MIC_GAIN_CMD: u16 = 0x0937;
#[allow(dead_code)]
pub(crate) const GAIA_SET_SIDETONE_CMD: u16 = 0x0939;
#[allow(dead_code)]
pub(crate) const GAIA_SET_VOLUME_LIMITER_CMD: u16 = 0x093A;
#[allow(dead_code)]
pub(crate) const GAIA_SET_INACTIVITY_TIMER_CMD: u16 = 0x09A3;
#[allow(dead_code)]
pub(crate) const GAIA_SET_MUTE_LED_CMD: u16 = 0x09AE;
#[allow(dead_code)]
pub(crate) const GAIA_SET_BT_POWER_DEFAULT_CMD: u16 = 0x09B2;
#[allow(dead_code)]
pub(crate) const GAIA_SET_BT_CALL_BEHAVIOR_CMD: u16 = 0x09B3;

pub(crate) const MAX_VOLUME_PERCENT: u8 = 100;
pub(crate) const MAX_SIDETONE_LEVEL: u8 = 3;
pub(crate) const MAX_MIC_GAIN_LEVEL: u8 = 15;
pub(crate) const MAX_MUTE_LED_BRIGHTNESS: u8 = 3;
pub(crate) const MAX_INACTIVITY_MINUTES: u8 = 90;

pub(crate) const GAME_SINK_NAME: &str = "Arctis_Game";
pub(crate) const GAME_SINK_DESC: &str = "Arctis Nova 7 Game";
pub(crate) const CHAT_SINK_NAME: &str = "Arctis_Chat";
pub(crate) const CHAT_SINK_DESC: &str = "Arctis Nova 7 Chat";
pub(crate) const FALLBACK_DEFAULT_SINK: &str = "auto_null";

/// Bluetooth call audio mixing behavior (`0xB3` / `0x09B3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BtCallBehavior {
    /// Keep 2.4G game audio unchanged during a Bluetooth call (`0x00`).
    DoNothing = 0,
    /// Lower 2.4G game audio by 12 dB during a Bluetooth call (`0x01`).
    Attenuate12dB = 1,
    /// Mute 2.4G game audio completely during a Bluetooth call (`0x02`).
    MuteGame = 2,
}

impl Default for BtCallBehavior {
    fn default() -> Self {
        Self::DoNothing
    }
}

impl BtCallBehavior {
    pub fn from_byte(raw: u8) -> Self {
        match raw {
            1 => Self::Attenuate12dB,
            2 => Self::MuteGame,
            _ => Self::DoNothing,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::DoNothing => "do_nothing",
            Self::Attenuate12dB => "attenuate_12db",
            Self::MuteGame => "mute_game",
        }
    }

    pub fn from_str_lossy(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "attenuate_12db" | "attenuate" | "-12db" | "1" => Self::Attenuate12dB,
            "mute_game" | "mute" | "2" => Self::MuteGame,
            _ => Self::DoNothing,
        }
    }
}

/// Returns `true` if the given USB vendor and product IDs match a supported Arctis Nova 7 model.
pub(crate) fn is_supported_device(vendor_id: u16, product_id: u16) -> bool {
    vendor_id == VENDOR_ID && SUPPORTED_PRODUCT_IDS.contains(&product_id)
}

/// Returns `true` if the given product ID natively reports battery in `0..=100%` rather than `0..=4`.
pub(crate) fn is_percentage_battery_device(product_id: u16) -> bool {
    PERCENTAGE_BATTERY_PRODUCT_IDS.contains(&product_id)
}

/// Returns `true` if the given product ID natively reports battery in discrete steps `0..=4`.
#[allow(dead_code)]
pub(crate) fn is_discrete_battery_device(product_id: u16) -> bool {
    DISCRETE_BATTERY_PRODUCT_IDS.contains(&product_id)
}

/// Returns the human-readable product name for a supported Arctis Nova 7 device.
#[allow(dead_code)]
pub fn device_name(product_id: u16) -> &'static str {
    match product_id {
        0x2200 => "Arctis Nova 7",
        0x2202 => "Arctis Nova 7 TX",
        0x2204 => "Arctis Nova 7X",
        0x2206 => "Arctis Nova 7X TX",
        0x2208 => "Arctis Nova 7P",
        0x220a => "Arctis Nova 7P TX",
        0x2238 => "Arctis Nova 7 Diablo IV",
        0x223a => "Arctis Nova 7 Diablo IV TX",
        0x2256 => "Arctis Nova 7X White",
        0x2258 => "Arctis Nova 7X v2 TX",
        0x2278 => "Arctis Nova 7 WoW Edition",
        0x227a => "Arctis Nova 7 WoW Edition TX",
        0x227c => "Arctis Nova 7 Gen 2",
        0x227e => "Arctis Nova 7 Wireless Gen 2 TX",
        0x2288 => "Arctis Nova 7P Gen 2",
        0x228a => "Arctis Nova 7P Gen 2 TX",
        0x229c => "Arctis Nova 7X Gen 2",
        0x229e => "Arctis Nova 7X v2 TX",
        0x22A1 => "Arctis Nova 7 Gen 2 Upgrade TX",
        0x22a4 => "Arctis Nova 7X",
        0x22a5 => "Arctis Nova 7X",
        0x22a9 => "Arctis Nova 7 Diablo IV TX",
        0x22ad => "Arctis Nova 7X v2",
        _ => "Arctis Nova 7",
    }
}

/// Maps a percentage (`0..=100`) to the 4-step hardware sidetone bucket (`0x00..=0x03`).
pub(crate) fn sidetone_bucket(percent: u8) -> u8 {
    if percent < 30 {
        0x00
    } else if percent < 60 {
        0x01
    } else if percent < 80 {
        0x02
    } else {
        0x03
    }
}

/// Maps a percentage (`0..=100`) to the 16-step hardware microphone gain level (`0x00..=0x0F`).
#[allow(dead_code)]
pub(crate) fn mic_gain_from_percent(percent: u8) -> u8 {
    let clamped = percent.min(MAX_VOLUME_PERCENT) as u16;
    ((clamped * MAX_MIC_GAIN_LEVEL as u16 + 50) / 100) as u8
}

/// Builds a 64-byte USB HID report with `[0x00, opcode, payload...]`.
pub(crate) fn build_hid_command_report(opcode: u8, payload: &[u8]) -> [u8; HID_MSG_SIZE] {
    let mut data = [0u8; HID_MSG_SIZE];
    data[0] = 0x00;
    data[1] = opcode;
    let copy_len = payload.len().min(HID_MSG_SIZE - 2);
    data[2..2 + copy_len].copy_from_slice(&payload[..copy_len]);
    data
}

/// Builds a Qualcomm GAIA v3 command frame `[vendor_hi, vendor_lo, cmd_hi, cmd_lo, payload...]`.
#[allow(dead_code)]
pub(crate) fn build_gaia_command(command_id: u16, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.push((GAIA_VENDOR_STEELSERIES >> 8) as u8);
    frame.push((GAIA_VENDOR_STEELSERIES & 0xFF) as u8);
    frame.push((command_id >> 8) as u8);
    frame.push((command_id & 0xFF) as u8);
    frame.extend_from_slice(payload);
    frame
}

/// Builds the 64-byte HID report for querying general headset status & battery (`0xB0`).
pub(crate) fn build_status_request_report() -> [u8; HID_MSG_SIZE] {
    build_hid_command_report(STATUS_OPCODE, &[])
}

/// Builds the 64-byte HID report for persisting current DSP/hardware settings to onboard flash (`0x09`).
pub(crate) fn build_save_to_flash_report() -> [u8; HID_MSG_SIZE] {
    build_hid_command_report(SAVE_TO_FLASH_OPCODE, &[0x00])
}

/// Builds the 64-byte HID report for setting the hardware sidetone level from percentage (`0..=100`).
pub(crate) fn build_sidetone_report(percent: u8) -> [u8; HID_MSG_SIZE] {
    let level = sidetone_bucket(percent.min(MAX_VOLUME_PERCENT));
    build_sidetone_level_report(level)
}

/// Builds the 64-byte HID report for setting the hardware sidetone level directly (`0..=3`, `0x39`).
pub(crate) fn build_sidetone_level_report(level: u8) -> [u8; HID_MSG_SIZE] {
    build_hid_command_report(SIDETONE_OPCODE, &[level.min(MAX_SIDETONE_LEVEL)])
}

/// Builds the 64-byte HID report for setting microphone gain (`0..=15`, `0x37` / `0x0937`).
pub(crate) fn build_mic_gain_report(level: u8) -> [u8; HID_MSG_SIZE] {
    build_hid_command_report(MIC_GAIN_OPCODE, &[level.min(MAX_MIC_GAIN_LEVEL)])
}

/// Builds the 64-byte HID report for toggling the hardware volume limiter (`0x3A` / `0x093A`).
pub(crate) fn build_volume_limiter_report(enabled: bool) -> [u8; HID_MSG_SIZE] {
    build_hid_command_report(VOLUME_LIMITER_OPCODE, &[u8::from(enabled)])
}

/// Builds the 64-byte HID report for setting the inactivity auto-shutoff timer in minutes (`0xA3` / `0x09A3`).
pub(crate) fn build_inactivity_timer_report(minutes: u8) -> [u8; HID_MSG_SIZE] {
    build_hid_command_report(INACTIVITY_TIMER_OPCODE, &[minutes.min(MAX_INACTIVITY_MINUTES)])
}

/// Builds the 64-byte HID report for setting the mic mute LED brightness (`0..=3`, `0xAE` / `0x09AE`).
pub(crate) fn build_mute_led_brightness_report(brightness: u8) -> [u8; HID_MSG_SIZE] {
    build_hid_command_report(MUTE_LED_OPCODE, &[brightness.min(MAX_MUTE_LED_BRIGHTNESS)])
}

/// Builds the 64-byte HID report for Bluetooth auto-power-on default (`0xB2` / `0x09B2`).
pub(crate) fn build_bt_power_default_report(enabled: bool) -> [u8; HID_MSG_SIZE] {
    build_hid_command_report(BT_POWER_DEFAULT_OPCODE, &[u8::from(enabled)])
}

/// Builds the 64-byte HID report for Bluetooth call mixing behavior (`0xB3` / `0x09B3`).
pub(crate) fn build_bt_call_behavior_report(behavior: BtCallBehavior) -> [u8; HID_MSG_SIZE] {
    build_hid_command_report(BT_CALL_BEHAVIOR_OPCODE, &[behavior as u8])
}

/// Parses an incoming HID interrupt buffer for a ChatMix event (`0x45`).
/// Returns `Some((game_volume, chat_volume))` clamped to `0..=100` if valid.
pub(crate) fn parse_chatmix_report(buf: &[u8]) -> Option<(u8, u8)> {
    if buf.len() >= 3 && buf[0] == CHATMIX_OPCODE {
        let game_vol = buf[1].min(MAX_VOLUME_PERCENT);
        let chat_vol = buf[2].min(MAX_VOLUME_PERCENT);
        Some((game_vol, chat_vol))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_supported_device() {
        assert_eq!(SUPPORTED_PRODUCT_IDS.len(), 23);
        for &pid in SUPPORTED_PRODUCT_IDS {
            assert!(is_supported_device(VENDOR_ID, pid));
        }
        assert!(!is_supported_device(0x0000, 0x2202));
        assert!(!is_supported_device(VENDOR_ID, 0x9999));
    }

    #[test]
    fn test_battery_format_partition() {
        assert_eq!(PERCENTAGE_BATTERY_PRODUCT_IDS.len(), 12);
        assert_eq!(DISCRETE_BATTERY_PRODUCT_IDS.len(), 11);
        assert_eq!(
            PERCENTAGE_BATTERY_PRODUCT_IDS.len() + DISCRETE_BATTERY_PRODUCT_IDS.len(),
            SUPPORTED_PRODUCT_IDS.len()
        );

        for &pid in PERCENTAGE_BATTERY_PRODUCT_IDS {
            assert!(is_percentage_battery_device(pid));
            assert!(!is_discrete_battery_device(pid));
        }

        for &pid in DISCRETE_BATTERY_PRODUCT_IDS {
            assert!(is_discrete_battery_device(pid));
            assert!(!is_percentage_battery_device(pid));
        }

        // Specifically test the 3 newly added product IDs
        assert!(is_discrete_battery_device(0x22a4));
        assert!(is_percentage_battery_device(0x22a5));
        assert!(is_percentage_battery_device(0x22ad));
    }

    #[test]
    fn test_device_name() {
        assert_eq!(device_name(0x22a4), "Arctis Nova 7X");
        assert_eq!(device_name(0x22a5), "Arctis Nova 7X");
        assert_eq!(device_name(0x22ad), "Arctis Nova 7X v2");
        assert_eq!(device_name(0x2200), "Arctis Nova 7");
        assert_eq!(device_name(0x9999), "Arctis Nova 7");
    }

    #[test]
    fn test_sidetone_bucket_boundaries() {
        assert_eq!(sidetone_bucket(0), 0x00);
        assert_eq!(sidetone_bucket(29), 0x00);
        assert_eq!(sidetone_bucket(30), 0x01);
        assert_eq!(sidetone_bucket(59), 0x01);
        assert_eq!(sidetone_bucket(60), 0x02);
        assert_eq!(sidetone_bucket(79), 0x02);
        assert_eq!(sidetone_bucket(80), 0x03);
        assert_eq!(sidetone_bucket(100), 0x03);
        assert_eq!(sidetone_bucket(255), 0x03);
    }

    #[test]
    fn test_build_sidetone_report() {
        let report = build_sidetone_report(65);
        assert_eq!(report.len(), HID_MSG_SIZE);
        assert_eq!(report[0], 0x00);
        assert_eq!(report[1], SIDETONE_OPCODE);
        assert_eq!(report[2], 0x02);
        assert!(report[3..].iter().all(|&b| b == 0));
    }

    #[test]
    fn test_parse_chatmix_report_valid_and_clamped() {
        let buf = [0x45, 100, 42, 0, 0];
        assert_eq!(parse_chatmix_report(&buf), Some((100, 42)));

        // Values above 100 should be safely clamped to 100
        let overflow_buf = [0x45, 150, 255];
        assert_eq!(parse_chatmix_report(&overflow_buf), Some((100, 100)));
    }

    #[test]
    fn test_parse_chatmix_report_invalid() {
        assert_eq!(parse_chatmix_report(&[]), None);
        assert_eq!(parse_chatmix_report(&[0x45]), None);
        assert_eq!(parse_chatmix_report(&[0x45, 100]), None);
        assert_eq!(parse_chatmix_report(&[0x44, 100, 100]), None);
    }
}
