use crate::audio::{find_arctis_sink, get_default_sink, link_sink_to_device, move_all_inputs_to, set_sink_volume};
use crate::usb::usb_find_and_open;
use anyhow::{Context, Result};
use log::{debug, error, info, warn};
use rusb::{DeviceHandle, UsbContext};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

pub(crate) struct ArctisController {
    original_default_sink: String,
    running: Arc<AtomicBool>,
    sinks_created: Arc<AtomicBool>,
}

impl ArctisController {
    pub(crate) fn new() -> Result<Self> {
        let original_default_sink = get_default_sink().unwrap_or_else(|_| "auto_null".to_string());
        info!("Original default sink: {}", original_default_sink);

        let running = Arc::new(AtomicBool::new(true));
        let r = running.clone();

        ctrlc::set_handler(move || {
            r.store(false, Ordering::SeqCst);
        })
        .context("Failed to set Ctrl+C handler")?;

        Ok(Self {
            original_default_sink,
            running,
            sinks_created: Arc::new(AtomicBool::new(false)),
        })
    }

    fn setup_virtual_sinks(&self) -> Result<()> {
        if self.sinks_created.load(Ordering::SeqCst) {
            info!("Virtual sinks already exist, skipping creation");
            return Ok(());
        }

        let arctis_sink = find_arctis_sink().context("Arctis Nova 7 device not found in audio system")?;
        info!("Found Physical Sink: {}", arctis_sink);

        info!("Cleaning up old virtual sinks (if any)...");
        // Suppress output and errors — we just want a clean slate.
        let _ = Command::new("pw-cli")
            .args(&["destroy", "Arctis_Game"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = Command::new("pw-cli")
            .args(&["destroy", "Arctis_Chat"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        std::thread::sleep(Duration::from_millis(500));

        info!("Creating virtual sinks for Arctis Nova 7...");
        
        // GAME SINK
        let game_result = Command::new("pw-cli")
            .args(&[
                "create-node",
                "adapter",
                r#"{factory.name=support.null-audio-sink node.name=Arctis_Game node.description="Arctis Nova 7 Game" media.class=Audio/Sink monitor.channel-volumes=true object.linger=true audio.position=[FL FR]}"#
            ])
            .stdout(Stdio::null())
            .status()
            .context("Failed to create Game sink")?;

        if !game_result.success() {
            anyhow::bail!("Failed to create Arctis_Game sink");
        }

        // CHAT SINK
        let chat_result = Command::new("pw-cli")
            .args(&[
                "create-node",
                "adapter",
                r#"{factory.name=support.null-audio-sink node.name=Arctis_Chat node.description="Arctis Nova 7 Chat" media.class=Audio/Sink monitor.channel-volumes=true object.linger=true audio.position=[FL FR]}"#
            ])
            .stdout(Stdio::null())
            .status()
            .context("Failed to create Chat sink")?;

        if !chat_result.success() {
            anyhow::bail!("Failed to create Arctis_Chat sink");
        }

        std::thread::sleep(Duration::from_millis(1000));

        info!("Linking virtual sinks to headset...");
        link_sink_to_device("Arctis_Game", &arctis_sink)?;
        link_sink_to_device("Arctis_Chat", &arctis_sink)?;

        info!("Setting Arctis_Game as default sink...");
        let _ = Command::new("pactl")
            .args(&["set-default-sink", "Arctis_Game"])
            .status();

        self.sinks_created.store(true, Ordering::SeqCst);
        info!("Setup complete! Virtual sinks are ready.");

        Ok(())
    }

    pub(crate) fn start(&self) -> Result<()> {
        // 1. Attempt to create virtual audio sinks
        loop {
            if !self.running.load(Ordering::SeqCst) {
                return Ok(());
            }

            info!("Waiting for Arctis Nova 7 audio device...");
            match self.setup_virtual_sinks() {
                Ok(_) => break,
                Err(e) => {
                    warn!("Audio setup failed: {}. Retrying in 3 seconds...", e);
                    std::thread::sleep(Duration::from_secs(3));
                }
            }
        }

        // 2. Start USB connection and ChatMix read loop
        loop {
            if !self.running.load(Ordering::SeqCst) {
                break;
            }

            match self.try_connect_and_run() {
                Ok(_) => {
                    info!("Connection loop ended gracefully; exiting main loop");
                    break;
                }
                Err(e) => {
                    if !self.running.load(Ordering::SeqCst) {
                        break;
                    }

                    warn!("USB connection lost / error: {}", e);
                    info!("Waiting for reconnection...");
                    std::thread::sleep(Duration::from_secs(3));
                }
            }
        }

        Ok(())
    }

    fn try_connect_and_run(&self) -> Result<()> {
        let usb_ctx = rusb::Context::new().context("Failed to initialize libusb context")?;

        while self.running.load(Ordering::SeqCst) {
            match usb_find_and_open(&usb_ctx) {
                Ok((mut handle, endpoint, interface_num)) => {
                    info!("{}", "=".repeat(50));
                    info!("Arctis Nova 7 ChatMix Connected!");
                    info!("  • Arctis_Game -> Game Audio");
                    info!("  • Arctis_Chat -> Chat Audio");
                    info!("{}", "=".repeat(50));

                    // Re-link virtual sinks after device reconnect
                    if let Err(e) = self.relink_virtual_sinks_with_retry() {
                        warn!("Failed to relink virtual sinks after reconnect: {}", e);
                    }

                    // Move active sink inputs to the Game channel
                    if let Err(e) = move_all_inputs_to("Arctis_Game") {
                        warn!("Failed to move existing sink inputs: {}", e);
                    } else {
                        info!("Moved active audio streams to Arctis_Game");
                    }

                    // Read Loop
                    let res = self.read_loop(&mut handle, endpoint);

                    // Release the interface (optional, errors here are non-fatal)
                    let _ = handle.release_interface(interface_num);

                    return res;
                }
                Err(e) => {
                    if !self.running.load(Ordering::SeqCst) {
                        break;
                    }
                    debug!("usb_find_and_open failed: {:?}", e);
                    std::thread::sleep(Duration::from_secs(2));
                    continue;
                }
            }
        }

        Ok(())
    }

    fn read_loop<T: UsbContext>(&self, handle: &mut DeviceHandle<T>, endpoint: u8) -> Result<()> {
        let mut buf = [0u8; 64];
        let mut consecutive_errors = 0u32;
        const MAX_ERRORS: u32 = 5;

        while self.running.load(Ordering::SeqCst) {
            match handle.read_interrupt(endpoint, &mut buf, Duration::from_millis(1000)) {
                Ok(len) => {
                    consecutive_errors = 0;

                    if len >= 3 && buf[0] == 0x45 {
                        let game_vol = buf[1];
                        let chat_vol = buf[2];
                        // Apply volume levels
                        set_sink_volume("Arctis_Game", game_vol);
                        set_sink_volume("Arctis_Chat", chat_vol);
                    }
                }
                Err(rusb::Error::Timeout) => {
                    // Timeout is expected — no data received in this interval.
                    consecutive_errors = 0;
                    continue;
                }
                Err(rusb::Error::NoDevice) => {
                    error!("Device disconnected (NoDevice)");
                    return Err(anyhow::anyhow!("USB device disconnected (NoDevice)"));
                }
                Err(rusb::Error::Io) => {
                    consecutive_errors += 1;
                    warn!("USB I/O error (attempt {}/{})", consecutive_errors, MAX_ERRORS);
                    if consecutive_errors >= MAX_ERRORS {
                        return Err(anyhow::anyhow!("Too many USB I/O errors"));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => {
                    consecutive_errors += 1;
                    warn!("USB error: {:?} (attempt {}/{})", e, consecutive_errors, MAX_ERRORS);
                    if consecutive_errors >= MAX_ERRORS {
                        return Err(anyhow::anyhow!("Too many USB errors: {:?}", e));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
        Ok(())
    }

    fn relink_virtual_sinks_with_retry(&self) -> Result<()> {
        const RETRIES: usize = 10;
        for attempt in 1..=RETRIES {
            if !self.running.load(Ordering::SeqCst) {
                anyhow::bail!("Shutdown in progress");
            }

            match find_arctis_sink() {
                Ok(arctis_sink) => {
                    info!("Relinking virtual sinks to '{}'", arctis_sink);
                    // Log warnings but don't abort the flow
                    if let Err(e) = link_sink_to_device("Arctis_Game", &arctis_sink) {
                        warn!("Link warning (Game): {}", e);
                    }
                    if let Err(e) = link_sink_to_device("Arctis_Chat", &arctis_sink) {
                        warn!("Link warning (Chat): {}", e);
                    }

                    // Set Game sink as the default
                    let _ = Command::new("pactl")
                        .args(&["set-default-sink", "Arctis_Game"])
                        .status();

                    std::thread::sleep(Duration::from_millis(300));
                    return Ok(());
                }
                Err(e) => {
                    debug!("Retry {}/{}: Could not find sink: {}", attempt, RETRIES, e);
                    std::thread::sleep(Duration::from_millis(300));
                    continue;
                }
            }
        }
        anyhow::bail!("Failed to locate Arctis sink after retries");
    }

    fn cleanup(&self) {
        info!("Shutting down...");
        // Restore the original default sink
        let _ = Command::new("pactl")
            .args(&["set-default-sink", &self.original_default_sink])
            .status();

        // Destroy virtual sink nodes
        let _ = Command::new("pw-cli").args(&["destroy", "Arctis_Game"]).stdout(Stdio::null()).status();
        let _ = Command::new("pw-cli").args(&["destroy", "Arctis_Chat"]).stdout(Stdio::null()).status();

        info!("Arctis Nova 7 ChatMix shut down.");
    }
}

impl Drop for ArctisController {
    fn drop(&mut self) {
        self.cleanup();
    }
}
