// Stage 3: Embedded HTTP Server & REST/SSE API (`127.0.0.1:3927`).
//
// Zero-external-dependency HTTP/1.1 server built on `std::net::TcpListener`.
// Exposes:
// - `GET  /api/status`     -> Full headset telemetry, ChatMix, active EQ, and hardware settings
// - `GET  /api/eq`         -> Active 10-band Parametric EQ profile
// - `POST /api/eq`         -> Apply custom 10-band gains/bands or switch preset (`0x33`)
// - `GET  /api/presets`    -> List all available built-in and user EQ presets
// - `POST /api/presets`    -> Activate a named preset or save a new custom preset
// - `GET  /api/settings`   -> Current hardware and daemon settings
// - `POST /api/settings`   -> Update hardware controls (`0x37`, `0x39`, `0x3A`, `0xA3`, `0xAE`, `0xB2`, `0xB3`)
// - `GET  /api/sidetone`   -> Current sidetone state
// - `POST /api/sidetone`   -> Update sidetone level (`0..=3`) or percentage (`0..=100`, `0x39`)
// - `POST /api/save-flash` -> Persist active DSP & hardware settings to onboard flash (`0x09`)
// - `GET  /api/events`     -> Server-Sent Events (SSE) stream for live telemetry & ChatMix updates

use crate::device::BtCallBehavior;
use crate::eq::{
    EqBand, EqFilterType, ParametricEqProfile, DEFAULT_Q_FACTOR, EQ_BAND_COUNT, EQ_FREQUENCIES_HZ,
    MAX_EQ_GAIN_DB, MIN_EQ_GAIN_DB,
};
use crate::state::{
    config_to_settings_json, eq_profile_to_json, escape_json, load_all_presets, DeviceCommand,
    DeviceWorkerHandle, HardwareSettingsPatch,
};
use anyhow::{Context, Result};
use log::{debug, info, warn};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, RecvTimeoutError},
    Arc,
};
use std::thread;
use std::time::Duration;

const CMD_REPLY_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_HTTP_BODY_BYTES: usize = 64 * 1024;

/// Spawns the embedded HTTP server on a background thread bound to `bind_addr` (default `127.0.0.1:3927`).
pub fn spawn_web_server(
    bind_addr: String,
    worker_handle: DeviceWorkerHandle,
    running: Arc<AtomicBool>,
) -> Result<thread::JoinHandle<()>> {
    let listener = TcpListener::bind(&bind_addr)
        .with_context(|| format!("Failed to bind embedded HTTP server on {}", bind_addr))?;
    listener
        .set_nonblocking(true)
        .context("Failed to set non-blocking mode on HTTP TcpListener")?;

    info!("Embedded Web API & SSE server listening on http://{}", bind_addr);

    let handle = thread::Builder::new()
        .name("arctis-web-api".to_string())
        .spawn(move || {
            while running.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, peer)) => {
                        debug!("HTTP connection from {}", peer);
                        let h = worker_handle.clone();
                        let r = running.clone();
                        thread::spawn(move || {
                            if let Err(e) = handle_client(stream, h, r) {
                                debug!("HTTP client error ({}): {}", peer, e);
                            }
                        });
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(25));
                    }
                    Err(e) => {
                        warn!("HTTP accept error: {}", e);
                        thread::sleep(Duration::from_millis(100));
                    }
                }
            }
            info!("Embedded Web API server stopped.");
        })
        .context("Failed to spawn web server thread")?;

    Ok(handle)
}

