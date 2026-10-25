use crate::device::{CHAT_SINK_NAME, GAME_SINK_NAME, MAX_VOLUME_PERCENT};
use anyhow::{Context, Result};
use log::debug;
use std::process::{Command, Stdio};

const STEREO_CHANNELS: [&str; 2] = ["FL", "FR"];

/// Queries PulseAudio/PipeWire for the current default sink name.
pub(crate) fn get_default_sink() -> Result<String> {
    let output = Command::new("pactl")
        .arg("get-default-sink")
        .output()
        .context("Failed to execute pactl get-default-sink")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("pactl get-default-sink failed: {}", stderr.trim());
    }

    let sink = String::from_utf8(output.stdout)?.trim().to_string();
    if sink.is_empty() {
        anyhow::bail!("Empty default sink");
    }
    Ok(sink)
}

/// Sets the system default audio sink via `pactl set-default-sink`.
pub(crate) fn set_default_sink(sink_name: &str) -> Result<()> {
    let status = Command::new("pactl")
        .args(["set-default-sink", sink_name])
        .status()
        .with_context(|| format!("Failed to set default sink to '{}'", sink_name))?;

    if !status.success() {
        anyhow::bail!("pactl set-default-sink exited with status {}", status);
    }
    Ok(())
}

/// Parses `pactl list short sinks` output to find the physical Arctis Nova 7 sink.
/// Inspects only the sink name column (column 1) so numeric sink indices (e.g. `57`)
/// or sample specs cannot cause false positives, and ignores our own virtual sinks.
/// Prefers entries whose sink name contains `"usb"`, `"playback"`, `"dot"`, or `"wireless"`.
pub(crate) fn parse_arctis_sink(sinks_output: &str) -> Option<String> {
    let mut fallback: Option<String> = None;

    for line in sinks_output.lines() {
        let Some(raw_name) = line.split('\t').nth(1) else {
            continue;
        };
        let name = raw_name.trim();
        if name.is_empty()
            || name.eq_ignore_ascii_case(GAME_SINK_NAME)
            || name.eq_ignore_ascii_case(CHAT_SINK_NAME)
        {
            continue;
        }

        let lower_name = name.to_lowercase();
        // Match on "nova" or "7" in addition to "arctis" within the sink name itself
        if lower_name.contains("arctis")
            && (lower_name.contains("7") || lower_name.contains("nova"))
        {
            if lower_name.contains("usb")
                || lower_name.contains("playback")
                || lower_name.contains("dot")
                || lower_name.contains("wireless")
            {
                return Some(name.to_string());
            }
            if fallback.is_none() {
                fallback = Some(name.to_string());
            }
        }
    }

    fallback
}

/// Finds the physical Arctis Nova 7 sink name in PipeWire/PulseAudio.
pub(crate) fn find_arctis_sink() -> Result<String> {
    let output = Command::new("pactl")
        .args(["list", "short", "sinks"])
        .output()
        .context("Failed to list sinks")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("pactl list short sinks failed: {}", stderr.trim());
    }

    let sinks = String::from_utf8(output.stdout)?;
    parse_arctis_sink(&sinks)
        .ok_or_else(|| anyhow::anyhow!("No Arctis Nova 7 device found in pactl output"))
}

/// Formats the PipeWire node properties string for a stereo virtual null-audio-sink.
pub(crate) fn format_null_sink_args(node_name: &str, description: &str) -> String {
    format!(
        r#"{{factory.name=support.null-audio-sink node.name={} node.description="{}" media.class=Audio/Sink monitor.channel-volumes=true object.linger=true audio.position=[FL FR]}}"#,
        node_name, description
    )
}

/// Creates a PipeWire virtual null-audio-sink with the given node name and description.
pub(crate) fn create_virtual_sink(node_name: &str, description: &str) -> Result<()> {
    let props = format_null_sink_args(node_name, description);
    let status = Command::new("pw-cli")
        .args(["create-node", "adapter", &props])
        .stdout(Stdio::null())
        .status()
        .with_context(|| format!("Failed to execute pw-cli create-node for {}", node_name))?;

    if !status.success() {
        anyhow::bail!("Failed to create {} sink", node_name);
    }
    Ok(())
}

