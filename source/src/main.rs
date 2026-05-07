use anyhow::{Context, Result};
use ctrlc;
use env_logger;
use hidapi::HidApi;
use log::{debug, error, info, warn};
use rusb::{DeviceHandle, UsbContext};
use std::env;
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

// Arctis Nova 7 Vendor/Product IDs
const VENDOR_ID: u16 = 0x1038;
const SUPPORTED_PRODUCT_IDS: &[u16] = &[
    0x2202, // Arctis Nova 7 (discrete battery: 0-4)
    0x22A1, // Arctis Nova 7 Gen 2 (percentage battery: 0-100, Jan 2026 update)
    0x227e, // Arctis Nova 7 Wireless Gen 2 (percentage battery: 0-100)
    0x2206, // Arctis Nova 7x (discrete battery: 0-4)
    0x2258, // Arctis Nova 7x v2 (percentage battery: 0-100)
    0x229e, // Arctis Nova 7x v2 (percentage battery: 0-100)
    0x223a, // Arctis Nova 7 Diablo IV (discrete battery: 0-4, before Jan 2026 update)
    0x22a9, // Arctis Nova 7 Diablo IV (percentage battery: 0-100, after Jan 2026 update)
    0x227a  // Arctis Nova 7 WoW Edition (discrete battery: 0-4)
];
const HID_MSG_SIZE: usize = 64;

struct ArctisController {
    original_default_sink: String,
    running: Arc<AtomicBool>,
    sinks_created: Arc<AtomicBool>,
}

impl ArctisController {
    fn new() -> Result<Self> {
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
        // Hata vermemesi için çıktıları yutuyoruz, amaç temizlik.
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

    fn start(&self) -> Result<()> {
        // 1. Sanal ses cihazlarını oluşturmayı dene
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

        // 2. USB bağlantısını ve ChatMix döngüsünü başlat
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

                    // Cihaz yeniden bağlandığında sanal sinkleri tekrar bağla
                    if let Err(e) = self.relink_virtual_sinks_with_retry() {
                        warn!("Failed to relink virtual sinks after reconnect: {}", e);
                    }

                    // Girişleri Game kanalına taşı
                    if let Err(e) = move_all_inputs_to("Arctis_Game") {
                        warn!("Failed to move existing sink inputs: {}", e);
                    } else {
                        info!("Moved active audio streams to Arctis_Game");
                    }

                    // Okuma döngüsü (Read Loop)
                    let res = self.read_loop(&mut handle, endpoint);

                    // Arayüzü serbest bırak (opsiyonel, hata verirse önemli değil)
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
                        // Ses seviyelerini ayarla
                        set_sink_volume("Arctis_Game", game_vol);
                        set_sink_volume("Arctis_Chat", chat_vol);
                    }
                }
                Err(rusb::Error::Timeout) => {
                    // Timeout normaldir, veri gelmemiş olabilir.
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
                    // Hataları logla ama akışı kesme
                    if let Err(e) = link_sink_to_device("Arctis_Game", &arctis_sink) {
                        warn!("Link warning (Game): {}", e);
                    }
                    if let Err(e) = link_sink_to_device("Arctis_Chat", &arctis_sink) {
                        warn!("Link warning (Chat): {}", e);
                    }

                    // Game'i varsayılan yap
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
        // Eski varsayılan sink'e dön
        let _ = Command::new("pactl")
            .args(&["set-default-sink", &self.original_default_sink])
            .status();

        // Sanal cihazları temizle
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

fn main() -> Result<()> {
    env_logger::init();
    info!("Initializing Arctis Nova 7 ChatMix Controller...");

    let controller = ArctisController::new()?;
    controller.start()?;

    Ok(())
}

fn get_default_sink() -> Result<String> {
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

fn find_arctis_sink() -> Result<String> {
    let output = Command::new("pactl")
        .args(&["list", "short", "sinks"])
        .output()
        .context("Failed to list sinks")?;

    let sinks = String::from_utf8(output.stdout)?;
    let mut fallback: Option<String> = None;

    for line in sinks.lines() {
        let lower = line.to_lowercase();
        // "nova" veya "7" ibarelerini arıyoruz
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
fn link_sink_to_device(sink_name: &str, device_name: &str) -> Result<()> {
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
            // Eğer bağlantı zaten varsa (File exists), hata değil başarı say.
            if stderr.contains("File exists") || stderr.contains("exists") {
                debug!("Link already exists (skipping): {} -> {}", src, dst);
            } else {
                anyhow::bail!("pw-link failed for {}: {}", channel, stderr.trim());
            }
        }
    }
    Ok(())
}

fn set_sink_volume(sink_name: &str, volume_percent: u8) {
    let _ = Command::new("pactl")
        .args(&["set-sink-volume", sink_name, &format!("{}%", volume_percent)])
        .output();
}

fn move_all_inputs_to(sink_name: &str) -> Result<()> {
    let output = Command::new("pactl")
        .args(&["list", "short", "sink-inputs"])
        .output()
        .context("Failed to list sink-inputs")?;

    let stdout = String::from_utf8(output.stdout)?;
    for line in stdout.lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        if !cols.is_empty() {
            if let Ok(index) = cols[0].parse::<u32>() {
                // Hata alırsak da devam ediyoruz, bazı inputlar taşınamaz olabilir.
                let _ = Command::new("pactl")
                    .args(&["move-sink-input", &index.to_string(), sink_name])
                    .status();
            }
        }
    }
    Ok(())
}

/* ---------- hidapi sidetone write ---------- */
fn try_hidapi_sidetone_from_env() {
    // Sidetone ayarı ortam değişkeninden okunur
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
fn usb_find_and_open<T: UsbContext>(usb_ctx: &T) -> Result<(DeviceHandle<T>, u8, u8)> {
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
