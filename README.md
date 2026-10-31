# Arctis Nova 7 ChatMix, 10-Band DSP EQ & Web Companion

Lightweight Linux daemon and hardware controller for the **SteelSeries Arctis Nova 7** headset family (`7`, `7P`, `7X`, `Gen 2`, `Diablo IV`, `WoW Edition`).

It creates two PipeWire virtual sinks (`Arctis_Game` and `Arctis_Chat`), links them to your physical headset output, maps the hardware ChatMix dial in real time, provides **10-band hardware parametric DSP EQ (`0x33`)**, monitors live **battery & wireless telemetry (`0xB0` / `0xB7` / `0xB9` / `0xBB`)**, exposes full **onboard hardware controls** (Sidetone, ClearCast Mic Gain, EU Volume Limiter, Auto-Shutoff Timer, Mute LED Brightness, Bluetooth Power-On Default, and Bluetooth Call Mixing Behavior), and serves an **embedded Web Companion Dashboard at `http://127.0.0.1:3927`**.

This repository contains:
- Rust implementation of the PipeWire ChatMix daemon, USB HID / `hidraw` hardware DSP controller, XDG configuration manager, and embedded web server (`src/`)
- Convenience installer/uninstaller script (`install.sh`) supporting per-user or system-wide systemd services, XDG config initialization, and udev rules
- Comprehensive `udev` rule (`config/99-arctis.rules`) covering all 23 RX & TX Product IDs with `uaccess` and USB autosuspend protection (`power/control="on"`)
- Arch Linux AUR packaging (`aur/arctis-nova-7-chatmix` and `aur/arctis-nova-7-chatmix-bin`)

---

## Supported Devices

Supported environment:
- Linux with PipeWire (`pactl`, `pw-link` / `pw-cli` available)
- `libusb` & `hidapi` (`hidraw` vendor usage page `0xFFC0`)
- SteelSeries USB Vendor ID: `0x1038`

| Model / Variant | Role | Product ID | Battery Telemetry Format |
|-----------------|------|------------|--------------------------|
| Arctis Nova 7 | Headset / RX | `0x2200` | Discrete (`0–4` → `0–100%`) |
| Arctis Nova 7 | Wireless Dongle / TX | `0x2202` | Discrete (`0–4` → `0–100%`) |
| Arctis Nova 7X | Headset / RX | `0x2204` | Discrete (`0–4` → `0–100%`) |
| Arctis Nova 7X | Wireless Dongle / TX | `0x2206` | Discrete (`0–4` → `0–100%`) |
| Arctis Nova 7P | Headset / RX | `0x2208` | Discrete (`0–4` → `0–100%`) |
| Arctis Nova 7P | Wireless Dongle / TX | `0x220a` | Discrete (`0–4` → `0–100%`) |
| Arctis Nova 7 Diablo IV Edition | Headset / RX | `0x2238` | Discrete (`0–4` → `0–100%`) |
| Arctis Nova 7 Diablo IV Edition (Pre-update) | Wireless Dongle / TX | `0x223a` | Discrete (`0–4` → `0–100%`) |
| Arctis Nova 7X White | Headset / RX | `0x2256` | Direct Percentage (`0–100%`) |
| Arctis Nova 7X v2 (Variant 1) | Wireless Dongle / TX | `0x2258` | Direct Percentage (`0–100%`) |
| Arctis Nova 7 WoW Edition | Headset / RX | `0x2278` | Discrete (`0–4` → `0–100%`) |
| Arctis Nova 7 WoW Edition | Wireless Dongle / TX | `0x227a` | Discrete (`0–4` → `0–100%`) |
| Arctis Nova 7 Gen 2 (`brisket_rx`) | Headset / RX | `0x227c` | Direct Percentage (`0–100%`) |
| Arctis Nova 7 Wireless Gen 2 (`brisket_tx`) | Wireless Dongle / TX | `0x227e` | Direct Percentage (`0–100%`) |
| Arctis Nova 7P Gen 2 (`brisket_ps_rx`) | Headset / RX | `0x2288` | Direct Percentage (`0–100%`) |
| Arctis Nova 7P Gen 2 (`brisket_ps_tx`) | Wireless Dongle / TX | `0x228a` | Direct Percentage (`0–100%`) |
| Arctis Nova 7X Gen 2 (`brisket_xbox_rx`) | Headset / RX | `0x229c` | Direct Percentage (`0–100%`) |
| Arctis Nova 7X v2 (Variant 2) | Wireless Dongle / TX | `0x229e` | Direct Percentage (`0–100%`) |
| Arctis Nova 7 Gen 2 Upgrade (Jan 2026) | Wireless Dongle / TX | `0x22a1` | Direct Percentage (`0–100%`) |
| Arctis Nova 7X | Headset / RX | `0x22a4` | Discrete (`0–4` → `0–100%`) |
| Arctis Nova 7X | Wireless Dongle / TX | `0x22a5` | Direct Percentage (`0–100%`) |
| Arctis Nova 7 Diablo IV Edition (Post-update) | Wireless Dongle / TX | `0x22a9` | Direct Percentage (`0–100%`) |
| Arctis Nova 7X v2 | Wireless Dongle / TX | `0x22ad` | Direct Percentage (`0–100%`) |

