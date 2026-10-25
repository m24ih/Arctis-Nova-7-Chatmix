use crate::device::{
    build_sidetone_report, is_supported_device, sidetone_bucket, HID_CLASS_CODE,
    MAX_VOLUME_PERCENT, SUPPORTED_PRODUCT_IDS, VENDOR_ID,
};
use anyhow::{Context, Result};
use hidapi::HidApi;
use log::{debug, info};
use rusb::{DeviceHandle, Direction, TransferType, UsbContext};
use std::env;
use std::time::Duration;

const CLAIM_RETRIES: usize = 5;
const CLAIM_RETRY_DELAY: Duration = Duration::from_millis(200);

/// Resolves the desired sidetone percentage (`0..=100`) from environment variable values.
/// Returns `None` if disabled (`ARCTIS_SIDETONE_DISABLE=1`), unset, or invalid.
pub(crate) fn resolve_sidetone_percent(
    disable_env: Option<&str>,
    percent_env: Option<&str>,
) -> Option<u8> {
    if disable_env.map(str::trim) == Some("1") {
        return None;
    }
    let raw = percent_env?.trim();
    let parsed = raw.parse::<u8>().ok()?;
    Some(parsed.min(MAX_VOLUME_PERCENT))
}

/// Selects the target `(interface_number, endpoint_address)` from an iterator of
/// `(interface_number, class_code, interrupt_in_endpoint)` tuples.
///
/// Iterates through all descriptors and returns the **last** HID interface
/// (`class_code == HID_CLASS_CODE`) that has an Interrupt IN endpoint, matching the
/// composite USB layout of Arctis Nova 7 dongles (e.g. `1038:22a1` exposes HID
/// interfaces 3, 4, and 5, where interface 5 / `0x86` is the final vendor HID interface).
pub(crate) fn select_hid_interface<I>(interfaces: I) -> Option<(u8, u8)>
where
    I: IntoIterator<Item = (u8, u8, Option<u8>)>,
{
    let mut target = None;
    for (interface_num, class_code, interrupt_in_ep) in interfaces {
        if class_code == HID_CLASS_CODE {
            if let Some(endpoint_addr) = interrupt_in_ep {
                target = Some((interface_num, endpoint_addr));
            }
        }
    }
    target
}

/* ---------- hidapi sidetone write ---------- */
fn try_hidapi_sidetone_from_env() {
    let disable_env = env::var("ARCTIS_SIDETONE_DISABLE").ok();
    if disable_env.as_deref().map(str::trim) == Some("1") {
        debug!("Sidetone disabled via ARCTIS_SIDETONE_DISABLE=1, skipping.");
        return;
    }

    let percent_env = env::var("ARCTIS_SIDETONE_PERCENT").ok();
    if let Some(percent) =
        resolve_sidetone_percent(disable_env.as_deref(), percent_env.as_deref())
    {
        if let Err(e) = hidapi_send_sidetone(percent) {
            debug!("Failed to apply sidetone via hidapi: {}", e);
        }
    }
}

fn hidapi_send_sidetone(percent: u8) -> Result<()> {
    let data = build_sidetone_report(percent);
    let bucket = sidetone_bucket(percent);

    let api = HidApi::new()?;
    let device = SUPPORTED_PRODUCT_IDS
        .iter()
        .find_map(|&pid| api.open(VENDOR_ID, pid).ok())
        .context("Failed to open any supported Arctis Nova 7 device for sidetone")?;

    device.write(&data)?;
    info!("Sidetone updated to bucket {}", bucket);
    Ok(())
}

/* ---------- USB Finder ---------- */
pub(crate) fn usb_find_and_open<T: UsbContext>(usb_ctx: &T) -> Result<(DeviceHandle<T>, u8, u8)> {
    let dev = usb_ctx
        .devices()?
        .iter()
        .find(|d| {
            d.device_descriptor()
                .map(|desc| is_supported_device(desc.vendor_id(), desc.product_id()))
                .unwrap_or(false)
        })
        .ok_or_else(|| anyhow::anyhow!("Arctis Nova 7 not found"))?;

    let config = dev.config_descriptor(0)?;
    let candidates = config.interfaces().filter_map(|interface| {
        let desc = interface.descriptors().next()?;
        let interrupt_in_ep = desc
            .endpoint_descriptors()
            .find(|endpoint| {
                endpoint.transfer_type() == TransferType::Interrupt
                    && endpoint.direction() == Direction::In
            })
            .map(|endpoint| endpoint.address());
        Some((desc.interface_number(), desc.class_code(), interrupt_in_ep))
    });

    let (interface_num, target_endpoint) = select_hid_interface(candidates)
        .ok_or_else(|| anyhow::anyhow!("Could not find HID interface"))?;
    let handle = dev.open().context("Failed to open USB device")?;

    try_hidapi_sidetone_from_env();

    // Kernel Driver Detach
    let _ = handle.set_auto_detach_kernel_driver(true);
    if let Ok(true) = handle.kernel_driver_active(interface_num) {
        let _ = handle.detach_kernel_driver(interface_num);
    }

    // Claim Interface with Retry
    for _ in 1..=CLAIM_RETRIES {
        if handle.claim_interface(interface_num).is_ok() {
            return Ok((handle, target_endpoint, interface_num));
        }
        std::thread::sleep(CLAIM_RETRY_DELAY);
    }

    Err(anyhow::anyhow!("Failed to claim interface"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_sidetone_percent() {
        assert_eq!(resolve_sidetone_percent(Some("1"), Some("50")), None);
        assert_eq!(resolve_sidetone_percent(Some(" 1 "), Some("50")), None);
        assert_eq!(resolve_sidetone_percent(None, None), None);
        assert_eq!(resolve_sidetone_percent(None, Some("")), None);
        assert_eq!(resolve_sidetone_percent(None, Some("abc")), None);
        assert_eq!(resolve_sidetone_percent(None, Some("45")), Some(45));
        assert_eq!(resolve_sidetone_percent(Some("0"), Some(" 85 ")), Some(85));
        assert_eq!(resolve_sidetone_percent(None, Some("150")), Some(100));
    }

    #[test]
    fn test_select_hid_interface_prefers_last_hid_interface_on_composite_device() {
        // Real USB descriptor layout from SteelSeries Arctis Nova 7 (1038:22a1):
        // Interfaces 0..=2: Audio (class 1)
        // Interface 3: HID (class 3), EP 0x84
        // Interface 4: HID (class 3), EP 0x85 (Consumer Control)
        // Interface 5: HID (class 3), EP 0x86 (Vendor HID)
        let descriptors = vec![
            (0, 1, None),
            (1, 1, None),
            (2, 1, None),
            (3, 3, Some(0x84)),
            (4, 3, Some(0x85)),
            (5, 3, Some(0x86)),
        ];
        assert_eq!(select_hid_interface(descriptors), Some((5, 0x86)));

        // Non-HID or HID without Interrupt IN endpoint returns None
        assert_eq!(select_hid_interface(vec![(0, 1, Some(0x81)), (3, 3, None)]), None);
    }
}
