// XDG Configuration & EQ Profile Manager (`~/.config/arctis-nova-7/`).
//
// Directory layout:
// - `$XDG_CONFIG_HOME/arctis-nova-7/config.toml` (or `~/.config/arctis-nova-7/config.toml`)
// - `$XDG_CONFIG_HOME/arctis-nova-7/profiles/*.toml` (`flat.toml`, `harman.toml`, `fps.toml`, ...)

use crate::device::{
    BtCallBehavior, MAX_INACTIVITY_MINUTES, MAX_MIC_GAIN_LEVEL, MAX_MUTE_LED_BRIGHTNESS,
    MAX_SIDETONE_LEVEL,
};
use crate::eq::{
    EqBand, EqFilterType, ParametricEqProfile, DEFAULT_Q_FACTOR, EQ_BAND_COUNT, EQ_FREQUENCIES_HZ,
    MAX_EQ_GAIN_DB, MIN_EQ_GAIN_DB,
};
use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub const APP_CONFIG_DIR_NAME: &str = "arctis-nova-7";
pub const CONFIG_FILE_NAME: &str = "config.toml";
pub const PROFILES_DIR_NAME: &str = "profiles";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardwareConfig {
    pub sidetone_enabled: bool,
    pub sidetone_level: u8,
    pub mic_gain: u8,
    pub volume_limiter: bool,
    pub inactivity_timer_mins: u8,
    pub mute_led_brightness: u8,
    pub bt_power_on_default: bool,
    pub bt_call_behavior: BtCallBehavior,
}

impl Default for HardwareConfig {
    fn default() -> Self {
        Self {
            sidetone_enabled: true,
            sidetone_level: 0,
            mic_gain: 12, // ~75% prevents analog clipping on ClearCast Gen 2
            volume_limiter: false,
            inactivity_timer_mins: 30,
            mute_led_brightness: 2,
            bt_power_on_default: false,
            bt_call_behavior: BtCallBehavior::DoNothing,
        }
    }
}

