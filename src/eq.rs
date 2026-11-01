// 10-Band Hardware Parametric EQ Engine for SteelSeries Arctis Nova 7.
//
// Protocol details (reverse-engineered from SteelSeries GG Engine `.edevice`
// and SteelSeries Arctis Companion v6.10.0):
// - USB HID Opcode: `0x33`
// - Bluetooth GAIA v3 Set Command ID: `0x0933` (2355)
// - 10 fixed/standard center frequencies:
//   31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000 Hz
// - Gain encoding formula:
//   SteelSeries DSP uses 0.5 dB steps across `-10.0 dB ..= +10.0 dB`:
//   `encoded = 2 * (10 + gain_db)` in `0..=40` (`0x00..=0x28`), where `0 dB == 20 (0x14)`.

use crate::device::{
    build_gaia_command, EQ_24G_OPCODE, EQ_BT_OPCODE, EQ_MIC_OPCODE, GAIA_SET_EQ_CMD,
    HID_MSG_SIZE,
};
use anyhow::{bail, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EqChannel {
    Speaker24G = 0,
    SpeakerBt = 1,
    Microphone = 2,
}

impl EqChannel {
    #[allow(non_upper_case_globals, dead_code)]
    pub const Channel24G: Self = Self::Speaker24G;
    #[allow(non_upper_case_globals, dead_code)]
    pub const Bluetooth: Self = Self::SpeakerBt;

    pub fn from_str_lossy(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "bt" | "bluetooth" | "speaker_bt" => Self::SpeakerBt,
            "mic" | "microphone" => Self::Microphone,
            _ => Self::Speaker24G,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Speaker24G => "24g",
            Self::SpeakerBt => "bt",
            Self::Microphone => "mic",
        }
    }
}

pub const EQ_BAND_COUNT: usize = 10;

pub const EQ_FREQUENCIES_HZ: [u16; EQ_BAND_COUNT] = [
    31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000,
];

pub const MIN_EQ_GAIN_DB: f32 = -10.0;
pub const MAX_EQ_GAIN_DB: f32 = 10.0;
pub const EQ_ZERO_DB_RAW: u8 = 20;
pub const MAX_EQ_RAW: u8 = 40;
pub const DEFAULT_Q_FACTOR: f32 = 1.414;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EqFilterType {
    Peaking = 1,
    LowPass = 2,
    HighPass = 3,
    LowShelf = 4,
    HighShelf = 5,
}

impl Default for EqFilterType {
    fn default() -> Self {
        Self::Peaking
    }
}

