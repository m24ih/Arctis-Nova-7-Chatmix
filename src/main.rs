mod audio;
mod config;
mod controller;
mod device;
mod eq;
mod state;
mod telemetry;
mod usb;
mod web;

use anyhow::Result;
use config::ConfigManager;
use controller::ArctisController;
use device::{
    build_bt_call_behavior_report, build_bt_power_default_report, build_inactivity_timer_report,
    build_mic_gain_report, build_mute_led_brightness_report, build_sidetone_level_report,
    build_volume_limiter_report, BtCallBehavior,
};
use eq::ParametricEqProfile;
use log::info;
use std::env;
use telemetry::HeadsetTelemetry;
use usb::{
    hidapi_apply_config_and_eq, hidapi_request_status, hidapi_save_to_flash, hidapi_send_report,
    hidapi_send_sidetone,
};

fn print_help() {
    println!(
        "\
Arctis Nova 7 ChatMix & Hardware DSP Controller

USAGE:
    arctis_chatmix [OPTIONS]

When run without one-shot options, starts the PipeWire ChatMix, Telemetry & Web API daemon.

OPTIONS:
    --init-config                  Initialize ~/.config/arctis-nova-7/config.toml & presets
    --list-profiles                List available 10-band EQ profiles
    --profile <NAME>               Activate and apply a 10-band EQ profile (flat, harman, fps, ...)
    --set-eq <G1,G2,...,G10>       Apply custom 10-band EQ gains in dB (-10.0..+10.0)
    --status                       Query headset battery & telemetry status (0xB0)
    --sidetone <0..100>            Set sidetone by percentage (0..100)
    --sidetone-level <0..3>        Set sidetone hardware bucket (0=Off, 1=Low, 2=Med, 3=High)
    --mic-gain <0..15>             Set microphone hardware gain level (0..15, 0x37)
    --volume-limiter <on|off>      Toggle hardware EU volume limiter (0x3A)
    --inactivity-timer <0..90>     Set auto-shutoff inactivity timer in minutes (0xA3)
    --mute-led <0..3>              Set microphone mute LED brightness (0..3, 0xAE)
    --bt-power <on|off>            Set Bluetooth power-on default (0xB2)
    --bt-call <MODE>               Set Bluetooth call behavior: do_nothing, attenuate_12db, mute_game (0xB3)
    --apply-config                 Apply all hardware settings & active EQ from config.toml
    --save-flash                   Persist current settings to headset onboard flash (0x09)
    --web-bind <HOST:PORT>         Override embedded HTTP server bind address (default 127.0.0.1:3927)
    --no-web                       Disable embedded HTTP server
    -h, --help                     Print this help message"
    );
}

fn parse_on_off(val: &str) -> Result<bool> {
    match val.trim().to_ascii_lowercase().as_str() {
        "on" | "1" | "true" | "yes" => Ok(true),
        "off" | "0" | "false" | "no" => Ok(false),
        other => anyhow::bail!("Expected 'on' or 'off', got '{}'", other),
    }
}