---

## Features

### 1. PipeWire Virtual Sinks & Real-Time ChatMix (`0x45`)
- Creates two PipeWire virtual sinks:
  - `Arctis_Game` — default output sink for game and media audio
  - `Arctis_Chat` — dedicated output sink for voice/communication apps (Discord, TeamSpeak, WebRTC)
- Automatically links virtual sinks to the physical headset playback ports (`pw-link`)
- Reads hardware ChatMix dial reports (`0x45`) and smoothly adjusts `Arctis_Game` and `Arctis_Chat` volumes
- **Hotplug & Reconnect Resilience**: Detects dongle unplug/replug, reclaims the USB interface, relinks virtual sinks, restores `Arctis_Game` as default sink, and moves active audio streams seamlessly without restarting apps
- **Clean Shutdown**: Restores the previous default sink and removes virtual sink nodes on exit

### 2. 10-Band Hardware Parametric DSP Equalizer (`0x33`)
- Programs the headset's onboard 10-band parametric biquad DSP (`31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000 Hz`) over `hidraw` (`Usage Page 0xFFC0`)
- Supports gain range `-10.0 dB` to `+10.0 dB` (0.5 dB resolution centered at byte value `20 = 0.0 dB`), filter types (`peaking`, `low_shelf`, `high_shelf`, `low_pass`, `high_pass`), and custom `Q` factors
- Ships with built-in audiophile and gaming presets in `~/.config/arctis-nova-7/profiles/`:
  - `flat` — Neutral 0.0 dB reference across all 10 bands
  - `harman` — Tuned to compensate Nova 7's 4 kHz dip and 8 kHz peak toward the Harman OE target
  - `fps` — Competitive footsteps clarity with reduced sub-bass rumble and boosted 2–4 kHz cues
  - `bass_boost`, `smiley`, `vocal` — Ready-to-use musical & voice presets

### 3. Live Battery & Hardware Telemetry (`0xB0`, `0xB7`, `0xB9`, `0xBB`, `0x52`)
- Queries general headset status (`0xB0`) and listens to real-time asynchronous HID events:
  - **Battery percentage** (automatically normalizing both Gen 1 discrete `0–4` and Gen 2 `0–100%` firmware formats)
  - **Charging state** (`Disconnected`, `Charging`, `Discharging`, `Connected`)
  - **Wireless link state** (`Offline`, `Cable`, `Dongle 2.4GHz`, `Bluetooth`, `Dual 2.4GHz + BT`)
  - **Hardware microphone mute** (`0x52`), ChatMix balance (`0x45`), and active onboard settings