impl EqFilterType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Peaking => "peaking",
            Self::LowPass => "low_pass",
            Self::HighPass => "high_pass",
            Self::LowShelf => "low_shelf",
            Self::HighShelf => "high_shelf",
        }
    }

    pub fn from_str_lossy(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "low_pass" | "lowpass" | "lp" => Self::LowPass,
            "high_pass" | "highpass" | "hp" => Self::HighPass,
            "low_shelf" | "lowshelf" | "ls" => Self::LowShelf,
            "high_shelf" | "highshelf" | "hs" => Self::HighShelf,
            _ => Self::Peaking,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EqBand {
    pub frequency_hz: u16,
    pub gain_db: f32,
    pub q_factor: f32,
    pub filter_type: EqFilterType,
}

impl EqBand {
    pub const fn new(frequency_hz: u16, gain_db: f32) -> Self {
        Self {
            frequency_hz,
            gain_db,
            q_factor: DEFAULT_Q_FACTOR,
            filter_type: EqFilterType::Peaking,
        }
    }

    pub const fn with_q_and_filter(
        frequency_hz: u16,
        gain_db: f32,
        q_factor: f32,
        filter_type: EqFilterType,
    ) -> Self {
        Self {
            frequency_hz,
            gain_db,
            q_factor,
            filter_type,
        }
    }

    pub fn encoded_gain(&self) -> u8 {
        encode_eq_gain(self.gain_db)
    }

    #[allow(dead_code)]
    pub fn encoded_q(&self) -> u16 {
        let clamped = self.q_factor.clamp(0.1, 10.0);
        (clamped * 1000.0).round() as u16
    }
}

/// Encodes a floating-point dB gain (`-10.0..=+10.0`) into the SteelSeries
/// 0.5 dB step hardware byte (`0..=40`) using `2 * (10 + gain)`.
pub fn encode_eq_gain(gain_db: f32) -> u8 {
    if !gain_db.is_finite() {
        return EQ_ZERO_DB_RAW;
    }
    let clamped = gain_db.clamp(MIN_EQ_GAIN_DB, MAX_EQ_GAIN_DB);
    let raw = (2.0 * (10.0 + clamped)).round() as i32;
    raw.clamp(0, MAX_EQ_RAW as i32) as u8
}

/// Decodes a SteelSeries hardware EQ byte (`0..=40`) back into dB (`-10.0..=+10.0`).
#[allow(dead_code)]
pub fn decode_eq_gain(raw: u8) -> f32 {
    let clamped = raw.min(MAX_EQ_RAW);
    (clamped as f32 / 2.0) - 10.0
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParametricEqProfile {
    pub name: String,
    pub description: String,
    pub bands: [EqBand; EQ_BAND_COUNT],
}

impl ParametricEqProfile {
    pub fn from_gains(
        name: impl Into<String>,
        description: impl Into<String>,
        gains_db: [f32; EQ_BAND_COUNT],
    ) -> Self {
        let mut bands = [EqBand::new(0, 0.0); EQ_BAND_COUNT];
        let mut idx = 0;
        while idx < EQ_BAND_COUNT {
            bands[idx] = EqBand::new(EQ_FREQUENCIES_HZ[idx], gains_db[idx]);
            idx += 1;
        }
        Self {
            name: name.into(),
            description: description.into(),
            bands,
        }
    }

    pub fn gains_db(&self) -> [f32; EQ_BAND_COUNT] {
        let mut out = [0.0f32; EQ_BAND_COUNT];
        for (i, band) in self.bands.iter().enumerate() {
            out[i] = band.gain_db;
        }
        out
    }

    pub fn q_factors(&self) -> [f32; EQ_BAND_COUNT] {
        let mut out = [DEFAULT_Q_FACTOR; EQ_BAND_COUNT];
        for (i, band) in self.bands.iter().enumerate() {
            out[i] = band.q_factor;
        }
        out
    }

    /// Flat reference preset (all 10 bands at 0.0 dB).
    pub fn flat() -> Self {
        Self::from_gains(
            "flat",
            "Flat reference response (all 10 bands at 0.0 dB)",
            [0.0; EQ_BAND_COUNT],
        )
    }

    /// Audiophile Harman Target balance preset calibrated for Arctis Nova 7 drivers.
    pub fn harman() -> Self {
        Self::from_gains(
            "harman",
            "Audiophile Harman balance (cuts 250Hz mud, restores 4kHz dip)",
            [2.0, 1.0, -1.5, -2.0, 0.0, 0.5, 1.5, 2.5, -1.0, 1.5],
        )
    }

    /// Competitive FPS tactical preset emphasizing footsteps and directional cues (1-4 kHz).
    pub fn fps() -> Self {
        Self::from_gains(
            "fps",
            "Competitive FPS tactical footsteps (cuts low rumble, boosts 2-4kHz)",
            [-3.0, -1.5, -2.0, -2.5, 0.0, 1.0, 2.5, 4.0, 1.5, 0.0],
        )
    }

    /// Counter-Strike 2 (CS2) competitive preset emphasizing footsteps, defuse wires, and utility cues.
    pub fn cs2() -> Self {
        Self::from_gains(
            "cs2",
            "Counter-Strike 2 footsteps, defuse cues, and reduced HE/AWP low boom",
            [-3.5, -2.0, -1.5, -2.0, 0.5, 2.0, 3.5, 4.5, 2.0, 0.5],
        )
    }

    /// Valorant tactical preset emphasizing agent footsteps, spike cues, and ability audio.
    pub fn valorant() -> Self {
        Self::from_gains(
            "valorant",
            "Valorant agent footsteps, spike plant/defuse cues, and ability clarity",
            [-2.5, -1.5, -1.0, -1.5, 0.5, 1.5, 3.0, 3.5, 1.5, 0.0],
        )
    }

    /// Apex Legends battle-royale preset separating vertical footsteps, ziplines, and shield cracks.
    pub fn apex() -> Self {
        Self::from_gains(
            "apex",
            "Apex Legends vertical footsteps, shield cracks, and reduced ambient wind",
            [-3.0, -2.0, -1.5, -2.0, 0.0, 1.5, 3.5, 4.0, 2.5, 1.0],
        )
    }

    /// Call of Duty: Warzone preset cutting airstrike/vehicle sub-bass and boosting footsteps/crates.
    pub fn warzone() -> Self {
        Self::from_gains(
            "warzone",
            "Call of Duty: Warzone reduced airstrike sub-bass and boosted footstep/crate highs",
            [-4.0, -2.5, -1.5, -2.0, -0.5, 1.5, 3.0, 4.5, 2.0, 0.5],
        )
    }

    /// Baldur's Gate 3 & RPG Cinema preset with warm orchestral sub-bass and clear dialogue mids.
    pub fn bg3() -> Self {
        Self::from_gains(
            "bg3",
            "Baldur's Gate 3 & RPG Cinema warm orchestral bass, clear dialogue, and airy highs",
            [2.5, 2.0, 0.5, -1.0, 0.5, 1.0, 1.5, 2.0, 1.5, 2.0],
        )
    }

    /// Warm Bass Boost preset.
    pub fn bass_boost() -> Self {
        Self::from_gains(
            "bass",
            "Warm sub-bass and punchy low-end enhancement",
            [3.5, 3.0, 2.0, 0.5, 0.0, 0.0, 0.5, 1.0, 1.0, 0.5],
        )
    }

    /// V-shaped Smiley preset.
    pub fn smiley() -> Self {
        Self::from_gains(
            "smiley",
            "Classic V-shaped dynamic entertainment curve",
            [3.0, 2.0, 1.0, -1.0, -1.5, -1.0, 1.0, 2.5, 3.0, 2.0],
        )
    }

    /// Vocal & Podcast clarity preset.
    pub fn vocal() -> Self {
        Self::from_gains(
            "vocal",
            "Vocal & podcast speech clarity (rolls off rumble, boosts 500Hz-4kHz presence)",
            [-3.0, -2.0, -1.0, 0.0, 1.5, 2.5, 3.0, 2.0, 0.5, -0.5],
        )
    }

    /// Balanced preset matching official Arctis Companion Balanced profile.
    pub fn balanced() -> Self {
        Self::from_gains(
            "balanced",
            "Official Arctis Nova balanced natural and smooth audio profile",
            [1.0, 1.0, 0.5, 0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 0.5],
        )
    }

    /// Deadlock competitive game preset matching official Arctis Companion Deadlock profile.
    pub fn deadlock() -> Self {
        Self::from_gains(
            "deadlock",
            "Valve Deadlock competitive shooter preset: ability cues and footstep clarity",
            [-2.0, -1.0, 0.0, 1.0, 2.0, 3.0, 3.5, 2.5, 1.0, 0.0],
        )
    }

    pub fn builtin_presets() -> Vec<Self> {
        vec![
            Self::flat(),
            Self::balanced(),
            Self::harman(),
            Self::deadlock(),
            Self::fps(),
            Self::cs2(),
            Self::valorant(),
            Self::apex(),
            Self::warzone(),
            Self::bg3(),
            Self::bass_boost(),
            Self::smiley(),
            Self::vocal(),
        ]
    }

    pub fn builtin_by_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "flat" | "default" => Some(Self::flat()),
            "balanced" | "balance" => Some(Self::balanced()),
            "deadlock" => Some(Self::deadlock()),
            "harman" | "music" | "audiophile" => Some(Self::harman()),
            "fps" | "footsteps" | "competitive" => Some(Self::fps()),
            "cs2" | "counter-strike" | "csgo" => Some(Self::cs2()),
            "valorant" | "val" => Some(Self::valorant()),
            "apex" | "apex_legends" | "apex-legends" => Some(Self::apex()),
            "warzone" | "cod" => Some(Self::warzone()),
            "bg3" | "baldurs_gate_3" | "rpg" | "cinema" => Some(Self::bg3()),
            "bass" | "bass_boost" | "bass-boost" => Some(Self::bass_boost()),
            "smiley" | "v-shape" | "vshape" => Some(Self::smiley()),
            "vocal" | "voice" | "podcast" => Some(Self::vocal()),
            _ => None,
        }
    }

    /// Parses a comma-separated list of 10 dB gain values (e.g. `"2.0,1.0,-1.5,-2.0,0,0.5,1.5,2.5,-1.0,1.5"`).
    pub fn from_csv_gains(name: impl Into<String>, csv: &str) -> Result<Self> {
        let parts: Vec<&str> = csv
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if parts.len() != EQ_BAND_COUNT {
            bail!(
                "Expected {} comma-separated EQ gain values (in dB), got {}",
                EQ_BAND_COUNT,
                parts.len()
            );
        }
        let mut gains = [0.0f32; EQ_BAND_COUNT];
        for (i, part) in parts.iter().enumerate() {
            let val: f32 = part
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid EQ gain value '{}' at band {}", part, i + 1))?;
            gains[i] = val.clamp(MIN_EQ_GAIN_DB, MAX_EQ_GAIN_DB);
        }
        Ok(Self::from_gains(name, "Custom CLI EQ profile", gains))
    }

    /// Builds the 64-byte USB HID report for writing the 10-band EQ to the headset for a specific channel.
    /// - `Speaker24G`: opcode `0x33`
    /// - `SpeakerBt`: opcode `0x34`
    /// - `Microphone`: opcode `0x32`
    pub fn build_hid_report_for_channel(&self, channel: EqChannel) -> [u8; HID_MSG_SIZE] {
        let mut data = [0u8; HID_MSG_SIZE];
        data[0] = 0x00;
        data[1] = match channel {
            EqChannel::Speaker24G => EQ_24G_OPCODE,
            EqChannel::SpeakerBt => EQ_BT_OPCODE,
            EqChannel::Microphone => EQ_MIC_OPCODE,
        };
        for (i, band) in self.bands.iter().enumerate() {
            data[2 + i] = band.encoded_gain();
        }
        // Trailing band termination byte expected by Nova 7 firmware
        data[2 + EQ_BAND_COUNT] = 0x00;
        data
    }

    /// Builds the 64-byte USB HID report for writing the 10-band EQ to the headset (`0x33`).
    pub fn build_hid_report(&self) -> [u8; HID_MSG_SIZE] {
        self.build_hid_report_for_channel(EqChannel::Speaker24G)
    }

    /// Builds the Bluetooth GAIA v3 command payload (`0x0933`) matching the Android Companion app.
    #[allow(dead_code)]
    pub fn build_gaia_frame(&self) -> Vec<u8> {
        let mut payload = Vec::with_capacity(EQ_BAND_COUNT + 1);
        for band in &self.bands {
            payload.push(band.encoded_gain());
        }
        payload.push(0x00);
        build_gaia_command(GAIA_SET_EQ_CMD, &payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_eq_channel_from_str() {
        assert_eq!(EqChannel::from_str_lossy("24g"), EqChannel::Speaker24G);
        assert_eq!(EqChannel::from_str_lossy("speaker_24ghz"), EqChannel::Speaker24G);
        assert_eq!(EqChannel::from_str_lossy("bt"), EqChannel::SpeakerBt);
        assert_eq!(EqChannel::from_str_lossy("bluetooth"), EqChannel::SpeakerBt);
        assert_eq!(EqChannel::from_str_lossy("speaker_bt"), EqChannel::SpeakerBt);
        assert_eq!(EqChannel::from_str_lossy("mic"), EqChannel::Microphone);
        assert_eq!(EqChannel::from_str_lossy("microphone"), EqChannel::Microphone);
        assert_eq!(EqChannel::from_str_lossy("unknown"), EqChannel::Speaker24G);
    }

    #[test]
    fn test_build_hid_report_for_channel_opcodes() {
        let profile = ParametricEqProfile::flat();
        let rep_24g = profile.build_hid_report_for_channel(EqChannel::Speaker24G);
        assert_eq!(rep_24g[0], 0x00);
        assert_eq!(rep_24g[1], 0x33);

        let rep_bt = profile.build_hid_report_for_channel(EqChannel::SpeakerBt);
        assert_eq!(rep_bt[0], 0x00);
        assert_eq!(rep_bt[1], 0x34);

        let rep_mic = profile.build_hid_report_for_channel(EqChannel::Microphone);
        assert_eq!(rep_mic[0], 0x00);
        assert_eq!(rep_mic[1], 0x32);

        // Verify existing build_hid_report still defaults to 24G (0x33)
        assert_eq!(profile.build_hid_report()[1], 0x33);
    }
}