pub const DEFAULT_WEB_BIND: &str = "127.0.0.1:3927";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppConfig {
    pub active_profile: String,
    pub auto_apply_on_connect: bool,
    pub save_to_flash: bool,
    pub poll_status_interval_secs: u64,
    pub web_enabled: bool,
    pub web_bind: String,
    pub hardware: HardwareConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            active_profile: "harman".to_string(),
            auto_apply_on_connect: true,
            save_to_flash: false,
            poll_status_interval_secs: 30,
            web_enabled: true,
            web_bind: DEFAULT_WEB_BIND.to_string(),
            hardware: HardwareConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn to_toml_string(&self) -> String {
        format!(
            r#"# Arctis Nova 7 Configuration (`~/.config/arctis-nova-7/config.toml`)

[general]
# Active EQ profile name inside `profiles/<name>.toml` (e.g. "flat", "harman", "fps")
active_profile = "{active_profile}"
# Automatically apply EQ and hardware settings when the USB receiver connects
auto_apply_on_connect = {auto_apply}
# Persist settings to headset onboard flash memory (0x09) when applying
save_to_flash = {save_flash}
# Interval in seconds for querying 0xB0 general status / battery telemetry (0 = disable polling)
poll_status_interval_secs = {poll_secs}
# Embedded HTTP & SSE server for Web UI / API
web_enabled = {web_enabled}
web_bind = "{web_bind}"

[hardware]
# Hardware sidetone (0x39 / 0x0939): level 0 (Off), 1 (Low), 2 (Medium), 3 (High)
sidetone_enabled = {sidetone_enabled}
sidetone_level = {sidetone_level}
# Microphone gain (0x37 / 0x0937): 0..=15 (12 = ~75%)
mic_gain = {mic_gain}
# EU Volume Limiter (0x3A / 0x093A): true or false
volume_limiter = {volume_limiter}
# Inactivity auto-shutoff timer in minutes (0xA3 / 0x09A3): 0..=90 (0 = disabled)
inactivity_timer_mins = {inactivity_mins}
# Microphone mute LED brightness (0xAE / 0x09AE): 0 (Off), 1 (Low), 2 (Med), 3 (High)
mute_led_brightness = {mute_led}
# Automatically power on Bluetooth when turning headset on (0xB2 / 0x09B2)
bt_power_on_default = {bt_power}
# Bluetooth call behavior (0xB3 / 0x09B3): "do_nothing", "attenuate_12db", or "mute_game"
bt_call_behavior = "{bt_call}"
"#,
            active_profile = self.active_profile,
            auto_apply = self.auto_apply_on_connect,
            save_flash = self.save_to_flash,
            poll_secs = self.poll_status_interval_secs,
            web_enabled = self.web_enabled,
            web_bind = self.web_bind,
            sidetone_enabled = self.hardware.sidetone_enabled,
            sidetone_level = self.hardware.sidetone_level.min(MAX_SIDETONE_LEVEL),
            mic_gain = self.hardware.mic_gain.min(MAX_MIC_GAIN_LEVEL),
            volume_limiter = self.hardware.volume_limiter,
            inactivity_mins = self.hardware.inactivity_timer_mins.min(MAX_INACTIVITY_MINUTES),
            mute_led = self.hardware.mute_led_brightness.min(MAX_MUTE_LED_BRIGHTNESS),
            bt_power = self.hardware.bt_power_on_default,
            bt_call = self.hardware.bt_call_behavior.as_str(),
        )
    }

    pub fn from_toml_str(input: &str) -> Result<Self> {
        let mut cfg = Self::default();
        let mut web_host: Option<String> = None;
        let mut web_port: Option<u16> = None;

        for raw_line in input.lines() {
            let line = strip_comment(raw_line).trim();
            if line.is_empty() || line.starts_with('[') {
                continue;
            }
            let Some((raw_key, raw_val)) = line.split_once('=') else {
                continue;
            };
            let key = raw_key.trim();
            let val = raw_val.trim();

            match key {
                "active_profile" => {
                    cfg.active_profile = unquote(val).to_string();
                }
                "auto_apply_on_connect" => {
                    if let Some(b) = parse_bool(val) {
                        cfg.auto_apply_on_connect = b;
                    }
                }
                "save_to_flash" => {
                    if let Some(b) = parse_bool(val) {
                        cfg.save_to_flash = b;
                    }
                }
                "poll_status_interval_secs" => {
                    if let Ok(n) = val.parse::<u64>() {
                        cfg.poll_status_interval_secs = n;
                    }
                }
                "web_enabled" => {
                    if let Some(b) = parse_bool(val) {
                        cfg.web_enabled = b;
                    }
                }
                "web_bind" => {
                    let parsed = unquote(val).trim();
                    if !parsed.is_empty() {
                        cfg.web_bind = parsed.to_string();
                    }
                }
                "web_host" => {
                    let parsed = unquote(val).trim();
                    if !parsed.is_empty() {
                        web_host = Some(parsed.to_string());
                    }
                }
                "web_port" => {
                    if let Ok(p) = unquote(val).trim().parse::<u16>() {
                        web_port = Some(p);
                    }
                }
                "sidetone_enabled" => {
                    if let Some(b) = parse_bool(val) {
                        cfg.hardware.sidetone_enabled = b;
                    }
                }
                "sidetone_level" => {
                    if let Ok(n) = val.parse::<u8>() {
                        cfg.hardware.sidetone_level = n.min(MAX_SIDETONE_LEVEL);
                    }
                }
                "mic_gain" => {
                    if let Ok(n) = val.parse::<u8>() {
                        cfg.hardware.mic_gain = n.min(MAX_MIC_GAIN_LEVEL);
                    }
                }
                "volume_limiter" => {
                    if let Some(b) = parse_bool(val) {
                        cfg.hardware.volume_limiter = b;
                    }
                }
                "inactivity_timer_mins" => {
                    if let Ok(n) = val.parse::<u8>() {
                        cfg.hardware.inactivity_timer_mins = n.min(MAX_INACTIVITY_MINUTES);
                    }
                }
                "mute_led_brightness" => {
                    if let Ok(n) = val.parse::<u8>() {
                        cfg.hardware.mute_led_brightness = n.min(MAX_MUTE_LED_BRIGHTNESS);
                    }
                }
                "bt_power_on_default" => {
                    if let Some(b) = parse_bool(val) {
                        cfg.hardware.bt_power_on_default = b;
                    }
                }
                "bt_call_behavior" => {
                    cfg.hardware.bt_call_behavior =
                        BtCallBehavior::from_str_lossy(unquote(val));
                }
                _ => {}
            }
        }

        if web_host.is_some() || web_port.is_some() {
            let host = web_host.unwrap_or_else(|| "127.0.0.1".to_string());
            let port = web_port.unwrap_or(3927);
            cfg.web_bind = format!("{}:{}", host, port);
        }

        Ok(cfg)
    }
}