### 4. Onboard Hardware Controls & Flash Persistence (`0x09`)
- **Sidetone (`0x39`)**: Off (`0`), Low (`1`), Medium (`2`), High (`3`)
- **Microphone Gain (`0x37`)**: Hardware ClearCast Gen 2 gain (`0..=15`, default `12` / ~75% to prevent analog clipping)
- **EU Volume Limiter (`0x3A`)**: Toggle hardware acoustic limiter on/off
- **Inactivity Auto-Shutoff Timer (`0xA3`)**: `0..=90` minutes (`0` = disabled)
- **Mic Mute LED Brightness (`0xAE`)**: Off (`0`), Low (`1`), Medium (`2`), High (`3`)
- **Bluetooth Power-On Default (`0xB2`)**: Automatically enable Bluetooth when powering on the headset
- **Bluetooth Call Audio Mixing (`0xB3`)**: Keep 2.4G audio unchanged (`do_nothing`), attenuate game audio by `-12 dB` (`attenuate_12db`), or mute 2.4G audio (`mute_game`) during Bluetooth phone calls
- **Save to Onboard Flash (`0x09`)**: Persist active EQ and hardware settings directly to the headset's non-volatile memory

### 5. Embedded Web Companion Dashboard (`http://127.0.0.1:3927`)
- Lightweight local web server bound strictly to `127.0.0.1:3927` (zero external runtime asset dependencies)
- Interactive 10-band EQ sliders with live preset switching and profile saving
- Real-time battery, wireless connection, charging, and ChatMix telemetry via Server-Sent Events (SSE) / REST API
- One-click toggles and sliders for Sidetone, Mic Gain, Volume Limiter, Auto-Shutoff, Mute LED, and Bluetooth Call Mixing

---

## Configuration Directory (`~/.config/arctis-nova-7/`)

On first run (or via `arctis_chatmix --init-config`), the application initializes an XDG configuration directory at `$XDG_CONFIG_HOME/arctis-nova-7/` (default: `~/.config/arctis-nova-7/`):

```text
~/.config/arctis-nova-7/
├── config.toml            # General preferences & hardware control state
└── profiles/              # 10-Band Parametric EQ presets (*.toml)
    ├── flat.toml
    ├── harman.toml
    ├── fps.toml
    ├── bass_boost.toml
    ├── smiley.toml
    └── vocal.toml
```

Example `~/.config/arctis-nova-7/config.toml`:

```toml
[general]
active_profile = "harman"
auto_apply_on_connect = true
save_to_flash = false
poll_status_interval_secs = 30
web_enabled = true
web_bind = "127.0.0.1:3927"

[hardware]
sidetone_enabled = true
sidetone_level = 0
mic_gain = 12
volume_limiter = false
inactivity_timer_mins = 30
mute_led_brightness = 2
bt_power_on_default = false
bt_call_behavior = "do_nothing"
```

---

## CLI Usage

In addition to running as a background daemon and serving the Web UI at `http://127.0.0.1:3927`, `arctis_chatmix` provides direct CLI flags for scripting and quick hardware adjustments:

```bash
# Start the PipeWire ChatMix, Telemetry & Web Companion daemon (default)
arctis_chatmix

# Initialize ~/.config/arctis-nova-7/config.toml and built-in EQ profiles
arctis_chatmix --init-config

# Query live battery, wireless link, charging, and hardware telemetry (0xB0)
arctis_chatmix --status

# List available 10-band EQ profiles and apply a preset
arctis_chatmix --list-profiles
arctis_chatmix --profile harman
arctis_chatmix --profile fps --save-flash

# Apply custom 10-band EQ gains in dB (31Hz .. 16kHz)
arctis_chatmix --set-eq "-0.5,-1.5,0.0,1.0,-1.5,-1.0,4.5,1.5,-4.0,1.0"

# Configure onboard hardware controls
arctis_chatmix --sidetone-level 2
arctis_chatmix --mic-gain 12
arctis_chatmix --volume-limiter off
arctis_chatmix --inactivity-timer 30
arctis_chatmix --mute-led 1
arctis_chatmix --bt-power off
arctis_chatmix --bt-call attenuate_12db

# Re-apply all settings from ~/.config/arctis-nova-7/config.toml
arctis_chatmix --apply-config
```

