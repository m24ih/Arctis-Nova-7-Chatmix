// Arctis Nova 7 hardware, protocol, and virtual sink definitions.

pub(crate) const VENDOR_ID: u16 = 0x1038;

pub(crate) const SUPPORTED_PRODUCT_IDS: &[u16] = &[
    0x2202, // Arctis Nova 7 (discrete battery: 0-4)
    0x22A1, // Arctis Nova 7 Gen 2 (percentage battery: 0-100, Jan 2026 update)
    0x227e, // Arctis Nova 7 Wireless Gen 2 (percentage battery: 0-100)
    0x2206, // Arctis Nova 7x (discrete battery: 0-4)
    0x2258, // Arctis Nova 7x v2 (percentage battery: 0-100)
    0x229e, // Arctis Nova 7x v2 (percentage battery: 0-100)
    0x223a, // Arctis Nova 7 Diablo IV (discrete battery: 0-4, before Jan 2026 update)
    0x22a9, // Arctis Nova 7 Diablo IV (percentage battery: 0-100, after Jan 2026 update)
    0x227a, // Arctis Nova 7 WoW Edition (discrete battery: 0-4)
];

pub(crate) const HID_MSG_SIZE: usize = 64;
pub(crate) const HID_CLASS_CODE: u8 = 3;

pub(crate) const CHATMIX_OPCODE: u8 = 0x45;
pub(crate) const SIDETONE_OPCODE: u8 = 0x39;
pub(crate) const MAX_VOLUME_PERCENT: u8 = 100;

pub(crate) const GAME_SINK_NAME: &str = "Arctis_Game";
pub(crate) const GAME_SINK_DESC: &str = "Arctis Nova 7 Game";
pub(crate) const CHAT_SINK_NAME: &str = "Arctis_Chat";
pub(crate) const CHAT_SINK_DESC: &str = "Arctis Nova 7 Chat";
pub(crate) const FALLBACK_DEFAULT_SINK: &str = "auto_null";

/// Returns `true` if the given USB vendor and product IDs match a supported Arctis Nova 7 model.
pub(crate) fn is_supported_device(vendor_id: u16, product_id: u16) -> bool {
    vendor_id == VENDOR_ID && SUPPORTED_PRODUCT_IDS.contains(&product_id)
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

/// Builds the 64-byte HID report for setting the hardware sidetone level.
pub(crate) fn build_sidetone_report(percent: u8) -> [u8; HID_MSG_SIZE] {
    let mut data = [0u8; HID_MSG_SIZE];
    data[0] = 0x00;
    data[1] = SIDETONE_OPCODE;
    data[2] = sidetone_bucket(percent.min(MAX_VOLUME_PERCENT));
    data
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
        for &pid in SUPPORTED_PRODUCT_IDS {
            assert!(is_supported_device(VENDOR_ID, pid));
        }
        assert!(!is_supported_device(0x0000, 0x2202));
        assert!(!is_supported_device(VENDOR_ID, 0x9999));
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
