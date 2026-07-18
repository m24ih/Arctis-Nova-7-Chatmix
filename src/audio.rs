use anyhow::{Context, Result};
use log::debug;
use std::process::Command;

pub(crate) fn get_default_sink() -> Result<String> {
    let output = Command::new("pactl")
        .arg("get-default-sink")
        .output()
        .context("Failed to get default sink")?;
    let sink = String::from_utf8(output.stdout)?.trim().to_string();
    if sink.is_empty() {
        anyhow::bail!("Empty default sink");
    }
    Ok(sink)
}

pub(crate) fn find_arctis_sink() -> Result<String> {
    let output = Command::new("pactl")
        .args(&["list", "short", "sinks"])
        .output()
        .context("Failed to list sinks")?;

    let sinks = String::from_utf8(output.stdout)?;
    let mut fallback: Option<String> = None;

    for line in sinks.lines() {
        let lower = line.to_lowercase();
        // Match on "nova" or "7" in addition to "arctis"
        if lower.contains("arctis") && (lower.contains("7") || lower.contains("nova")) {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 2 {
                let name = parts[1].to_string();
                if lower.contains("usb") || lower.contains("playback") {
                    return Ok(name);
                }
                if fallback.is_none() {
                    fallback = Some(name);
                }
            }
        }
    }

    if let Some(f) = fallback {
        return Ok(f);
    }
    anyhow::bail!("No Arctis Nova 7 device found in pactl output");
}

// FIX: Idempotent Link Creation
pub(crate) fn link_sink_to_device(sink_name: &str, device_name: &str) -> Result<()> {
    for channel in ["FL", "FR"] {
        let src = format!("{}:monitor_{}", sink_name, channel);
        let dst = format!("{}:playback_{}", device_name, channel);

        let output = Command::new("pw-link")
            .arg(&src)
            .arg(&dst)
            .output()
            .context(format!("Failed to execute pw-link for {}", channel))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // If the link already exists, treat it as success rather than an error.
            if stderr.contains("File exists") || stderr.contains("exists") {
                debug!("Link already exists (skipping): {} -> {}", src, dst);
            } else {
                anyhow::bail!("pw-link failed for {}: {}", channel, stderr.trim());
            }
        }
    }
    Ok(())
}

pub(crate) fn set_sink_volume(sink_name: &str, volume_percent: u8) {
    let _ = Command::new("pactl")
        .args(&["set-sink-volume", sink_name, &format!("{}%", volume_percent)])
        .output();
}

pub(crate) fn move_all_inputs_to(sink_name: &str) -> Result<()> {
    let output = Command::new("pactl")
        .args(&["list", "short", "sink-inputs"])
        .output()
        .context("Failed to list sink-inputs")?;

    let stdout = String::from_utf8(output.stdout)?;
    for line in stdout.lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        if !cols.is_empty() {
            if let Ok(index) = cols[0].parse::<u32>() {
                // Ignore errors — some inputs may not be movable.
                let _ = Command::new("pactl")
                    .args(&["move-sink-input", &index.to_string(), sink_name])
                    .status();
            }
        }
    }
    Ok(())
}