fn handle_client(
    mut stream: TcpStream,
    worker: DeviceWorkerHandle,
    running: Arc<AtomicBool>,
) -> Result<()> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(());
    }

    let parts: Vec<&str> = request_line.trim().split_whitespace().collect();
    if parts.len() < 2 {
        return write_json_response(&mut stream, 400, r#"{"error":"Malformed HTTP request"}"#);
    }

    let method = parts[0].to_ascii_uppercase();
    let raw_target = parts[1];
    let path = raw_target.split('?').next().unwrap_or(raw_target);

    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                if let Ok(len) = v.trim().parse::<usize>() {
                    content_length = len.min(MAX_HTTP_BODY_BYTES);
                }
            }
        }
    }

    if method == "OPTIONS" {
        return write_cors_preflight(&mut stream);
    }

    let mut body_bytes = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body_bytes)?;
    }
    let body = String::from_utf8_lossy(&body_bytes).to_string();

    match (method.as_str(), path) {
        ("GET", "/api/events") => handle_sse_stream(stream, worker, running),
        ("GET", "/api/status") => {
            if extract_json_bool(&body, "refresh").unwrap_or(false)
                || raw_target.contains("refresh=1")
                || raw_target.contains("refresh=true")
            {
                let (tx, rx) = mpsc::channel();
                let _ = worker.send_command(DeviceCommand::RequestStatus { reply: tx });
                let _ = rx.recv_timeout(Duration::from_millis(400));
            }
            let json = worker
                .state
                .read()
                .map(|s| s.to_status_json())
                .unwrap_or_else(|_| r#"{"error":"State lock poisoned"}"#.to_string());
            write_json_response(&mut stream, 200, &json)
        }
        ("POST", "/api/status") => {
            let (tx, rx) = mpsc::channel();
            worker.send_command(DeviceCommand::RequestStatus { reply: tx })?;
            let _ = rx.recv_timeout(CMD_REPLY_TIMEOUT);
            let json = worker
                .state
                .read()
                .map(|s| s.to_status_json())
                .unwrap_or_else(|_| r#"{"error":"State lock poisoned"}"#.to_string());
            write_json_response(&mut stream, 200, &json)
        }
        ("GET", "/api/eq") => {
            let json = worker
                .state
                .read()
                .map(|s| {
                    format!(
                        "{{\"active_profile\":\"{}\",\"profile\":{}}}",
                        escape_json(&s.config.active_profile),
                        eq_profile_to_json(&s.active_eq)
                    )
                })
                .unwrap_or_else(|_| r#"{"error":"State lock poisoned"}"#.to_string());
            write_json_response(&mut stream, 200, &json)
        }
        ("POST", "/api/eq") | ("PUT", "/api/eq") => handle_post_eq(&mut stream, &worker, &body),
        ("GET", "/api/presets") => {
            let presets = load_all_presets(&worker.config_manager);
            let active_name = worker
                .state
                .read()
                .map(|s| s.config.active_profile.clone())
                .unwrap_or_else(|_| "harman".to_string());
            let items: Vec<String> = presets.iter().map(eq_profile_to_json).collect();
            let json = format!(
                "{{\"active_profile\":\"{}\",\"presets\":[{}]}}",
                escape_json(&active_name),
                items.join(",")
            );
            write_json_response(&mut stream, 200, &json)
        }
        ("POST", "/api/presets") | ("PUT", "/api/presets") => {
            handle_post_presets(&mut stream, &worker, &body)
        }
        ("GET", "/api/settings") => {
            let json = worker
                .state
                .read()
                .map(|s| config_to_settings_json(&s.config))
                .unwrap_or_else(|_| r#"{"error":"State lock poisoned"}"#.to_string());
            write_json_response(&mut stream, 200, &json)
        }
        ("POST", "/api/settings") | ("PUT", "/api/settings") | ("PATCH", "/api/settings") => {
            handle_post_settings(&mut stream, &worker, &body)
        }
        ("GET", "/api/sidetone") => {
            let json = worker
                .state
                .read()
                .map(|s| {
                    format!(
                        "{{\"enabled\":{},\"level\":{}}}",
                        s.config.hardware.sidetone_enabled, s.config.hardware.sidetone_level
                    )
                })
                .unwrap_or_else(|_| r#"{"error":"State lock poisoned"}"#.to_string());
            write_json_response(&mut stream, 200, &json)
        }
        ("POST", "/api/sidetone") | ("PUT", "/api/sidetone") => {
            handle_post_sidetone(&mut stream, &worker, &body)
        }
        ("POST", "/api/save-flash") => handle_post_save_flash(&mut stream, &worker),
        ("GET", "/") | ("GET", "/index.html") => write_html_response(&mut stream, 200, DEFAULT_INDEX_HTML),
        _ => write_json_response(&mut stream, 404, r#"{"error":"Endpoint not found"}"#),
    }
}

fn handle_sse_stream(
    mut stream: TcpStream,
    worker: DeviceWorkerHandle,
    running: Arc<AtomicBool>,
) -> Result<()> {
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let headers = concat!(
        "HTTP/1.1 200 OK\r\n",
        "Content-Type: text/event-stream; charset=utf-8\r\n",
        "Cache-Control: no-cache, no-transform\r\n",
        "Connection: keep-alive\r\n",
        "Access-Control-Allow-Origin: *\r\n",
        "X-Accel-Buffering: no\r\n\r\n"
    );
    stream.write_all(headers.as_bytes())?;

    // Send initial status snapshot immediately on connect
    if let Ok(st) = worker.state.read() {
        let initial = format!("event: status\ndata: {}\n\n", st.to_status_json());
        stream.write_all(initial.as_bytes())?;
    }
    stream.flush()?;

    let rx = worker.broadcaster.subscribe();
    while running.load(Ordering::SeqCst) {
        match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(msg) => {
                if stream.write_all(msg.as_bytes()).is_err() || stream.flush().is_err() {
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                // Keep-alive comment frame
                if stream.write_all(b": keep-alive\n\n").is_err() || stream.flush().is_err() {
                    break;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    Ok(())
}

fn handle_post_eq(
    stream: &mut TcpStream,
    worker: &DeviceWorkerHandle,
    body: &str,
) -> Result<()> {
    // Case 1: Only a preset/profile name was provided without custom gains -> activate preset
    let has_gains = extract_json_f32_array(body, "gains_db").is_some()
        || extract_json_f32_array(body, "gains").is_some()
        || extract_json_array_raw(body, "bands").is_some();

    if !has_gains {
        if let Some(preset_name) = extract_json_string(body, "preset")
            .or_else(|| extract_json_string(body, "name"))
            .or_else(|| extract_json_string(body, "active_profile"))
        {
            let (tx, rx) = mpsc::channel();
            worker.send_command(DeviceCommand::ActivatePreset {
                name: preset_name,
                reply: tx,
            })?;
            return match rx.recv_timeout(CMD_REPLY_TIMEOUT) {
                Ok(Ok(profile)) => {
                    let resp = format!(
                        "{{\"ok\":true,\"active_profile\":\"{}\",\"profile\":{}}}",
                        escape_json(&profile.name),
                        eq_profile_to_json(&profile)
                    );
                    write_json_response(stream, 200, &resp)
                }
                Ok(Err(e)) => write_error_response(stream, 400, &e.to_string()),
                Err(_) => write_error_response(stream, 504, "Timed out waiting for USB worker"),
            };
        }
    }

    // Case 2: Parse 10-band EQ profile from `bands` or `gains_db` / `gains`
    let profile = match parse_eq_profile_from_json(body) {
        Ok(p) => p,
        Err(e) => return write_error_response(stream, 400, &e.to_string()),
    };
    let save_to_disk = extract_json_bool(body, "save").unwrap_or(true);

    let (tx, rx) = mpsc::channel();
    worker.send_command(DeviceCommand::ApplyEq {
        profile: profile.clone(),
        save_profile_to_disk: save_to_disk,
        reply: tx,
    })?;

    match rx.recv_timeout(CMD_REPLY_TIMEOUT) {
        Ok(Ok(())) => {
            let resp = format!(
                "{{\"ok\":true,\"active_profile\":\"{}\",\"profile\":{}}}",
                escape_json(&profile.name),
                eq_profile_to_json(&profile)
            );
            write_json_response(stream, 200, &resp)
        }
        Ok(Err(e)) => write_error_response(stream, 500, &e.to_string()),
        Err(_) => write_error_response(stream, 504, "Timed out waiting for USB worker"),
    }
}

fn handle_post_presets(
    stream: &mut TcpStream,
    worker: &DeviceWorkerHandle,
    body: &str,
) -> Result<()> {
    // If body includes gains/bands, save & activate as a named preset
    let has_gains = extract_json_f32_array(body, "gains_db").is_some()
        || extract_json_f32_array(body, "gains").is_some()
        || extract_json_array_raw(body, "bands").is_some();

    if has_gains {
        return handle_post_eq(stream, worker, body);
    }

    let Some(name) = extract_json_string(body, "name")
        .or_else(|| extract_json_string(body, "preset"))
        .or_else(|| extract_json_string(body, "active_profile"))
    else {
        return write_error_response(
            stream,
            400,
            "Missing 'name' or 'preset' field in JSON body",
        );
    };

    let (tx, rx) = mpsc::channel();
    worker.send_command(DeviceCommand::ActivatePreset { name, reply: tx })?;

    match rx.recv_timeout(CMD_REPLY_TIMEOUT) {
        Ok(Ok(profile)) => {
            let resp = format!(
                "{{\"ok\":true,\"active_profile\":\"{}\",\"profile\":{}}}",
                escape_json(&profile.name),
                eq_profile_to_json(&profile)
            );
            write_json_response(stream, 200, &resp)
        }
        Ok(Err(e)) => write_error_response(stream, 400, &e.to_string()),
        Err(_) => write_error_response(stream, 504, "Timed out waiting for USB worker"),
    }
}

fn handle_post_sidetone(
    stream: &mut TcpStream,
    worker: &DeviceWorkerHandle,
    body: &str,
) -> Result<()> {
    let level = extract_json_u64(body, "level")
        .or_else(|| extract_json_u64(body, "sidetone_level"))
        .map(|v| v.min(3) as u8);
    let percent = extract_json_u64(body, "percent")
        .or_else(|| extract_json_u64(body, "sidetone_percent"))
        .map(|v| v.min(100) as u8);
    let enabled_opt = extract_json_bool(body, "enabled")
        .or_else(|| extract_json_bool(body, "sidetone_enabled"));
    let enabled = enabled_opt.unwrap_or_else(|| match (level, percent) {
        (Some(0), None) | (None, Some(0)) => false,
        _ => true,
    });

    if level.is_none() && percent.is_none() && enabled_opt.is_none() {
        return write_error_response(
            stream,
            400,
            "Expected 'level' (0..3), 'percent' (0..100), or 'enabled' (bool)",
        );
    }

    let (tx, rx) = mpsc::channel();
    worker.send_command(DeviceCommand::SetSidetone {
        enabled,
        level,
        percent,
        reply: tx,
    })?;

    match rx.recv_timeout(CMD_REPLY_TIMEOUT) {
        Ok(Ok(applied_level)) => {
            let resp = format!(
                "{{\"ok\":true,\"enabled\":{},\"level\":{}}}",
                applied_level > 0,
                applied_level
            );
            write_json_response(stream, 200, &resp)
        }
        Ok(Err(e)) => write_error_response(stream, 500, &e.to_string()),
        Err(_) => write_error_response(stream, 504, "Timed out waiting for USB worker"),
    }
}

fn handle_post_settings(
    stream: &mut TcpStream,
    worker: &DeviceWorkerHandle,
    body: &str,
) -> Result<()> {
    let patch = HardwareSettingsPatch {
        sidetone_enabled: extract_json_bool(body, "sidetone_enabled"),
        sidetone_level: extract_json_u64(body, "sidetone_level").map(|v| v.min(3) as u8),
        mic_gain: extract_json_u64(body, "mic_gain").map(|v| v.min(15) as u8),
        volume_limiter: extract_json_bool(body, "volume_limiter"),
        inactivity_timer_mins: extract_json_u64(body, "inactivity_timer_mins")
            .or_else(|| extract_json_u64(body, "sleep_timer_mins"))
            .map(|v| v.min(90) as u8),
        mute_led_brightness: extract_json_u64(body, "mute_led_brightness").map(|v| v.min(3) as u8),
        bt_power_on_default: extract_json_bool(body, "bt_power_on_default"),
        bt_call_behavior: extract_json_string(body, "bt_call_behavior")
            .map(|s| BtCallBehavior::from_str_lossy(&s)),
        auto_apply_on_connect: extract_json_bool(body, "auto_apply_on_connect"),
        save_to_flash: extract_json_bool(body, "save_to_flash"),
        poll_status_interval_secs: extract_json_u64(body, "poll_status_interval_secs"),
    };

    let (tx, rx) = mpsc::channel();
    worker.send_command(DeviceCommand::UpdateSettings { patch, reply: tx })?;

    match rx.recv_timeout(CMD_REPLY_TIMEOUT) {
        Ok(Ok(_)) => {
            let json = worker
                .state
                .read()
                .map(|s| format!("{{\"ok\":true,\"settings\":{}}}", config_to_settings_json(&s.config)))
                .unwrap_or_else(|_| r#"{"ok":true}"#.to_string());
            write_json_response(stream, 200, &json)
        }
        Ok(Err(e)) => write_error_response(stream, 500, &e.to_string()),
        Err(_) => write_error_response(stream, 504, "Timed out waiting for USB worker"),
    }
}

fn handle_post_save_flash(stream: &mut TcpStream, worker: &DeviceWorkerHandle) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    worker.send_command(DeviceCommand::SaveToFlash { reply: tx })?;

    match rx.recv_timeout(CMD_REPLY_TIMEOUT) {
        Ok(Ok(())) => write_json_response(
            stream,
            200,
            r#"{"ok":true,"message":"Settings saved to onboard flash (0x09)"}"#,
        ),
        Ok(Err(e)) => write_error_response(stream, 500, &e.to_string()),
        Err(_) => write_error_response(stream, 504, "Timed out waiting for USB worker"),
    }
}

/* ---------- Lightweight JSON Parsers ---------- */

fn parse_eq_profile_from_json(body: &str) -> Result<ParametricEqProfile> {
    let name = extract_json_string(body, "name")
        .or_else(|| extract_json_string(body, "preset"))
        .unwrap_or_else(|| "custom".to_string());
    let description = extract_json_string(body, "description")
        .unwrap_or_else(|| "Custom 10-band Parametric EQ profile".to_string());

    // Check if full `bands: [ { ... }, ... ]` array is provided
    if let Some(bands_slice) = extract_json_array_raw(body, "bands") {
        let objects = split_json_objects(bands_slice);
        if !objects.is_empty() {
            if objects.len() != EQ_BAND_COUNT {
                anyhow::bail!(
                    "Expected exactly {} EQ bands in 'bands', got {}",
                    EQ_BAND_COUNT,
                    objects.len()
                );
            }
            let mut bands = [EqBand::new(0, 0.0); EQ_BAND_COUNT];
            for (i, obj) in objects.iter().enumerate() {
                let freq = extract_json_u64(obj, "frequency_hz")
                    .or_else(|| extract_json_u64(obj, "frequency"))
                    .map(|v| v as u16)
                    .unwrap_or(EQ_FREQUENCIES_HZ[i]);
                let gain = extract_json_f32(obj, "gain_db")
                    .or_else(|| extract_json_f32(obj, "gain"))
                    .unwrap_or(0.0)
                    .clamp(MIN_EQ_GAIN_DB, MAX_EQ_GAIN_DB);
                let q = extract_json_f32(obj, "q_factor")
                    .or_else(|| extract_json_f32(obj, "q"))
                    .unwrap_or(DEFAULT_Q_FACTOR)
                    .clamp(0.1, 10.0);
                let filter = extract_json_string(obj, "filter_type")
                    .map(|s| EqFilterType::from_str_lossy(&s))
                    .unwrap_or(EqFilterType::Peaking);
                bands[i] = EqBand::with_q_and_filter(freq, gain, q, filter);
            }
            return Ok(ParametricEqProfile {
                name,
                description,
                bands,
            });
        }

        // Empty objects array: allow `bands` as a 10-float array
        if let Some(gains_vec) = extract_json_f32_array(body, "bands") {
            if gains_vec.len() != EQ_BAND_COUNT {
                anyhow::bail!(
                    "Expected exactly {} EQ bands in 'bands', got {}",
                    EQ_BAND_COUNT,
                    gains_vec.len()
                );
            }
            let mut bands = [EqBand::new(0, 0.0); EQ_BAND_COUNT];
            let qs_vec = extract_json_f32_array(body, "q_factors");
            for i in 0..EQ_BAND_COUNT {
                let gain = gains_vec[i].clamp(MIN_EQ_GAIN_DB, MAX_EQ_GAIN_DB);
                let q = qs_vec
                    .as_ref()
                    .and_then(|v| v.get(i).copied())
                    .unwrap_or(DEFAULT_Q_FACTOR)
                    .clamp(0.1, 10.0);
                bands[i] = EqBand::with_q_and_filter(EQ_FREQUENCIES_HZ[i], gain, q, EqFilterType::Peaking);
            }
            return Ok(ParametricEqProfile {
                name,
                description,
                bands,
            });
        }
    }

    let gains_vec = extract_json_f32_array(body, "gains_db")
        .or_else(|| extract_json_f32_array(body, "gains"))
        .ok_or_else(|| {
            anyhow::anyhow!("JSON body must include 'gains_db' (10 floats) or 'bands' (10 objects)")
        })?;

    if gains_vec.len() != EQ_BAND_COUNT {
        anyhow::bail!(
            "Expected exactly {} EQ bands in 'gains_db', got {}",
            EQ_BAND_COUNT,
            gains_vec.len()
        );
    }

    let qs_vec = extract_json_f32_array(body, "q_factors");
    let mut bands = [EqBand::new(0, 0.0); EQ_BAND_COUNT];
    for i in 0..EQ_BAND_COUNT {
        let gain = gains_vec[i].clamp(MIN_EQ_GAIN_DB, MAX_EQ_GAIN_DB);
        let q = qs_vec
            .as_ref()
            .and_then(|v| v.get(i).copied())
            .unwrap_or(DEFAULT_Q_FACTOR)
            .clamp(0.1, 10.0);
        bands[i] = EqBand::with_q_and_filter(EQ_FREQUENCIES_HZ[i], gain, q, EqFilterType::Peaking);
    }

    Ok(ParametricEqProfile {
        name,
        description,
        bands,
    })
}

fn find_json_value_slice<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\\\"{}\\\"", key);
    let mut in_string = false;
    let mut escaped = false;
    let mut i = 0;

    while i < json.len() {
        let ch = json[i..].chars().next()?;
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            i += ch.len_utf8();
            continue;
        }

        if ch == '"' {
            if json[i..].starts_with(&needle) {
                let after_key = i + needle.len();
                let rest = json[after_key..].trim_start();
                if let Some(after_colon) = rest.strip_prefix(':') {
                    return Some(after_colon.trim_start());
                }
            }
            in_string = true;
            i += ch.len_utf8();
            continue;
        }

        i += ch.len_utf8();
    }
    None
}

pub(crate) fn extract_json_string(json: &str, key: &str) -> Option<String> {
    let val = find_json_value_slice(json, key)?;
    let inner = val.strip_prefix('"')?;
    let mut out = String::new();
    let mut escaped = false;
    for ch in inner.chars() {
        if escaped {
            match ch {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                other => out.push(other),
            }
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some(out);
        } else {
            out.push(ch);
        }
    }
    None
}

pub(crate) fn extract_json_bool(json: &str, key: &str) -> Option<bool> {
    let val = find_json_value_slice(json, key)?;
    if val.starts_with("true") {
        Some(true)
    } else if val.starts_with("false") {
        Some(false)
    } else if val.starts_with('"') {
        match extract_json_string(json, key)?.to_ascii_lowercase().as_str() {
            "true" | "1" | "on" | "yes" => Some(true),
            "false" | "0" | "off" | "no" => Some(false),
            _ => None,
        }
    } else {
        None
    }
}

pub(crate) fn extract_json_u64(json: &str, key: &str) -> Option<u64> {
    let val = find_json_value_slice(json, key)?;
    let token: String = val
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    token.parse::<u64>().ok()
}

pub(crate) fn extract_json_f32(json: &str, key: &str) -> Option<f32> {
    let val = find_json_value_slice(json, key)?;
    let token: String = val
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-' || *c == '+' || *c == '.')
        .collect();
    token.parse::<f32>().ok()
}

pub(crate) fn extract_json_array_raw<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let val = find_json_value_slice(json, key)?;
    let after_open = val.strip_prefix('[')?;
    let mut depth = 1usize;
    for (idx, ch) in after_open.char_indices() {
        if ch == '[' {
            depth += 1;
        } else if ch == ']' {
            depth -= 1;
            if depth == 0 {
                return Some(&after_open[..idx]);
            }
        }
    }
    None
}

pub(crate) fn extract_json_f32_array(json: &str, key: &str) -> Option<Vec<f32>> {
    let raw = extract_json_array_raw(json, key)?;
    let mut out = Vec::new();
    for part in raw.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parsed = trimmed.parse::<f32>().ok()?;
        out.push(parsed);
    }
    Some(out)
}

fn split_json_objects(raw: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    for (idx, ch) in raw.char_indices() {
        if ch == '{' {
            if depth == 0 {
                start = Some(idx);
            }
            depth += 1;
        } else if ch == '}' {
            if depth > 0 {
                depth -= 1;
                if depth == 0 {
                    if let Some(s) = start {
                        out.push(&raw[s..=idx]);
                        start = None;
                    }
                }
            }
        }
    }
    out
}

/* ---------- HTTP Response Helpers ---------- */

fn status_reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        504 => "Gateway Timeout",
        _ => "OK",
    }
}

fn write_cors_preflight(stream: &mut TcpStream) -> Result<()> {
    let resp = concat!(
        "HTTP/1.1 204 No Content\r\n",
        "Access-Control-Allow-Origin: *\r\n",
        "Access-Control-Allow-Methods: GET, POST, PUT, PATCH, OPTIONS\r\n",
        "Access-Control-Allow-Headers: Content-Type\r\n",
        "Content-Length: 0\r\n",
        "Connection: close\r\n\r\n"
    );
    stream.write_all(resp.as_bytes())?;
    stream.flush()?;
    Ok(())
}

fn write_json_response(stream: &mut TcpStream, status: u16, json_body: &str) -> Result<()> {
    let bytes = json_body.as_bytes();
    let header = format!(
        concat!(
            "HTTP/1.1 {} {}\r\n",
            "Content-Type: application/json; charset=utf-8\r\n",
            "Access-Control-Allow-Origin: *\r\n",
            "Access-Control-Allow-Methods: GET, POST, PUT, PATCH, OPTIONS\r\n",
            "Access-Control-Allow-Headers: Content-Type\r\n",
            "Content-Length: {}\r\n",
            "Connection: close\r\n\r\n"
        ),
        status,
        status_reason(status),
        bytes.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(bytes)?;
    stream.flush()?;
    Ok(())
}

fn write_html_response(stream: &mut TcpStream, status: u16, html_body: &str) -> Result<()> {
    let bytes = html_body.as_bytes();
    let header = format!(
        concat!(
            "HTTP/1.1 {} {}\r\n",
            "Content-Type: text/html; charset=utf-8\r\n",
            "Content-Length: {}\r\n",
            "Connection: close\r\n\r\n"
        ),
        status,
        status_reason(status),
        bytes.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(bytes)?;
    stream.flush()?;
    Ok(())
}

fn write_error_response(stream: &mut TcpStream, status: u16, message: &str) -> Result<()> {
    let json = format!("{{\"ok\":false,\"error\":\"{}\"}}", escape_json(message));
    write_json_response(stream, status, &json)
}

const DEFAULT_INDEX_HTML: &str = include_str!("../web/index.html");