pub fn profile_to_toml_string(profile: &ParametricEqProfile) -> String {
    let gains = profile
        .gains_db()
        .iter()
        .map(|g| format!("{:.1}", g))
        .collect::<Vec<_>>()
        .join(", ");
    let qs = profile
        .q_factors()
        .iter()
        .map(|q| format!("{:.3}", q))
        .collect::<Vec<_>>()
        .join(", ");
    let freqs = EQ_FREQUENCIES_HZ
        .iter()
        .map(|f| f.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let filters = profile
        .bands
        .iter()
        .map(|b| format!("\"{}\"", b.filter_type.as_str()))
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        r#"# Arctis Nova 7 10-Band Hardware Parametric EQ Profile
# Frequencies (Hz): [{freqs}]
# Gain range: -10.0 dB to +10.0 dB (0.5 dB hardware resolution, encoded as 2 * (10 + gain))

name = "{name}"
description = "{desc}"
frequencies_hz = [{freqs}]
gains_db = [{gains}]
q_factors = [{qs}]
filter_types = [{filters}]
"#,
        name = profile.name,
        desc = profile.description,
        freqs = freqs,
        gains = gains,
        qs = qs,
        filters = filters,
    )
}

pub fn profile_from_toml_str(input: &str, fallback_name: &str) -> Result<ParametricEqProfile> {
    let mut name = fallback_name.to_string();
    let mut description = String::from("Custom 10-band Parametric EQ profile");
    let mut freqs = EQ_FREQUENCIES_HZ;
    let mut gains = [0.0f32; EQ_BAND_COUNT];
    let mut qs = [DEFAULT_Q_FACTOR; EQ_BAND_COUNT];
    let mut filters = [EqFilterType::Peaking; EQ_BAND_COUNT];

    for raw_line in input.lines() {
        let line = strip_comment(raw_line).trim();
        if line.is_empty() || line.starts_with('[') {
            continue;
        }
        let Some((raw_key, raw_val)) = line.split_once('=') else {
            continue;
        };
        let key = raw_key.trim();
        let val = raw_val.trim();

        match key {
            "name" => {
                let parsed = unquote(val).trim();
                if !parsed.is_empty() {
                    name = parsed.to_string();
                }
            }
            "description" => {
                description = unquote(val).to_string();
            }
            "frequencies_hz" => {
                if let Some(items) = parse_bracket_list(val) {
                    if items.len() == EQ_BAND_COUNT {
                        for (i, item) in items.iter().enumerate() {
                            if let Ok(hz) = item.parse::<u16>() {
                                freqs[i] = hz;
                            }
                        }
                    }
                }
            }
            "gains_db" => {
                let Some(items) = parse_bracket_list(val) else {
                    anyhow::bail!("Invalid gains_db array in EQ profile '{}'", name);
                };
                if items.len() != EQ_BAND_COUNT {
                    anyhow::bail!(
                        "EQ profile '{}' must have exactly {} bands in gains_db, found {}",
                        name,
                        EQ_BAND_COUNT,
                        items.len()
                    );
                }
                for (i, item) in items.iter().enumerate() {
                    let parsed: f32 = item.parse().with_context(|| {
                        format!("Invalid float '{}' in gains_db at index {}", item, i)
                    })?;
                    gains[i] = parsed.clamp(MIN_EQ_GAIN_DB, MAX_EQ_GAIN_DB);
                }
            }
            "q_factors" => {
                if let Some(items) = parse_bracket_list(val) {
                    if items.len() == EQ_BAND_COUNT {
                        for (i, item) in items.iter().enumerate() {
                            if let Ok(q) = item.parse::<f32>() {
                                qs[i] = q.clamp(0.1, 10.0);
                            }
                        }
                    }
                }
            }
            "filter_types" => {
                if let Some(items) = parse_bracket_list(val) {
                    if items.len() == EQ_BAND_COUNT {
                        for (i, item) in items.iter().enumerate() {
                            filters[i] = EqFilterType::from_str_lossy(unquote(item));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let mut bands = [EqBand::new(0, 0.0); EQ_BAND_COUNT];
    for i in 0..EQ_BAND_COUNT {
        bands[i] = EqBand::with_q_and_filter(freqs[i], gains[i], qs[i], filters[i]);
    }

    Ok(ParametricEqProfile {
        name,
        description,
        bands,
    })
}

#[derive(Debug, Clone)]
pub struct ConfigManager {
    base_dir: PathBuf,
}

impl ConfigManager {
    /// Resolves the XDG configuration directory (`$XDG_CONFIG_HOME/arctis-nova-7` or `~/.config/arctis-nova-7`).
    pub fn default_config_dir() -> PathBuf {
        if let Ok(xdg) = env::var("XDG_CONFIG_HOME") {
            let trimmed = xdg.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed).join(APP_CONFIG_DIR_NAME);
            }
        }
        if let Ok(home) = env::var("HOME") {
            let trimmed = home.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed)
                    .join(".config")
                    .join(APP_CONFIG_DIR_NAME);
            }
        }
        PathBuf::from(".config").join(APP_CONFIG_DIR_NAME)
    }

    pub fn new() -> Self {
        Self {
            base_dir: Self::default_config_dir(),
        }
    }

    #[allow(dead_code)]
    pub fn with_base_dir(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    pub fn config_file_path(&self) -> PathBuf {
        self.base_dir.join(CONFIG_FILE_NAME)
    }

    pub fn profiles_dir_path(&self) -> PathBuf {
        self.base_dir.join(PROFILES_DIR_NAME)
    }

    pub fn profile_file_path(&self, name: &str) -> PathBuf {
        let sanitized = sanitize_profile_filename(name);
        self.profiles_dir_path().join(format!("{}.toml", sanitized))
    }

    /// Creates `~/.config/arctis-nova-7/`, `config.toml`, and built-in `profiles/*.toml` if missing.
    pub fn ensure_initialized(&self) -> Result<()> {
        let profiles_dir = self.profiles_dir_path();
        fs::create_dir_all(&profiles_dir).with_context(|| {
            format!(
                "Failed to create XDG profiles directory '{}'",
                profiles_dir.display()
            )
        })?;

        let cfg_path = self.config_file_path();
        if !cfg_path.exists() {
            let default_cfg = AppConfig::default();
            fs::write(&cfg_path, default_cfg.to_toml_string()).with_context(|| {
                format!("Failed to write default config to '{}'", cfg_path.display())
            })?;
        }

        // Seed built-in EQ presets into `profiles/*.toml` if not present
        for preset in [
            ParametricEqProfile::flat(),
            ParametricEqProfile::harman(),
            ParametricEqProfile::fps(),
        ] {
            let path = self.profile_file_path(&preset.name);
            if !path.exists() {
                fs::write(&path, profile_to_toml_string(&preset)).with_context(|| {
                    format!("Failed to seed preset profile '{}'", path.display())
                })?;
            }
        }

        Ok(())
    }

    pub fn load_config(&self) -> Result<AppConfig> {
        let path = self.config_file_path();
        if !path.exists() {
            return Ok(AppConfig::default());
        }
        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read '{}'", path.display()))?;
        AppConfig::from_toml_str(&content)
    }

    pub fn save_config(&self, config: &AppConfig) -> Result<()> {
        fs::create_dir_all(&self.base_dir)?;
        let path = self.config_file_path();
        fs::write(&path, config.to_toml_string())
            .with_context(|| format!("Failed to save config to '{}'", path.display()))
    }

    pub fn save_profile(&self, profile: &ParametricEqProfile) -> Result<PathBuf> {
        let profiles_dir = self.profiles_dir_path();
        fs::create_dir_all(&profiles_dir)?;
        let path = self.profile_file_path(&profile.name);
        fs::write(&path, profile_to_toml_string(profile))
            .with_context(|| format!("Failed to write profile to '{}'", path.display()))?;
        Ok(path)
    }

    /// Loads a profile by name from `profiles/<name>.toml`, falling back to built-in presets if not on disk.
    pub fn load_profile(&self, name: &str) -> Result<ParametricEqProfile> {
        let path = self.profile_file_path(name);
        if path.exists() {
            let content = fs::read_to_string(&path)
                .with_context(|| format!("Failed to read profile '{}'", path.display()))?;
            return profile_from_toml_str(&content, name);
        }
        if let Some(builtin) = ParametricEqProfile::builtin_by_name(name) {
            return Ok(builtin);
        }
        anyhow::bail!(
            "EQ profile '{}' not found in '{}' and is not a built-in preset",
            name,
            self.profiles_dir_path().display()
        )
    }

    pub fn list_profiles(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        let dir = self.profiles_dir_path();
        if dir.exists() {
            for entry in fs::read_dir(&dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("toml") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        names.push(stem.to_string());
                    }
                }
            }
        }
        for builtin in ["flat", "harman", "fps", "bass", "smiley"] {
            if !names.iter().any(|n| n.eq_ignore_ascii_case(builtin)) {
                names.push(builtin.to_string());
            }
        }
        names.sort();
        Ok(names)
    }
}

fn sanitize_profile_filename(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "custom".to_string()
    } else {
        cleaned
    }
}

fn strip_comment(line: &str) -> &str {
    let mut in_quotes = false;
    for (idx, ch) in line.char_indices() {
        if ch == '"' {
            in_quotes = !in_quotes;
        } else if ch == '#' && !in_quotes {
            return &line[..idx];
        }
    }
    line
}

fn unquote(val: &str) -> &str {
    let trimmed = val.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    }
}

fn parse_bool(val: &str) -> Option<bool> {
    match unquote(val).to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn parse_bracket_list(val: &str) -> Option<Vec<&str>> {
    let trimmed = val.trim();
    let inner = trimmed.strip_prefix('[')?.strip_suffix(']')?;
    let items: Vec<&str> = inner
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    Some(items)
}
