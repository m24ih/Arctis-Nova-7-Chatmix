use crate::config::AppConfig;
use crate::device::{
    build_bt_call_behavior_report, build_bt_power_default_report, build_inactivity_timer_report,
    build_mic_gain_report, build_mute_led_brightness_report, build_save_to_flash_report,
    build_sidetone_level_report, build_sidetone_report, build_status_request_report,
    build_volume_limiter_report, is_percentage_battery_device, is_supported_device,
    sidetone_bucket, HID_CLASS_CODE, HID_MSG_SIZE, MAX_VOLUME_PERCENT, SUPPORTED_PRODUCT_IDS,
    VENDOR_ID,
};
use crate::eq::ParametricEqProfile;
use crate::telemetry::{parse_telemetry_event, TelemetryEvent};
use anyhow::{Context, Result};
use hidapi::{HidApi, HidDevice};
use log::{debug, info};
use rusb::{DeviceHandle, Direction, TransferType, UsbContext};
use std::env;
use std::time::Duration;

const CLAIM_RETRIES: usize = 5;
const CLAIM_RETRY_DELAY: Duration = Duration::from_millis(200);
const HID_INTER_CMD_DELAY: Duration = Duration::from_millis(15);

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

pub(crate) const VENDOR_USAGE_PAGE: u16 = 0xFFC0;

/// Opens the connected supported Arctis Nova 7 vendor HID device (`usage_page == 0xFFC0`) via `hidapi`
/// and returns `(HidDevice, product_id)`. Falls back to `api.open(VENDOR_ID, pid)` if `usage_page`
/// metadata is not populated by the OS backend.
pub(crate) fn open_hidapi_device() -> Result<(HidDevice, u16)> {
    let api = HidApi::new().context("Failed to initialize HidApi")?;

    // Prefer the vendor-defined HID interface (usage_page 0xFFC0, typically interface 3 / hidraw4)
    for info in api.device_list() {
        let vid = info.vendor_id();
        let pid = info.product_id();
        if is_supported_device(vid, pid) && info.usage_page() == VENDOR_USAGE_PAGE {
            if let Ok(dev) = info.open_device(&api) {
                return Ok((dev, pid));
            }
        }
    }

    // Fallback: open first matching device by VID:PID
    for &pid in SUPPORTED_PRODUCT_IDS {
        if let Ok(dev) = api.open(VENDOR_ID, pid) {
            return Ok((dev, pid));
        }
    }
    Err(anyhow::anyhow!(
        "Failed to open any supported Arctis Nova 7 device via hidapi"
    ))
}

/// Sends a single 64-byte HID command report to the headset via `hidapi`.
pub(crate) fn hidapi_send_report(report: &[u8; HID_MSG_SIZE]) -> Result<()> {
    let (device, _pid) = open_hidapi_device()?;
    device.write(report).context("hidapi write failed")?;
    Ok(())
}

/// Sends multiple 64-byte HID command reports sequentially over a single `hidapi` handle.
pub(crate) fn hidapi_send_reports(reports: &[[u8; HID_MSG_SIZE]]) -> Result<()> {
    let (device, _pid) = open_hidapi_device()?;
    for (idx, report) in reports.iter().enumerate() {
        device
            .write(report)
            .with_context(|| format!("hidapi write failed on report #{}", idx + 1))?;
        if idx + 1 < reports.len() {
            std::thread::sleep(HID_INTER_CMD_DELAY);
        }
    }
    Ok(())
}

/// Requests general headset status (`0xB0`) via `hidapi` and attempts a non-blocking/short read
/// if the response is delivered on the same handle.
pub(crate) fn hidapi_request_status() -> Result<Option<TelemetryEvent>> {
    let (device, pid) = open_hidapi_device()?;
    let req = build_status_request_report();
    device
        .write(&req)
        .context("Failed to write 0xB0 status request")?;

    let mut buf = [0u8; HID_MSG_SIZE];
    if let Ok(len) = device.read_timeout(&mut buf, 120) {
        if len > 0 {
            return Ok(parse_telemetry_event(
                &buf[..len],
                is_percentage_battery_device(pid),
            ));
        }
    }
    Ok(None)
}

/// Persists current DSP and hardware settings to headset onboard flash (`0x09`).
pub(crate) fn hidapi_save_to_flash() -> Result<()> {
    let report = build_save_to_flash_report();
    hidapi_send_report(&report)?;
    info!("Saved active hardware & EQ configuration to headset flash (0x09)");
    Ok(())
}