/// Destroys a PipeWire node by name, suppressing stdout/stderr.
pub(crate) fn destroy_virtual_sink(node_name: &str) {
    let _ = Command::new("pw-cli")
        .args(["destroy", node_name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Destroys both `Arctis_Game` and `Arctis_Chat` virtual sinks.
pub(crate) fn destroy_virtual_sinks() {
    destroy_virtual_sink(GAME_SINK_NAME);
    destroy_virtual_sink(CHAT_SINK_NAME);
}

/// Returns `true` if `pw-link` stderr indicates the link already exists.
pub(crate) fn is_link_exists_stderr(stderr: &str) -> bool {
    stderr.to_ascii_lowercase().contains("exists")
}

/// Idempotently links a virtual sink's `FL`/`FR` monitor ports to the physical device's playback ports.
pub(crate) fn link_sink_to_device(sink_name: &str, device_name: &str) -> Result<()> {
    for channel in STEREO_CHANNELS {
        let src = format!("{}:monitor_{}", sink_name, channel);
        let dst = format!("{}:playback_{}", device_name, channel);

        let output = Command::new("pw-link")
            .arg(&src)
            .arg(&dst)
            .output()
            .with_context(|| format!("Failed to execute pw-link for {}", channel))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // If the link already exists, treat it as success rather than an error.
            if is_link_exists_stderr(&stderr) {
                debug!("Link already exists (skipping): {} -> {}", src, dst);
            } else {
                anyhow::bail!("pw-link failed for {}: {}", channel, stderr.trim());
            }
        }
    }
    Ok(())
}

/// Sets the volume percentage (`0..=100`) on a sink via `pactl set-sink-volume`.
/// Returns `true` if `pactl` exited successfully.
pub(crate) fn set_sink_volume(sink_name: &str, volume_percent: u8) -> bool {
    let clamped = volume_percent.min(MAX_VOLUME_PERCENT);
    Command::new("pactl")
        .args(["set-sink-volume", sink_name, &format!("{}%", clamped)])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Parses `pactl list short sink-inputs` output into a list of stream index IDs.
pub(crate) fn parse_sink_input_indices(sink_inputs_output: &str) -> Vec<u32> {
    sink_inputs_output
        .lines()
        .filter_map(|line| {
            let first_col = line.split('\t').next()?.trim();
            first_col.parse::<u32>().ok()
        })
        .collect()
}

/// Moves all currently active audio streams (sink-inputs) to the specified target sink.
pub(crate) fn move_all_inputs_to(sink_name: &str) -> Result<()> {
    let output = Command::new("pactl")
        .args(["list", "short", "sink-inputs"])
        .output()
        .context("Failed to list sink-inputs")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("pactl list short sink-inputs failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8(output.stdout)?;
    for index in parse_sink_input_indices(&stdout) {
        // Ignore individual move errors — some streams (e.g. system monitors) may not be movable.
        let _ = Command::new("pactl")
            .args(["move-sink-input", &index.to_string(), sink_name])
            .status();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_arctis_sink_prefers_usb_over_fallback() {
        let sample = "\
42\talsa_output.pci-0000_00_1f.3.analog-stereo\tPipeWire\ts32le 2ch 48000Hz\tSUSPENDED
55\tArctis_Game\tPipeWire\tfloat32le 2ch 48000Hz\tIDLE
56\tArctis_Chat\tPipeWire\tfloat32le 2ch 48000Hz\tIDLE
61\talsa_output.steelseries_arctis_nova_7\tPipeWire\ts16le 2ch 48000Hz\tIDLE
62\talsa_output.usb-SteelSeries_Arctis_Nova_7-00.analog-stereo\tPipeWire\ts16le 2ch 48000Hz\tRUNNING
";
        assert_eq!(
            parse_arctis_sink(sample),
            Some("alsa_output.usb-SteelSeries_Arctis_Nova_7-00.analog-stereo".to_string())
        );
    }

    #[test]
    fn test_parse_arctis_sink_fallback_and_ignores_virtual_and_non_nova7_sinks() {
        let only_virtual_or_non_7_at_index_57 = "\
7\tArctis_Game\tPipeWire\tfloat32le 2ch 48000Hz\tIDLE
17\tArctis_Chat\tPipeWire\tfloat32le 2ch 48000Hz\tIDLE
57\talsa_output.usb-SteelSeries_Arctis_5-00.analog-stereo\tPipeWire\ts16le 2ch 48000Hz\tRUNNING
77\talsa_output.usb-SteelSeries_Arctis_Pro-00.analog-stereo\tPipeWire\ts16le 2ch 48000Hz\tIDLE
";
        assert_eq!(parse_arctis_sink(only_virtual_or_non_7_at_index_57), None);

        let fallback_only = "\
61\talsa_output.steelseries_arctis_7_custom\tPipeWire\ts16le 2ch 48000Hz\tIDLE
";
        assert_eq!(
            parse_arctis_sink(fallback_only),
            Some("alsa_output.steelseries_arctis_7_custom".to_string())
        );
    }

    #[test]
    fn test_parse_sink_input_indices() {
        let sample = "\
104\t55\t101\tPipeWire\tfloat32le 2ch 48000Hz
208\t62\t145\tPipeWire\ts16le 2ch 48000Hz
invalid\t62\t145\tPipeWire\ts16le 2ch 48000Hz

";
        assert_eq!(parse_sink_input_indices(sample), vec![104, 208]);
        assert!(parse_sink_input_indices("").is_empty());
    }

    #[test]
    fn test_format_null_sink_args() {
        let args = format_null_sink_args("Arctis_Game", "Arctis Nova 7 Game");
        assert_eq!(
            args,
            r#"{factory.name=support.null-audio-sink node.name=Arctis_Game node.description="Arctis Nova 7 Game" media.class=Audio/Sink monitor.channel-volumes=true object.linger=true audio.position=[FL FR]}"#
        );
    }

    #[test]
    fn test_is_link_exists_stderr() {
        assert!(is_link_exists_stderr("failed to link ports: File exists"));
        assert!(is_link_exists_stderr("link already EXISTS"));
        assert!(!is_link_exists_stderr("No such port"));
    }
}
