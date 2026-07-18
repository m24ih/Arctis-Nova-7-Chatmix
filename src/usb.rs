use crate::device::{HID_MSG_SIZE, SUPPORTED_PRODUCT_IDS, VENDOR_ID};
use anyhow::{Context, Result};
use hidapi::HidApi;
use log::{debug, info};
use rusb::{DeviceHandle, UsbContext};
use std::env;
use std::time::Duration;

/* ---------- hidapi sidetone write ---------- */
fn try_hidapi_sidetone_from_env() {
    // Skip sidetone if disabled via ARCTIS_SIDETONE_DISABLE=1
    if env::var("ARCTIS_SIDETONE_DISABLE").as_deref() == Ok("1") {
        debug!("Sidetone disabled via ARCTIS_SIDETONE_DISABLE=1, skipping.");
        return;
    }
    // Read sidetone level from environment variable
    if let Ok(v) = env::var("ARCTIS_SIDETONE_PERCENT") {
        if let Ok(num) = v.trim().parse::<u8>() {
            let _ = hidapi_send_sidetone(num.min(100));
        }
    }
}

fn hidapi_send_sidetone(percent: u8) -> Result<()> {
    let bucket = if percent < 30 { 0x00 } else if percent < 60 { 0x01 } else if percent < 80 { 0x02 } else { 0x03 };
    let mut data = [0u8; HID_MSG_SIZE];
    data[0] = 0x00;
    data[1] = 0x39;
    data[2] = bucket;

    let api = HidApi::new()?;
    // Try to open any of the supported devices
    let device = SUPPORTED_PRODUCT_IDS.iter().find_map(|&pid| {
        api.open(VENDOR_ID, pid).ok()
    }).context("Failed to open any supported Arctis Nova 7 device for sidetone")?;
    
    device.write(&data)?;
    info!("Sidetone updated to bucket {}", bucket);
    Ok(())
}

/* ---------- USB Finder ---------- */
pub(crate) fn usb_find_and_open<T: UsbContext>(usb_ctx: &T) -> Result<(DeviceHandle<T>, u8, u8)> {
    let dev = usb_ctx.devices()?.iter().find(|d| {
        if let Ok(desc) = d.device_descriptor() {
            desc.vendor_id() == VENDOR_ID && SUPPORTED_PRODUCT_IDS.contains(&desc.product_id())
        } else { false }
    }).ok_or_else(|| anyhow::anyhow!("Arctis Nova 7 not found"))?;

    let config = dev.config_descriptor(0)?;
    let mut target_interface_num = None;
    let mut target_endpoint = 0x84u8; // Fallback default

    for interface in config.interfaces() {
        if let Some(desc) = interface.descriptors().next() {
            if desc.class_code() == 3 { // HID Class
                for endpoint in desc.endpoint_descriptors() {
                    if endpoint.transfer_type() == rusb::TransferType::Interrupt && endpoint.direction() == rusb::Direction::In {
                        target_interface_num = Some(desc.interface_number());
                        target_endpoint = endpoint.address();
                        break;
                    }
                }
            }
        }
    }

    let interface_num = target_interface_num.ok_or_else(|| anyhow::anyhow!("Could not find HID interface"))?;
    let handle = dev.open().context("Failed to open USB device")?;

    try_hidapi_sidetone_from_env();

    // Kernel Driver Detach
    let _ = handle.set_auto_detach_kernel_driver(true);
    if let Ok(true) = handle.kernel_driver_active(interface_num) {
        let _ = handle.detach_kernel_driver(interface_num);
    }

    // Claim Interface with Retry
    const CLAIM_RETRIES: usize = 5;
    for _ in 1..=CLAIM_RETRIES {
        if handle.claim_interface(interface_num).is_ok() {
            return Ok((handle, target_endpoint, interface_num));
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    Err(anyhow::anyhow!("Failed to claim interface"))
}