---

## Installation

### Quick Start

1. Clone the repository:
   ```bash
   git clone https://github.com/m24ih/Arctis-Nova-7-Chatmix.git
   cd Arctis-Nova-7-Chatmix
   ```

2. Run the installer:
   ```bash
   chmod +x install.sh
   ./install.sh
   ```
   The installer installs the binary, initializes `~/.config/arctis-nova-7/`, installs `/etc/udev/rules.d/99-arctis.rules` for all 20 Nova 7 Product IDs, and enables the systemd service.

3. Open the Web Companion Dashboard in your browser:
   ```text
   http://127.0.0.1:3927
   ```

### Non-interactive Examples

- Per-user install:
  ```bash
  ./install.sh --binary ./arctis_chatmix --mode user --udev yes --enable-service yes --enable-linger no -y
  ```

- System install (requires sudo):
  ```bash
  sudo ./install.sh --binary ./arctis_chatmix --mode system --udev yes --enable-service yes -y
  ```

### Files the installer manages

| Component | User mode | System mode |
|-----------|-----------|-------------|
| Binary | `~/.local/bin/arctis_chatmix` | `/usr/local/bin/arctis_chatmix` |
| Systemd unit | `~/.config/systemd/user/arctis_chatmix.service` | `/etc/systemd/system/arctis_chatmix.service` |
| XDG Config & EQ Presets | `~/.config/arctis-nova-7/` | `~/.config/arctis-nova-7/` |
| udev rule *(optional)* | `/etc/udev/rules.d/99-arctis.rules` | `/etc/udev/rules.d/99-arctis.rules` |
| Local Web Companion UI | `http://127.0.0.1:3927` | `http://127.0.0.1:3927` |

### udev rule (`config/99-arctis.rules`)

The included udev rule grants active session (`TAG+="uaccess"`) and `audio` group (`MODE="0660", GROUP="audio"`) permissions to both `usb` and `hidraw*` nodes for all 20 Arctis Nova 7 RX/TX Product IDs, and sets `ATTR{power/control}="on"` to prevent Linux USB autosuspend from dropping the wireless receiver.

To apply manually:
```bash
sudo install -Dm644 config/99-arctis.rules /etc/udev/rules.d/99-arctis.rules
sudo usermod -aG audio $USER
sudo udevadm control --reload
sudo udevadm trigger --subsystem-match=usb --attr-match=idVendor=1038
sudo udevadm trigger --subsystem-match=hidraw --attr-match=idVendor=1038
```

---

## Uninstallation

Use `install.sh` with `--uninstall`:

```bash
# Interactive
./install.sh --uninstall

# Non-interactive per-user uninstall
./install.sh --uninstall --mode user --udev yes -y
```

> **Note:** User configuration and custom EQ profiles in `~/.config/arctis-nova-7/` are preserved during uninstallation. Virtual sinks (`Arctis_Game` / `Arctis_Chat`) disappear when the service stops or after your PipeWire session restarts.

---

## Running and Logs

- Check service status & logs (user mode):
  ```bash
  systemctl --user status arctis_chatmix.service
  journalctl --user -u arctis_chatmix.service -f
  ```
- System service logs:
  ```bash
  sudo journalctl -u arctis_chatmix.service -f
  ```

---

## Building from Source

Requirements: Rust (`cargo`), `libusb`, `hidapi`, `pkgconf`

```bash
# Arch Linux
sudo pacman -S rust libusb hidapi pkgconf

# Build release binary
cargo build --release
# Output: target/release/arctis_chatmix
```

---

## Troubleshooting

- Confirm PipeWire sees the physical sink and virtual sinks:
  ```bash
  pactl list short sinks | grep -i arctis
  ```
- Verify USB & `hidraw` permissions:
  ```bash
  ls -l /dev/hidraw*
  arctis_chatmix --status
  ```
- If audio streams do not switch automatically after reconnect:
  ```bash
  pactl set-default-sink Arctis_Game
  ```

---

## License

This project is provided under the MIT license — see the included [LICENSE](LICENSE) file.