fn handle_cli_args(args: &[String]) -> Result<bool> {
    if args.is_empty() {
        return Ok(false);
    }

    let cfg_mgr = ConfigManager::new();
    cfg_mgr.ensure_initialized()?;
    let mut cfg = cfg_mgr.load_config()?;
    let mut config_modified = false;
    let mut save_flash = false;
    let mut run_daemon_after = false;
    let mut saw_oneshot = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_help();
                return Ok(true);
            }
            "--init-config" => {
                println!(
                    "Initialized XDG config at: {}",
                    cfg_mgr.base_dir().display()
                );
            }
            "--list-profiles" => {
                let profiles = cfg_mgr.list_profiles()?;
                println!(
                    "Available 10-band EQ profiles (active = '{}'):",
                    cfg.active_profile
                );
                for name in profiles {
                    let marker = if name.eq_ignore_ascii_case(&cfg.active_profile) {
                        "*"
                    } else {
                        " "
                    };
                    if let Ok(prof) = cfg_mgr.load_profile(&name) {
                        println!(" {} {:<12} - {}", marker, prof.name, prof.description);
                    } else {
                        println!(" {} {}", marker, name);
                    }
                }
            }
            "--profile" => {
                i += 1;
                let name = args
                    .get(i)
                    .ok_or_else(|| anyhow::anyhow!("--profile requires a profile name"))?;
                let profile = cfg_mgr.load_profile(name)?;
                cfg.active_profile = profile.name.clone();
                config_modified = true;
                hidapi_send_report(&profile.build_hid_report())?;
                println!(
                    "Applied 10-band EQ profile '{}' ({})",
                    profile.name, profile.description
                );
            }
            "--set-eq" => {
                i += 1;
                let csv = args
                    .get(i)
                    .ok_or_else(|| anyhow::anyhow!("--set-eq requires 10 comma-separated dB values"))?;
                let profile = ParametricEqProfile::from_csv_gains("custom", csv)?;
                cfg_mgr.save_profile(&profile)?;
                cfg.active_profile = "custom".to_string();
                config_modified = true;
                hidapi_send_report(&profile.build_hid_report())?;
                println!("Saved and applied custom 10-band EQ profile: {:?}", profile.gains_db());
            }
            "--status" => {
                match hidapi_request_status()? {
                    Some(event) => {
                        let mut telemetry = HeadsetTelemetry::default();
                        telemetry.apply_event(&event);
                        println!("{}", telemetry.summary_line());
                    }
                    None => {
                        println!("Status request (0xB0) sent; no synchronous response received on hidraw.");
                    }
                }
            }
            "--sidetone" => {
                i += 1;
                let val: u8 = args
                    .get(i)
                    .ok_or_else(|| anyhow::anyhow!("--sidetone requires 0..=100"))?
                    .parse()?;
                hidapi_send_sidetone(val)?;
            }
            "--sidetone-level" => {
                i += 1;
                let level: u8 = args
                    .get(i)
                    .ok_or_else(|| anyhow::anyhow!("--sidetone-level requires 0..=3"))?
                    .parse()?;
                cfg.hardware.sidetone_enabled = level > 0;
                cfg.hardware.sidetone_level = level.min(3);
                config_modified = true;
                hidapi_send_report(&build_sidetone_level_report(level))?;
                println!("Sidetone level set to {}", level.min(3));
            }
            "--mic-gain" => {
                i += 1;
                let level: u8 = args
                    .get(i)
                    .ok_or_else(|| anyhow::anyhow!("--mic-gain requires 0..=15"))?
                    .parse()?;
                cfg.hardware.mic_gain = level.min(15);
                config_modified = true;
                hidapi_send_report(&build_mic_gain_report(level))?;
                println!("Microphone gain set to {}", level.min(15));
            }
            "--volume-limiter" => {
                i += 1;
                let raw = args
                    .get(i)
                    .ok_or_else(|| anyhow::anyhow!("--volume-limiter requires on|off"))?;
                let enabled = parse_on_off(raw)?;
                cfg.hardware.volume_limiter = enabled;
                config_modified = true;
                hidapi_send_report(&build_volume_limiter_report(enabled))?;
                println!("Volume limiter set to {}", enabled);
            }
            "--inactivity-timer" => {
                i += 1;
                let mins: u8 = args
                    .get(i)
                    .ok_or_else(|| anyhow::anyhow!("--inactivity-timer requires minutes (0..=90)"))?
                    .parse()?;
                cfg.hardware.inactivity_timer_mins = mins.min(90);
                config_modified = true;
                hidapi_send_report(&build_inactivity_timer_report(mins))?;
                println!("Inactivity timer set to {} minutes", mins.min(90));
            }
            "--mute-led" => {
                i += 1;
                let level: u8 = args
                    .get(i)
                    .ok_or_else(|| anyhow::anyhow!("--mute-led requires 0..=3"))?
                    .parse()?;
                cfg.hardware.mute_led_brightness = level.min(3);
                config_modified = true;
                hidapi_send_report(&build_mute_led_brightness_report(level))?;
                println!("Mute LED brightness set to {}", level.min(3));
            }
            "--bt-power" => {
                i += 1;
                let raw = args
                    .get(i)
                    .ok_or_else(|| anyhow::anyhow!("--bt-power requires on|off"))?;
                let enabled = parse_on_off(raw)?;
                cfg.hardware.bt_power_on_default = enabled;
                config_modified = true;
                hidapi_send_report(&build_bt_power_default_report(enabled))?;
                println!("Bluetooth power-on default set to {}", enabled);
            }
            "--bt-call" => {
                i += 1;
                let raw = args
                    .get(i)
                    .ok_or_else(|| anyhow::anyhow!("--bt-call requires mode"))?;
                let behavior = BtCallBehavior::from_str_lossy(raw);
                cfg.hardware.bt_call_behavior = behavior;
                config_modified = true;
                hidapi_send_report(&build_bt_call_behavior_report(behavior))?;
                println!("Bluetooth call behavior set to {}", behavior.as_str());
            }
            "--apply-config" => {
                let eq = cfg_mgr.load_profile(&cfg.active_profile).ok();
                hidapi_apply_config_and_eq(&cfg, eq.as_ref(), save_flash)?;
                println!("Applied full configuration and active EQ profile to headset.");
            }
            "--save-flash" => {
                save_flash = true;
                saw_oneshot = true;
            }
            "--web-bind" => {
                i += 1;
                let bind = args
                    .get(i)
                    .ok_or_else(|| anyhow::anyhow!("--web-bind requires HOST:PORT"))?;
                cfg.web_bind = bind.trim().to_string();
                cfg.web_enabled = true;
                config_modified = true;
                run_daemon_after = true;
            }
            "--no-web" => {
                cfg.web_enabled = false;
                config_modified = true;
                run_daemon_after = true;
            }
            other => {
                anyhow::bail!("Unknown argument '{}'. Run with --help for usage.", other);
            }
        }
        if !matches!(args.get(i).map(String::as_str), Some("--web-bind") | Some("--no-web"))
            && (i == 0 || !matches!(args.get(i - 1).map(String::as_str), Some("--web-bind")))
        {
            saw_oneshot = true;
        }
        i += 1;
    }

    if config_modified {
        cfg_mgr.save_config(&cfg)?;
    }

    if save_flash {
        hidapi_save_to_flash()?;
    }

    Ok(saw_oneshot || !run_daemon_after)
}

fn main() -> Result<()> {
    env_logger::init();

    let args: Vec<String> = env::args().skip(1).collect();
    if handle_cli_args(&args)? {
        return Ok(());
    }

    info!("Initializing Arctis Nova 7 ChatMix & Hardware Controller...");

    let controller = ArctisController::new()?;
    controller.start()?;

    Ok(())
}
