// Arctis Nova 7 Vendor/Product IDs
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
    0x227a  // Arctis Nova 7 WoW Edition (discrete battery: 0-4)
];
pub(crate) const HID_MSG_SIZE: usize = 64;