/// Applies all configured hardware controls (`0x37`, `0x39`, `0x3A`, `0xA3`, `0xAE`, `0xB2`, `0xB3`),
/// the active 10-band Parametric EQ profile (`0x33`), and optional flash save (`0x09`).
pub(crate) fn hidapi_apply_config_and_eq(
    config: &AppConfig,
    eq_profile: Option<&ParametricEqProfile>,
    force_flash: bool,
) -> Result<()> {
    let mut reports: Vec<[u8; HID_MSG_SIZE]> = Vec::with_capacity(10);

    // 1. Sidetone (env var overrides config.toml if ARCTIS_SIDETONE_PERCENT / DISABLE is set)
    let disable_env = env::var("ARCTIS_SIDETONE_DISABLE").ok();
    let percent_env = env::var("ARCTIS_SIDETONE_PERCENT").ok();
    if disable_env.as_deref().map(str::trim) == Some("1") {
        debug!("Sidetone disabled via ARCTIS_SIDETONE_DISABLE=1");
    } else if let Some(percent) =
        resolve_sidetone_percent(disable_env.as_deref(), percent_env.as_deref())
    {
        reports.push(build_sidetone_report(percent));
    } else if config.hardware.sidetone_enabled {
        reports.push(build_sidetone_level_report(config.hardware.sidetone_level));
    } else {
        reports.push(build_sidetone_level_report(0));
    }

    // 2. Microphone gain (0x37)
    reports.push(build_mic_gain_report(config.hardware.mic_gain));

    // 3. Volume limiter (0x3A)
    reports.push(build_volume_limiter_report(config.hardware.volume_limiter));

    // 4. Inactivity timer (0xA3)
    reports.push(build_inactivity_timer_report(
        config.hardware.inactivity_timer_mins,
    ));

    // 5. Mute LED brightness (0xAE)
    reports.push(build_mute_led_brightness_report(
        config.hardware.mute_led_brightness,
    ));

    // 6. Bluetooth power-on default (0xB2) & call behavior (0xB3)
    reports.push(build_bt_power_default_report(
        config.hardware.bt_power_on_default,
    ));
    reports.push(build_bt_call_behavior_report(
        config.hardware.bt_call_behavior,
    ));

    // 7. 10-band Parametric EQ (0x33)
    if let Some(profile) = eq_profile {
        reports.push(profile.build_hid_report());
        info!(
            "Applying 10-band EQ profile '{}' ({})",
            profile.name, profile.description
        );
    }

    // 8. Initial 0xB0 status request so telemetry populates immediately
    reports.push(build_status_request_report());

    // 9. Persistent flash save (0x09) if enabled in config or explicitly requested
    if config.save_to_flash || force_flash {
        reports.push(build_save_to_flash_report());
    }

    hidapi_send_reports(&reports)?;
    Ok(())
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

pub(crate) fn hidapi_send_sidetone(percent: u8) -> Result<()> {
    let data = build_sidetone_report(percent);
    let bucket = sidetone_bucket(percent);
    hidapi_send_report(&data)?;
    info!("Sidetone updated to bucket {}", bucket);
    Ok(())
}

/* ---------- rusb HID Control Transfer ---------- */
/// Sends a 64-byte HID command report over an already-claimed `rusb::DeviceHandle`
/// using USB HID Class `SET_REPORT` (`bmRequestType = 0x21`, `bRequest = 0x09`, `wValue = 0x0200`).
pub(crate) fn rusb_send_hid_report<T: UsbContext>(
    handle: &DeviceHandle<T>,
    interface_num: u8,
    report: &[u8; HID_MSG_SIZE],
) -> Result<()> {
    // Strip leading 0x00 report ID byte when sending via control transfer wValue=0x0200
    let payload = if report[0] == 0x00 {
        &report[1..]
    } else {
        &report[..]
    };
    handle
        .write_control(
            0x21,
            0x09,
            0x0200,
            u16::from(interface_num),
            payload,
            Duration::from_millis(250),
        )
        .context("rusb HID SET_REPORT control transfer failed")?;
    Ok(())
}

/// Sends a `0xB0` general status query over an active `rusb::DeviceHandle`.
/// The response arrives asynchronously in `read_loop` via `read_interrupt`.
pub(crate) fn rusb_request_status<T: UsbContext>(
    handle: &DeviceHandle<T>,
    interface_num: u8,
) -> Result<()> {
    let req = build_status_request_report();
    rusb_send_hid_report(handle, interface_num, &req)
}

/* ---------- USB Finder ---------- */
pub(crate) fn usb_find_and_open<T: UsbContext>(
    usb_ctx: &T,
    startup_config: Option<(&AppConfig, Option<&ParametricEqProfile>)>,
) -> Result<(DeviceHandle<T>, u8, u8, u16)> {
    let mut matched_pid = 0u16;
    let dev = usb_ctx
        .devices()?
        .iter()
        .find(|d| {
            d.device_descriptor()
                .map(|desc| {
                    let ok = is_supported_device(desc.vendor_id(), desc.product_id());
                    if ok {
                        matched_pid = desc.product_id();
                    }
                    ok
                })
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

    // Apply XDG hardware config + 10-band EQ via hidapi while hidraw is still attached
    if let Some((app_cfg, eq_prof)) = startup_config {
        if app_cfg.auto_apply_on_connect {
            if let Err(e) = hidapi_apply_config_and_eq(app_cfg, eq_prof, false) {
                debug!("Failed to auto-apply config/EQ via hidapi: {}", e);
                try_hidapi_sidetone_from_env();
            }
        } else {
            try_hidapi_sidetone_from_env();
        }
    } else {
        try_hidapi_sidetone_from_env();
    }

    // Kernel Driver Detach
    let _ = handle.set_auto_detach_kernel_driver(true);
    if let Ok(true) = handle.kernel_driver_active(interface_num) {
        let _ = handle.detach_kernel_driver(interface_num);
    }

    // Claim Interface with Retry
    for _ in 1..=CLAIM_RETRIES {
        if handle.claim_interface(interface_num).is_ok() {
            return Ok((handle, target_endpoint, interface_num, matched_pid));
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
