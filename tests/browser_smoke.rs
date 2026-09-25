// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License
//! Headless-Chromium integration smoke for Mosaic Studio: one self-contained
//! file driving the REAL released studio (real server binary, real committed
//! wasm build, real worker) in a real Chromium over CDP. It ports the
//! properties of the former `tests/browser_smoke.cjs` and
//! `tests/photo_smoke.cjs` playwright smokes to plain `cargo test`.
//!
//! ## Harness credit
//!
//! The CDP primitives — hand-rolled WebSocket framing over a `TcpStream`,
//! `/json` discovery over plain HTTP, `Page.navigate`, `Runtime.evaluate`
//! with `returnByValue` (+ `awaitPromise`), `chromium_available` gating,
//! process-group SIGKILL on drop and the bind-then-release free-port pick —
//! are adapted from the bot project's committed harness
//! (`bot/tests/browser_js.rs`). The duplication is deliberate: this crate is
//! a separate project with its own lockfile, and the harness must stay
//! std-only plus `serde_json`.
//!
//! ## Shape
//!
//! Each test spawns its own `lego-mosaic` server on a free loopback port
//! (`LEGO_MOSAIC_ADDR`, `/healthz` polled, killed on drop) and its own
//! headless Chromium (unique temp profile, loopback CDP, own process group,
//! SIGKILL of the whole group on drop). Uncaught page exceptions and
//! `console.error` are recorded wherever they surface and asserted empty at
//! the end — a page-script failure fails the test, not just the assertion
//! it happened to sit next to.
//!
//! ## Deliberately not ported (playwright-only capabilities)
//!
//! - Preview/mobile screenshots: visual artifacts, not assertable headless
//!   without extra dependencies (noted in the ported test too).
//! - Request interception (module/worker load-failure recovery), offline
//!   service-worker reload, worker heartbeat/cancel timing, crop-drag
//!   mouse choreography and multi-tab scenarios: they need network
//!   interception, service-worker control, wall-clock timing or raw input
//!   injection this minimal CDP client does not implement.
//! - The supplied-photo extras of the .cjs (family rules, 128×128 build,
//!   color caps, stage previews): property-adjacent UI coverage that needs
//!   interception or visual judgment; the pipeline itself stays covered by
//!   the native tests.
//!
//! Timeouts below are ceilings; a healthy run finishes in seconds.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Per-CDP-call timeout: navigation/loading can be slow, nothing else is.
const CDP_TIMEOUT: Duration = Duration::from_secs(30);
/// Local cap for one CDP discovery HTTP request (connect + headers + body).
const CDP_HTTP_TIMEOUT: Duration = Duration::from_secs(2);
/// Ceiling for waiting until a freshly spawned Chromium's CDP answers.
const CDP_LAUNCH_WAIT: Duration = Duration::from_secs(20);
/// Overall budget for one whole test interaction; every socket wait is
/// sliced from the deadline derived from it.
const TEST_TIMEOUT: Duration = Duration::from_secs(120);
/// Ceiling for the studio to become ready (wasm load + worker startup).
const READY_TIMEOUT: Duration = Duration::from_secs(30);
/// Ceiling for one studio build (worker conversion + presentation).
const BUILD_TIMEOUT: Duration = Duration::from_secs(60);
/// Ids for browser-level CDP commands (Target.*); page commands count from 1.
static BROWSER_ID: AtomicU64 = AtomicU64::new(1_000_000);

/// PATH probe for the harness gate: `/usr/bin/chromium` or any `chromium`
/// on PATH that exists and is executable. No spawn, no version check.
fn chromium_available() -> bool {
    let direct = Path::new("/usr/bin/chromium");
    if direct.is_file() {
        return is_executable(direct);
    }
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            let candidate = dir.join("chromium");
            candidate.is_file() && is_executable(&candidate)
        })
    })
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
}

/// Skip the test on a chromium-less host; eprintln (not panic) so the gate
/// can never turn a missing browser into a red run.
macro_rules! require_chromium {
    () => {
        if !chromium_available() {
            eprintln!("skipping: chromium not installed");
            return;
        }
    };
}

/// Monotonic-ish unique suffix for temp dir names.
fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
}

// ---------------------------------------------------------------------------
// CDP-over-TcpStream primitives — adapted from bot's tests/browser_js.rs
// (minimal, test-only form). Std-only on purpose: hand-rolled WebSocket
// framing, no ws or protocol crates.
// ---------------------------------------------------------------------------

/// How much of `deadline` is left, as a nonzero socket timeout.
fn ws_slice(deadline: Instant) -> Result<Duration, String> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err("test timed out".to_string());
    }
    Ok(left)
}

/// Recompute the remaining budget before every partial read/write, so the
/// socket timeout is always relative-to-now against the same absolute
/// deadline (never a stale timeout taken before a long partial transfer).
fn ws_read_exact(
    stream: &mut TcpStream,
    buf: &mut [u8],
    deadline: Instant,
    what: &str,
) -> Result<(), String> {
    let mut done = 0;
    while done < buf.len() {
        stream
            .set_read_timeout(Some(ws_slice(deadline)?))
            .map_err(|error| format!("read timeout setup: {error}"))?;
        let n = match stream.read(&mut buf[done..]) {
            Ok(n) => n,
            // A signal-interrupted syscall says nothing about the socket;
            // retry within the same budget.
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(ws_io_error(&error, deadline, what)),
        };
        if n == 0 {
            return Err(format!("ws {what}: connection closed by the peer"));
        }
        done += n;
    }
    ws_slice(deadline)?;
    Ok(())
}

/// Map an I/O error to the timeout sentinel (when the absolute budget is
/// spent) or a plain descriptive error.
fn ws_io_error(error: &std::io::Error, deadline: Instant, what: &str) -> String {
    if error.kind() == std::io::ErrorKind::WouldBlock
        || deadline.saturating_duration_since(Instant::now()).is_zero()
    {
        "test timed out".to_string()
    } else {
        format!("ws {what}: {error}")
    }
}

fn ws_write_all(
    stream: &mut TcpStream,
    buf: &[u8],
    deadline: Instant,
    what: &str,
) -> Result<(), String> {
    let mut done = 0;
    while done < buf.len() {
        stream
            .set_write_timeout(Some(ws_slice(deadline)?))
            .map_err(|error| format!("write timeout setup: {error}"))?;
        let n = match stream.write(&buf[done..]) {
            Ok(n) => n,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(ws_io_error(&error, deadline, what)),
        };
        if n == 0 {
            return Err(format!("ws {what}: write made no progress"));
        }
        done += n;
    }
    stream
        .flush()
        .map_err(|error| ws_io_error(&error, deadline, &format!("{what} flush")))?;
    ws_slice(deadline)?;
    Ok(())
}

/// HTTP-upgrade handshake against a `ws://host:port/path` URL; returns the
/// raw stream (frames are then read/written with [`ws_send`]/[`ws_recv`]).
fn ws_connect(ws_url: &str, deadline: Instant) -> Result<TcpStream, String> {
    let rest = ws_url
        .strip_prefix("ws://")
        .ok_or_else(|| format!("not a plain ws:// URL: {ws_url}"))?;
    let (authority, path) = rest.split_once('/').ok_or("ws URL without a path")?;
    let addr: SocketAddr = authority
        .parse()
        .map_err(|error| format!("ws URL must be host:port, got {authority:?}: {error}"))?;
    if !addr.ip().is_loopback() {
        return Err(format!("ws endpoint must be loopback, got {addr}"));
    }
    let mut stream = TcpStream::connect_timeout(&addr, ws_slice(deadline)?)
        .map_err(|error| format!("ws connect: {error}"))?;
    stream
        .set_nodelay(true)
        .map_err(|error| format!("nodelay: {error}"))?;
    let request = format!(
        "GET /{path} HTTP/1.1\r\nHost: {authority}\r\nUpgrade: websocket\r\n\
         Connection: Upgrade\r\nSec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\n\r\n",
        ws_handshake_key(),
    );
    ws_write_all(&mut stream, request.as_bytes(), deadline, "handshake write")?;
    // Read the handshake response byte-by-byte (NOT via BufReader: it would
    // buffer the first WS frames along with the headers, and those bytes
    // would be lost when the reader is dropped).
    let mut response = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        ws_read_exact(&mut stream, &mut byte, deadline, "handshake read")?;
        response.push(byte[0]);
        if response.ends_with(b"\r\n\r\n") {
            break;
        }
        if response.len() > 16 * 1024 {
            return Err("ws handshake response too large".to_string());
        }
    }
    let status = String::from_utf8_lossy(&response);
    let status = status.lines().next().unwrap_or_default();
    if !status.contains("101") {
        return Err(format!("ws handshake refused: {status}"));
    }
    Ok(stream)
}

/// Send one complete message as a masked text frame, bounded by `deadline`.
fn ws_send(stream: &mut TcpStream, payload: &[u8], deadline: Instant) -> Result<(), String> {
    ws_write_all(stream, &ws_frame(0x1, payload), deadline, "send")
}

/// Read one ws frame: 2-byte header, optional extended length, unmask.
fn ws_recv_frame(stream: &mut TcpStream, deadline: Instant) -> Result<(bool, u8, Vec<u8>), String> {
    let mut header = [0u8; 2];
    ws_read_exact(stream, &mut header, deadline, "recv")?;
    let fin = header[0] & 0x80 != 0;
    let opcode = header[0] & 0x0F;
    if header[0] & 0x70 != 0 {
        return Err(format!(
            "ws frame has reserved bits set (header {header:02x?})"
        ));
    }
    let mut length = u64::from(header[1] & 0x7F);
    match length {
        126 => {
            let mut extended = [0u8; 2];
            ws_read_exact(stream, &mut extended, deadline, "recv")?;
            length = u64::from(u16::from_be_bytes(extended));
        }
        127 => {
            let mut extended = [0u8; 8];
            ws_read_exact(stream, &mut extended, deadline, "recv")?;
            length = u64::from_be_bytes(extended);
        }
        _ => {}
    }
    if opcode >= 0x8 && (!fin || length > 125) {
        return Err(format!("ws control frame 0x{opcode:x} must be FIN+small"));
    }
    if length > 64 * 1024 * 1024 {
        return Err(format!("ws frame too large: {length} bytes"));
    }
    let masked = header[1] & 0x80 != 0;
    let mut mask = [0u8; 4];
    if masked {
        ws_read_exact(stream, &mut mask, deadline, "recv")?;
    }
    let mut payload = vec![0u8; usize::try_from(length).map_err(|_| "ws frame too large")?];
    ws_read_exact(stream, &mut payload, deadline, "recv")?;
    if masked {
        for (byte, key) in payload.iter_mut().zip(mask.iter().cycle()) {
            *byte ^= key;
        }
    }
    Ok((fin, opcode, payload))
}

/// Receive one complete text message, skipping control frames (answering
/// pings with pongs) and defragmenting continuations.
fn ws_recv(stream: &mut TcpStream, deadline: Instant) -> Result<Vec<u8>, String> {
    let mut payload: Vec<u8> = Vec::new();
    let mut in_progress = false;
    loop {
        let (fin, opcode, chunk) = ws_recv_frame(stream, deadline)?;
        match opcode {
            0x1 | 0x2 => {
                if in_progress {
                    return Err(format!(
                        "ws new data frame 0x{opcode:x} received mid-fragment"
                    ));
                }
                in_progress = true;
                payload = chunk;
            }
            0x0 => {
                if !in_progress {
                    return Err("ws continuation frame without a message in progress".to_string());
                }
                payload.extend(chunk);
            }
            0x8 => return Err("websocket closed by the browser".to_string()),
            0x9 => {
                // Ping: reply with a pong echoing the ping payload.
                ws_write_all(stream, &ws_frame(0xA, &chunk), deadline, "pong")?;
            }
            // 0xA = pong: control frames carry no CDP payload.
            0xA => {}
            other => return Err(format!("unexpected ws opcode {other}")),
        }
        if fin && opcode <= 0x2 {
            return Ok(payload);
        }
    }
}

/// Build one masked client frame for `opcode`.
fn ws_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let length = u64::try_from(payload.len()).unwrap_or(u64::MAX);
    let mut frame = vec![0x80 | opcode];
    match length {
        0..=125 => frame.push(0x80 | u8::try_from(length).unwrap_or(125)),
        126..=65_535 => {
            frame.push(0x80 | 0x7e);
            frame.extend_from_slice(&u16::try_from(length).unwrap_or(u16::MAX).to_be_bytes());
        }
        _ => {
            frame.push(0x80 | 0x7f);
            frame.extend_from_slice(&length.to_be_bytes());
        }
    }
    let mask = rand_mask();
    frame.extend_from_slice(&mask);
    frame.extend(
        payload
            .iter()
            .zip(mask.iter().cycle())
            .map(|(byte, key)| byte ^ key),
    );
    frame
}

/// A syntactically valid `Sec-WebSocket-Key`: base64 of 16 bytes (Chrome
/// rejects keys of other lengths outright). base64 of 16 zero-ish bytes is
/// still 24 valid chars — uniqueness is irrelevant to the handshake.
fn ws_handshake_key() -> String {
    // 16 bytes from the clock: base64-encoded below by hand (no base64
    // crate wanted here — tests carry no extra deps).
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&secs.to_be_bytes());
    bytes[8..12].copy_from_slice(&now.subsec_nanos().to_be_bytes());
    bytes[12..].copy_from_slice(&[0xb0, 0x0b, 0x5e, 0xed]);
    base64_std(&bytes)
}

/// RFC 4648 standard base64 (padding, `+/`), hand-rolled for this file (the
/// test photos and the handshake key both ride through it).
fn base64_std(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = u32::from_be_bytes([0, b[0], b[1], b[2]]);
        out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((n >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Four arbitrary mask bytes (masking is obfuscation, not security).
fn rand_mask() -> [u8; 4] {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let mixed = now.subsec_nanos() ^ u32::try_from(now.as_secs()).unwrap_or(0x1234_5678);
    let mut mask = mixed.to_be_bytes();
    // Never all-zero: some servers treat that as "unmasked" and error out.
    if mask == [0, 0, 0, 0] {
        mask = [0x12, 0x34, 0x56, 0x78];
    }
    mask
}

/// GET one loopback HTTP URL and parse the body as JSON (plain HTTP/1.1 —
/// CDP discovery only). Bounded: connect, read and the whole body.
fn http_json(url: &str, deadline: Instant) -> Result<Value, String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("not a plain http:// URL: {url}"))?;
    let (authority, path) = rest.split_once('/').ok_or("http URL without a path")?;
    let addr: SocketAddr = authority
        .parse()
        .map_err(|error| format!("http URL must be host:port, got {authority:?}: {error}"))?;
    if !addr.ip().is_loopback() {
        return Err(format!("http endpoint must be loopback, got {addr}"));
    }
    let budget = ws_slice(deadline)?.min(CDP_HTTP_TIMEOUT);
    let mut stream =
        TcpStream::connect_timeout(&addr, budget).map_err(|error| format!("connect: {error}"))?;
    stream
        .set_read_timeout(Some(budget))
        .map_err(|error| format!("read timeout: {error}"))?;
    let request = format!("GET /{path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("discovery write: {error}"))?;
    // Chromium's DevTools HTTP server answers promptly but does not close
    // the socket after `Connection: close`, so a drain-to-EOF would only
    // ever end in a read timeout: stop at Content-Length bytes instead.
    let mut raw = Vec::new();
    let read = stream
        .take(64 * 1024)
        .read_to_end(&mut raw)
        .map_err(|error| format!("discovery read: {error}"));
    // The read only ever ends at the Content-Length body (the DevTools
    // server keeps the socket open), so the timeout is the expected end:
    // accept it as long as something arrived, and validate the body below.
    match read {
        Ok(_) | Err(_) if !raw.is_empty() => {}
        Err(error) => return Err(error),
        Ok(_) => return Err("discovery read: empty response".to_string()),
    }
    let text = String::from_utf8_lossy(&raw);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or_default();
    serde_json::from_str(body.trim_start())
        .map_err(|error| format!("bad CDP discovery body: {error}"))
}

// ---------------------------------------------------------------------------
// CDP client: connect, attach to the first page tab, id-matched calls
// ---------------------------------------------------------------------------

/// A live CDP connection to the spawned Chromium's first page target.
/// One socket carries commands, replies and events. Fields drop in
/// declaration order: the socket closes before `_keepalive` SIGKILLs the
/// browser's process group (the browser is kept alive by its owner the
/// same way the source harness keeps its browser alive).
struct Cdp {
    stream: TcpStream,
    session: String,
    next_id: u64,
    /// Uncaught page exceptions and console errors, as reported by CDP
    /// events seen on this connection (recorded even mid-wait).
    errors: Vec<String>,
    /// The overall budget every wait on this connection is sliced from.
    deadline: Instant,
    _keepalive: Chromium,
}

impl Cdp {
    /// Spawn Chromium under `home`, fetch `/json/version` + `/json/list`
    /// over plain HTTP, connect the ws handshake to the browser endpoint,
    /// and attach (flattened) to the first page tab.
    fn connect(home: &Path, deadline: Instant) -> Result<Self, String> {
        let mut chromium = Chromium::spawn(home)?;
        chromium.wait_for_cdp(deadline)?;
        let port = chromium.port;
        let version = http_json(&format!("http://127.0.0.1:{port}/json/version"), deadline)?;
        let ws_url = version
            .get("webSocketDebuggerUrl")
            .and_then(Value::as_str)
            .ok_or("CDP endpoint has no WebSocket URL")?
            .to_string();
        let targets = http_json(&format!("http://127.0.0.1:{port}/json/list"), deadline)?;
        let target_id = targets
            .as_array()
            .and_then(|targets| targets.iter().find(|target| target["type"] == "page"))
            .and_then(|tab| tab.get("id").and_then(Value::as_str))
            .ok_or("CDP target list has no page tab")?
            .to_string();
        let mut stream = ws_connect(&ws_url, deadline)?;
        let id = BROWSER_ID.fetch_add(1, Ordering::Relaxed);
        send_command(
            &mut stream,
            id,
            "Target.attachToTarget",
            &json!({"targetId": target_id, "flatten": true}),
            None,
            deadline,
        )?;
        let session = wait_for_id(&mut stream, id, deadline, &mut Vec::new())?
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or("attachToTarget returned no sessionId")?
            .to_string();
        Ok(Self {
            stream,
            session,
            next_id: 0,
            errors: Vec::new(),
            deadline,
            _keepalive: chromium,
        })
    }

    /// Send one CDP command to the attached page session and wait for its
    /// reply, recording — not swallowing — page-error events seen on the
    /// way. Bounded by the connection's overall deadline and by
    /// [`CDP_TIMEOUT`] per call, whichever is shorter.
    fn cdp(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        send_command(
            &mut self.stream,
            id,
            method,
            params,
            Some(&self.session),
            self.deadline,
        )?;
        wait_for_id(&mut self.stream, id, self.deadline, &mut self.errors)
    }

    /// `Runtime.evaluate` with `returnByValue` (+ optional `awaitPromise`):
    /// the JS-friendly primitive every helper builds on. Returns the
    /// evaluation's value; a JS exception surfaces as a [`String`] error.
    fn evaluate(&mut self, expression: &str, await_promise: bool) -> Result<Value, String> {
        let value = self.cdp(
            "Runtime.evaluate",
            &json!({
                "expression": expression,
                "returnByValue": true,
                "awaitPromise": await_promise,
                "userGesture": true,
            }),
        )?;
        check_js_exception(&value)?;
        value
            .pointer("/result/value")
            .cloned()
            .ok_or_else(|| format!("evaluate returned no value: {expression}"))
    }

    /// Navigate the attached tab, wait (bounded) for the page's
    /// `Page.loadEventFired` and then for `readyState == complete`.
    /// One socket serves commands and events in order, and no other call
    /// runs between the navigation and the wait, so the event cannot be
    /// consumed elsewhere: whatever fired is still buffered in the socket.
    fn navigate(&mut self, url: &str) -> Result<(), String> {
        self.cdp("Runtime.enable", &json!({}))?;
        self.cdp("Page.enable", &json!({}))?;
        let value = self.cdp("Page.navigate", &json!({"url": url}))?;
        check_js_exception(&value)?;
        if let Some(error_text) = value.get("errorText").and_then(Value::as_str) {
            return Err(format!("navigation failed: {error_text}"));
        }
        wait_for_event(
            &mut self.stream,
            "Page.loadEventFired",
            self.deadline,
            &mut self.errors,
        )?;
        self.wait_ready_state("complete")
    }

    /// Poll `document.readyState` until it equals `state` (bounded).
    fn wait_ready_state(&mut self, state: &str) -> Result<(), String> {
        let deadline = self.deadline;
        loop {
            let ready = self.cdp(
                "Runtime.evaluate",
                &json!({"expression": "document.readyState", "returnByValue": true}),
            )?;
            if ready.pointer("/result/value").and_then(Value::as_str) == Some(state) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!("readyState never reached {state}"));
            }
            std::thread::sleep(
                Duration::from_millis(100).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }
}

/// Write one CDP command frame (browser-level when `session` is `None`).
fn send_command(
    stream: &mut TcpStream,
    id: u64,
    method: &str,
    params: &Value,
    session: Option<&str>,
    deadline: Instant,
) -> Result<(), String> {
    // Omit `sessionId` entirely for browser-level commands (`Target.*`):
    // the DevTools backend rejects a null sessionId field outright.
    let payload = match session {
        Some(session) => serde_json::to_vec(&json!({
            "id": id, "method": method, "params": params, "sessionId": session,
        })),
        None => serde_json::to_vec(&json!({"id": id, "method": method, "params": params})),
    }
    .map_err(|error| format!("serialize CDP command: {error}"))?;
    // Per-call cap: navigation/loading can be slow, but a single call never
    // waits longer than CDP_TIMEOUT.
    ws_send(stream, &payload, deadline.min(Instant::now() + CDP_TIMEOUT))
}

/// Read frames until the id-matched reply arrives. Events seen on the way
/// are recorded into `errors` when they are page errors, skipped otherwise.
fn wait_for_id(
    stream: &mut TcpStream,
    id: u64,
    deadline: Instant,
    errors: &mut Vec<String>,
) -> Result<Value, String> {
    loop {
        let value = next_frame(stream, deadline.min(Instant::now() + CDP_TIMEOUT))?;
        if value.get("id").and_then(Value::as_u64) == Some(id) {
            if let Some(error) = value.get("error") {
                return Err(format!("CDP call failed: {error}"));
            }
            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
        }
        record_page_error(&value, errors);
    }
}

/// Read frames until the named CDP event fires (used for
/// `Page.loadEventFired`); replies and other events are skipped / recorded.
fn wait_for_event(
    stream: &mut TcpStream,
    method: &str,
    deadline: Instant,
    errors: &mut Vec<String>,
) -> Result<Value, String> {
    loop {
        let value = next_frame(stream, deadline.min(Instant::now() + CDP_TIMEOUT))?;
        if value.get("method").and_then(Value::as_str) == Some(method) {
            return Ok(value);
        }
        record_page_error(&value, errors);
    }
}

/// One ws frame → parsed CDP message.
fn next_frame(stream: &mut TcpStream, deadline: Instant) -> Result<Value, String> {
    let frame = ws_recv(stream, deadline)?;
    serde_json::from_slice(&frame).map_err(|error| format!("bad CDP frame: {error}"))
}

/// Uncaught page exceptions (`Runtime.exceptionThrown`) and console errors
/// (`Runtime.consoleAPICalled`, type `error`) are page JS failures: record
/// them wherever they surface, so a test can assert zero of them.
fn record_page_error(value: &Value, errors: &mut Vec<String>) {
    if value.get("method").and_then(Value::as_str) == Some("Runtime.exceptionThrown") {
        let text = value
            .pointer("/params/exceptionDetails/exception/description")
            .or_else(|| value.pointer("/params/exceptionDetails/text"))
            .map_or_else(|| "unknown exception".to_string(), Value::to_string);
        errors.push(text);
    }
    if value.get("method").and_then(Value::as_str) == Some("Runtime.consoleAPICalled") {
        let is_error = value.pointer("/params/type").and_then(Value::as_str) == Some("error");
        let text = value
            .pointer("/params/args")
            .and_then(Value::as_array)
            .map(|args| {
                args.iter()
                    .map(|arg| {
                        arg.get("value")
                            .or_else(|| arg.get("description"))
                            .map(Value::to_string)
                            .unwrap_or_default()
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        if is_error {
            errors.push(format!("console.error: {text}"));
        }
    }
}

fn check_js_exception(value: &Value) -> Result<(), String> {
    if let Some(details) = value.get("exceptionDetails") {
        let description = details
            .pointer("/exception/description")
            .or_else(|| details.get("text"))
            .unwrap_or(details);
        return Err(format!("JS error: {description}"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Chromium lifecycle: own process group, guaranteed SIGKILL-of-the-group on
// drop — the studio itself is only ever tested through this browser.
// ---------------------------------------------------------------------------

/// A headless Chromium owned by the test: unique temp profile, loopback CDP
/// port, killed as a process group (SIGKILL) on drop, reaped before return.
struct Chromium {
    child: Option<Child>,
    port: u16,
    /// Keeps the unique profile dir alive to the last moment; removed here.
    profile: PathBuf,
}

impl Chromium {
    /// Spawn chromium with a profile in `home` and a CDP port reserved by
    /// bind-then-release. Deliberately no `--no-sandbox` fallback: if the
    /// host cannot support Chromium's sandbox, fail the launch.
    fn spawn(home: &Path) -> Result<Self, String> {
        let port = find_free_port()?;
        let profile = home.join("chromium-profile");
        std::fs::create_dir_all(&profile).map_err(|error| format!("profile dir: {error}"))?;
        let child = Command::new("chromium")
            .args([
                "--headless=new",
                "--disable-gpu",
                "--disable-dev-shm-usage",
                "--disable-extensions",
                "--disable-background-networking",
                "--no-first-run",
                "--no-default-browser-check",
                "--mute-audio",
                &format!("--user-data-dir={}", profile.display()),
                &format!("--remote-debugging-port={port}"),
                "about:blank",
            ])
            .env("HOME", home.display().to_string())
            .stdin(Stdio::null())
            // Chromium logs an unwritable-ProfileDetector warning or two
            // headlessly; captured for debugging, never parsed.
            .stdout(
                std::fs::File::create(home.join("chromium.log"))
                    .map_err(|e| format!("log: {e}"))?,
            )
            .stderr(
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(home.join("chromium.log"))
                    .map_err(|e| format!("log open: {e}"))?,
            )
            .process_group(0)
            .spawn()
            .map_err(|error| format!("failed to spawn chromium: {error}"))?;
        Ok(Self {
            child: Some(child),
            port,
            profile,
        })
    }

    /// Block until the CDP endpoint answers `/json/version`, bounded by
    /// the overall deadline and [`CDP_LAUNCH_WAIT`], whichever is sooner.
    fn wait_for_cdp(&mut self, overall: Instant) -> Result<(), String> {
        let deadline = overall.min(Instant::now() + CDP_LAUNCH_WAIT);
        let mut last = "browser did not come up".to_string();
        while Instant::now() < deadline {
            match http_json(
                &format!("http://127.0.0.1:{}/json/version", self.port),
                deadline,
            ) {
                Ok(version) if version.get("webSocketDebuggerUrl").is_some() => return Ok(()),
                Ok(_) => last = "CDP endpoint has no WebSocket URL".to_string(),
                Err(error) => last = error,
            }
            if let Some(child) = self.child.as_mut() {
                if let Ok(Some(status)) = child.try_wait() {
                    return Err(format!(
                        "chromium exited early: {status} (see chromium.log)"
                    ));
                }
            }
            std::thread::sleep(
                Duration::from_millis(250).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
        Err(format!("chromium CDP not reachable: {last}"))
    }

    /// SIGKILL the whole process group and reap. `kill(2)` with a pid we
    /// own and have not reaped, so the pid cannot have been reused (SAFETY
    /// for the raw syscall).
    fn kill(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let pid = child.id().cast_signed();
        // SAFETY: the child was spawned as leader of its own process group
        // (process_group(0)) and is not reaped yet, so -pid is its group.
        unsafe { kill_process_group(pid) };
        let _ = child.kill();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if child.try_wait().is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// `kill(-pid, SIGKILL)` through the libc symbol; no libc crate here, so
/// the extern is declared by hand (tests carry no extra dependencies).
unsafe fn kill_process_group(pid: i32) -> i32 {
    const SIGKILL: i32 = 9;
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    unsafe { kill(-pid, SIGKILL) }
}

impl Drop for Chromium {
    fn drop(&mut self) {
        self.kill();
        let _ = std::fs::remove_dir_all(&self.profile);
    }
}

/// Reserve a loopback port by binding and releasing; a small race window
/// remains, acceptable on a private dev box, and both consumers retry
/// implicitly via `wait_for_cdp`/health polling.
fn find_free_port() -> Result<u16, String> {
    TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .map_err(|error| format!("no free port: {error}"))
}

// ---------------------------------------------------------------------------
// The real server binary: CARGO_BIN_EXE_lego-mosaic on a free loopback
// port, /healthz polled, killed on drop. The wasm assets are committed and
// embedded in the binary, so no separate wasm build is needed.
// ---------------------------------------------------------------------------

/// The spawned server binary plus its fixture directory. Dropping it kills
/// the child and removes the directory.
struct Server {
    child: Option<Child>,
    port: u16,
    dir: PathBuf,
    base: String,
}

impl Server {
    /// Spawn `target/release/lego-mosaic` (provided automatically for the
    /// integration test via `CARGO_BIN_EXE_lego-mosaic`) on a loopback port
    /// with `LEGO_MOSAIC_ADDR`. Clean-room environment: the static host
    /// reads nothing else from the environment, so nothing else is passed.
    fn spawn(dir: &Path) -> Self {
        let binary = PathBuf::from(env!("CARGO_BIN_EXE_lego-mosaic"));
        std::fs::create_dir_all(dir).expect("server fixture dir");
        let port = find_free_port().expect("reserve server port");
        let log = std::fs::File::create(dir.join("server.log")).expect("server log");
        let stderr = log.try_clone().expect("clone log handle");
        let addr = format!("127.0.0.1:{port}");
        let child = Command::new(&binary)
            .env_clear()
            .env("LEGO_MOSAIC_ADDR", &addr)
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(stderr)
            .spawn()
            .expect("spawn lego-mosaic server binary");
        let server = Self {
            child: Some(child),
            port,
            dir: dir.to_path_buf(),
            base: format!("http://{addr}"),
        };
        wait_for_health(server.port);
        server
    }
}

/// Poll `/healthz` until the server answers 200 (bounded; a wedged startup
/// must not hang the suite).
fn wait_for_health(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) {
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("read timeout");
            if stream
                .write_all(b"GET /healthz HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
                .is_ok()
            {
                let mut response = Vec::new();
                let _ = stream.read_to_end(&mut response);
                if String::from_utf8_lossy(&response).contains("200") {
                    return;
                }
            }
        }
        assert!(Instant::now() < deadline, "server never became healthy");
        std::thread::sleep(Duration::from_millis(100));
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// ---------------------------------------------------------------------------
// Harness: (server, chromium, cdp) wired together + JS-friendly helpers.
// Drop order is struct-field order: cdp first (plain socket close, then the
// browser's SIGKILL inside its keepalive), then the server, then the
// fixture root that outlives both.
// ---------------------------------------------------------------------------

struct Harness {
    /// Drops LAST (struct fields drop in declaration order): removes the
    /// whole fixture root after the browser and the server are gone.
    /// Kept as an (unread) field purely for its `Drop` side effect, like
    /// `_keepalive` on the Cdp connection.
    #[expect(dead_code)]
    cleanup: RootCleanup,
    cdp: Cdp,
    server: Server,
    root: PathBuf,
}

/// The fixture root's last-drop owner: after `cdp` (browser SIGKILL) and
/// `server` (child kill) have dropped, this removes the root directory so
/// a finished run leaves zero fixture bytes behind.
struct RootCleanup(PathBuf);

impl Drop for RootCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Harness {
    /// Spawn the real server and a real headless Chromium; connect CDP to
    /// the browser's first tab (which the connection then keeps alive).
    fn new(tag: &str) -> Self {
        let deadline = Instant::now() + TEST_TIMEOUT;
        let root = std::env::temp_dir().join(format!(
            "lego-mosaic-browser-smoke-{}-{tag}-{}",
            std::process::id(),
            now_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("fixture root");
        let server = Server::spawn(&root.join("server"));
        let cdp =
            Cdp::connect(&root.join("browser"), deadline).expect("spawn chromium and connect CDP");
        Self {
            cleanup: RootCleanup(root.clone()),
            cdp,
            server,
            root,
        }
    }

    /// Navigate the browser's tab to `path` on the spawned server and wait
    /// for the load event (bounded by the harness deadline). A tiny capture
    /// shim records every request the page issues, for the local-only
    /// assertions the .cjs made from its playwright request log.
    fn goto(&mut self, path: &str) {
        let url = format!("{}{path}", self.server.base);
        self.cdp.navigate(&url).expect("navigate");
        self.eval(
            "(() => { window.__smokeRequests = []; \
             const original = window.fetch; \
             window.fetch = function (input, init) { \
               const url = typeof input === 'string' ? input : input.url; \
               const method = (init && init.method) || \
                 (input instanceof Request ? input.method : 'GET'); \
               window.__smokeRequests.push({ method: method, url: String(url) }); \
               return original.apply(this, arguments); }; \
             return true; })()",
        )
        .expect("install request capture shim");
    }

    /// Evaluate one JS expression and return its value (`returnByValue`).
    /// JS exceptions become `Err`.
    fn eval(&mut self, expression: &str) -> Result<Value, String> {
        self.cdp.evaluate(expression, false)
    }

    /// Same, for promise-returning expressions (`awaitPromise: true`).
    fn eval_async(&mut self, expression: &str) -> Result<Value, String> {
        self.cdp.evaluate(expression, true)
    }

    /// Poll a promise-returning expression until it settles and return its
    /// text value. A not-yet-settled promise shows up as a plain error (or
    /// a missing value) on this minimal client, which the loop simply
    /// retries within `timeout`.
    fn await_text(&mut self, expression: &str, timeout: Duration, what: &str) -> String {
        let deadline = Instant::now() + timeout;
        let mut last;
        loop {
            match self.eval_async(expression) {
                Ok(value) => {
                    if let Some(text) = value.as_str() {
                        return text.to_string();
                    }
                    last = format!("non-string value: {value}");
                }
                Err(error) => last = error,
            }
            assert!(
                Instant::now() < deadline,
                "{what}: promise never settled (last: {last})"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Page errors recorded so far (uncaught exceptions + console.error).
    fn assert_no_page_errors(&self) {
        assert!(
            self.cdp.errors.is_empty(),
            "page JS errors: {:?}",
            self.cdp.errors
        );
    }

    /// Poll `probe` (a JS expression producing a truthy value) until it is
    /// truthy or the timeout passes; panics with `what` otherwise.
    fn wait_for(&mut self, probe: &str, timeout: Duration, what: &str) {
        let deadline = Instant::now() + timeout;
        loop {
            let truthy = self
                .eval(probe)
                .is_ok_and(|value| value.as_bool().unwrap_or(!value.is_null()));
            if truthy {
                return;
            }
            assert!(Instant::now() < deadline, "{what}: condition never held");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Click via `document.querySelector(selector).click()` through
    /// `Runtime.evaluate` (simpler than dispatching CDP input coords).
    fn click(&mut self, selector: &str) {
        let value = js_string(selector);
        self.eval(&format!(
            "(() => {{ const el = document.querySelector({value}); \
             if (!el) throw new Error('no element matches'); el.click(); return true; }})()"
        ))
        .unwrap_or_else(|error| panic!("click {selector}: {error}"));
    }

    /// Set an input/select value and fire `input` + `change` (the studio
    /// listens on both), mirroring the .cjs `set` helper.
    fn set(&mut self, id: &str, value: &str) {
        let literal = js_string(value);
        self.eval(&format!(
            "(() => {{ const el = document.querySelector('#{id}'); \
             el.value = {literal}; \
             el.dispatchEvent(new Event('input', {{ bubbles: true }})); \
             el.dispatchEvent(new Event('change', {{ bubbles: true }})); \
             return true; }})()"
        ))
        .unwrap_or_else(|error| panic!("set #{id} = {value}: {error}"));
    }

    /// Uncheck one exclusion swatch by palette index and assert it stuck.
    fn uncheck_swatch(&mut self, index: usize) {
        self.eval(&format!(
            "(() => {{ const box = document.querySelector( \
               '#exclusions input[data-color=\"{index}\"]'); \
             box.checked = false; \
             box.dispatchEvent(new Event('change', {{ bubbles: true }})); \
             return box.checked; }})()"
        ))
        .unwrap_or_else(|error| panic!("uncheck swatch {index}: {error}"));
    }

    /// The .cjs `ready()` gate: the settings fieldset unlocks once the wasm
    /// module booted, the palette filled and the worker started.
    fn wait_ready(&mut self) {
        self.wait_for(
            "!document.querySelector('#settings').disabled",
            READY_TIMEOUT,
            "studio never became ready",
        );
    }

    /// One full build: click Build, wait for the busy gate to lift, assert
    /// no error box, and wait for the mosaic `<img>` to carry real pixels.
    fn build(&mut self) {
        self.click("#go");
        self.wait_ready();
        let error = self.text_of("#error");
        let shown = self
            .eval("!document.querySelector('#error').hidden")
            .ok()
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        assert!(!shown, "unexpected build error: {error}");
        self.wait_for(
            "(() => { const i = document.querySelector('#mosaic-image'); \
             return !i.hidden && i.complete && i.naturalWidth > 0; })()",
            BUILD_TIMEOUT,
            "mosaic image never rendered",
        );
    }

    /// `textContent` of one element, empty when the element is absent.
    fn text_of(&mut self, selector: &str) -> String {
        let literal = js_string(selector);
        self.eval(&format!(
            "document.querySelector({literal})?.textContent ?? ''"
        ))
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
    }

    /// The flat mosaic SVG text, fetched from the preview's blob URL — the
    /// exact bytes the Mosaic SVG download and the CLI produce.
    fn mosaic_svg(&mut self) -> String {
        self.await_text(
            "(async () => await (await fetch( \
               document.querySelector('#mosaic-image').src)).text())()",
            BUILD_TIMEOUT,
            "fetch mosaic svg",
        )
    }

    /// Click a download button and return the downloaded file's text. The
    /// studio's `download` creates a throwaway `<a download>` pointing at a
    /// blob URL, clicks it and revokes the URL one browser turn later —
    /// this helper intercepts the anchor's `click` to capture the suggested
    /// filename AND fetch the blob bytes in the same tick, so no
    /// playwright `Download` event plumbing is needed (the bytes are
    /// exactly what the browser would have saved).
    fn download_text(&mut self, button: &str, name: &str) -> String {
        let expected = js_string(name);
        let text = self.await_text(
            &format!(
                "(async () => {{ let saved = null, text = null; \
                 const original = HTMLElement.prototype.click; \
                 HTMLElement.prototype.click = function () {{ \
                   if (this.download === undefined) {{ return original.apply(this, arguments); }} \
                   saved = this.download; \
                   fetch(this.href).then(r => r.text()).then(t => {{ text = t; }}); \
                 }}; \
                 try {{ document.querySelector('#{button}').click(); }} \
                 finally {{ HTMLElement.prototype.click = original; }} \
                 for (let i = 0; i < 200 && (saved === null || text === null); i++) \
                   await new Promise(r => setTimeout(r, 25)); \
                 if (saved === null) throw new Error('no download started'); \
                 if (saved !== {expected}) throw new Error('wrong name ' + saved); \
                 if (text === null) throw new Error('download never resolved'); \
                 if (!text) throw new Error('download empty'); \
                 return text; }})()"
            ),
            Duration::from_secs(10),
            &format!("download {name}"),
        );
        assert!(!text.is_empty(), "download {name} produced no bytes");
        text
    }
}

/// A JS single-quoted string literal (the harness has no JSON bridge for
/// interpolating values into expressions).
fn js_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for c in value.chars() {
        match c {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// True when a CSV line starting with `prefix` carries the requested
/// quantity: `"<id>","<symbol>",…,"<count>","98138",…` — prefix plus the
/// quantity in the Quantity column (the .cjs regex `"black","06".*"32"`
/// also matches the Part/Availability columns; anchoring the count in its
/// column is the same property, stated more precisely).
fn csv_line_has_quantity(csv: &str, prefix: &str, quantity: &str) -> bool {
    let needle = format!(",\"{quantity}\",");
    csv.lines()
        .any(|line| line.starts_with(prefix) && line[prefix.len()..].contains(&needle))
}

/// Run the same release binary with `convert` args and require the output
/// file to exist (the CLI half of every parity assertion).
fn run_convert(args: &[String], output: &Path, what: &str) {
    let outcome = Command::new(env!("CARGO_BIN_EXE_lego-mosaic"))
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("{what}: {error}"));
    assert!(
        outcome.status.success(),
        "{what} failed: {}{}",
        String::from_utf8_lossy(&outcome.stdout),
        String::from_utf8_lossy(&outcome.stderr),
    );
    assert!(output.exists(), "{what} wrote no output file");
}

/// Hand the page a real PNG through the `#image` file input and fire the
/// change event. The bytes ride in as a `data:` URL fetched into a `File`
/// (a `DataTransfer` cannot reference an existing path), which reaches the
/// studio through exactly the same `FileList` assignment the .cjs
/// `setInputFiles` and the canvas upload used.
fn upload_png(harness: &mut Harness, bytes: &[u8], filename: &str) {
    let encoded = base64_std(bytes);
    let name = js_string(filename);
    harness
        .eval_async(&format!(
            "(async () => {{ const response = await fetch( \
               'data:image/png;base64,{encoded}'); \
             const blob = await response.blob(); \
             const d = new DataTransfer(); \
             d.items.add(new File([blob], {name}, {{ type: 'image/png' }})); \
             document.querySelector('#image').files = d.files; \
             document.querySelector('#image').dispatchEvent( \
               new Event('change', {{ bubbles: true }})); return true; }})()"
        ))
        .unwrap_or_else(|error| panic!("upload {filename}: {error}"));
    harness.wait_for(
        "!document.querySelector('#original').hidden",
        READY_TIMEOUT,
        "uploaded image never decoded",
    );
}

/// Upload a PNG and wait for that exact image to finish decoding. A changed
/// natural size keeps repeated-selection tests from accepting the prior
/// preview while the replacement is still decoding.
fn upload_png_and_wait(
    harness: &mut Harness,
    bytes: &[u8],
    filename: &str,
    width: usize,
    height: usize,
) {
    let encoded = base64_std(bytes);
    let name = js_string(filename);
    harness
        .eval_async(&format!(
            "(async () => {{ const response = await fetch( \
               'data:image/png;base64,{encoded}'); \
             const blob = await response.blob(); \
             const d = new DataTransfer(); \
             d.items.add(new File([blob], {name}, {{ type: 'image/png' }})); \
             document.querySelector('#image').files = d.files; \
             document.querySelector('#image').dispatchEvent( \
               new Event('change', {{ bubbles: true }})); return true; }})()"
        ))
        .unwrap_or_else(|error| panic!("upload {filename}: {error}"));
    harness.wait_for(
        &format!(
            "(() => {{ const i = document.querySelector('#original'); \
             return i.complete && i.naturalWidth === {width} && \
               i.naturalHeight === {height} && \
               document.querySelector('#source-size').textContent.includes( \
                 '{width} × {height}'); }})()"
        ),
        READY_TIMEOUT,
        &format!("uploaded image {filename} never loaded"),
    );
    let src = harness
        .eval("document.querySelector('#original').src")
        .expect("read original source")
        .as_str()
        .expect("original source is a string")
        .to_string();
    assert!(
        src.starts_with("data:image/png"),
        "the original preview must retain its data URL: {src}"
    );
    assert!(
        !src.starts_with("blob:"),
        "the original is not a Blob URL: {src}"
    );
}

/// A small deterministic PNG for browser-selection lifecycle tests.
fn solid_png(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(width, height, image::Rgba(color));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("encode browser fixture PNG");
    bytes.into_inner()
}

/// Geometry of the live original-wrap and crop frame, in CSS pixels.
fn crop_geometry(harness: &mut Harness) -> [f64; 4] {
    let value = harness
        .eval(
            "(() => { const w = document.querySelector('#crop-wrap').getBoundingClientRect(); \
             const f = document.querySelector('#crop-frame').getBoundingClientRect(); \
             return [w.width, w.height, f.width, f.height]; })()",
        )
        .expect("read crop geometry");
    let values = value.as_array().expect("crop geometry is an array");
    assert_eq!(values.len(), 4, "crop geometry: {value}");
    [
        values[0].as_f64().expect("wrap width"),
        values[1].as_f64().expect("wrap height"),
        values[2].as_f64().expect("frame width"),
        values[3].as_f64().expect("frame height"),
    ]
}

/// The symbol text of the first cell matched by `selector`.
fn row_symbol(harness: &mut Harness, selector: &str) -> String {
    let literal = js_string(selector);
    let value = harness
        .eval(&format!(
            "document.querySelector({literal})?.textContent ?? null"
        ))
        .unwrap_or_else(|error| panic!("read row symbol {selector}: {error}"));
    value.as_str().map_or_else(
        || panic!("row symbol {selector}: got {value}"),
        str::to_string,
    )
}

// ---------------------------------------------------------------------------
// Test 1 — the browser_smoke.cjs property set.
// ---------------------------------------------------------------------------

#[test]
fn browser_smoke_studio_end_to_end() {
    require_chromium!();
    let mut harness = Harness::new("studio");
    harness.goto("/");
    step_ready_swatches(&mut harness);
    step_canvas_upload_and_well(&mut harness);
    step_build_8x8_and_rows(&mut harness);
    step_exports(&mut harness);
    step_exclusions_and_guard(&mut harness);
    step_cli_parity(&mut harness);
    step_local_only_requests(&mut harness);
    harness.assert_no_page_errors();
}

/// The wasm module boots: settings unlock, and the mosaic-maker swatches
/// show Yellow and never a Brown (that one exists only in the extended
/// palette).
fn step_ready_swatches(harness: &mut Harness) {
    harness.wait_ready();
    let swatches = harness.text_of("#exclusions");
    assert!(swatches.contains("Yellow"), "swatches: {swatches}");
    assert!(!swatches.contains("Brown"), "swatches: {swatches}");
}

/// Upload path: draw black/white halves on a canvas, canvas → PNG blob →
/// `File` → `#image`, then fire the change event (exactly the .cjs flow).
/// The original preview must render inside the Original well: the crop
/// wrap is absolutely positioned, so an unanchored well would drop the
/// picture at the page's top-left area instead (regression guard).
fn step_canvas_upload_and_well(harness: &mut Harness) {
    harness
        .eval_async(
            "(async () => { const c = document.createElement('canvas'); \
             c.width = 64; c.height = 64; const x = c.getContext('2d'); \
             x.fillStyle = 'white'; x.fillRect(0, 0, 64, 64); \
             x.fillStyle = 'black'; x.fillRect(0, 0, 64, 32); \
             const blob = await new Promise(r => c.toBlob(r)); \
             const d = new DataTransfer(); \
             d.items.add(new File([blob], 'test.png', { type: 'image/png' })); \
             document.querySelector('#image').files = d.files; \
             document.querySelector('#image').dispatchEvent( \
               new Event('change', { bubbles: true })); return true; })()",
        )
        .expect("upload canvas png");
    harness.wait_ready();
    harness.wait_for(
        "(() => { const w = document.querySelector('#original-well') \
           .getBoundingClientRect(); \
         const i = document.querySelector('#original').getBoundingClientRect(); \
         return !document.querySelector('#original').hidden && i.width > 0 && \
           i.left >= w.left - 0.5 && i.right <= w.right + 0.5 && \
           i.top >= w.top - 0.5 && i.bottom <= w.bottom + 0.5; })()",
        READY_TIMEOUT,
        "original preview never settled inside the well",
    );
}

/// 8×8 build: 64 instruction cells and stable symbols — row 1 of the
/// top-down guide is the black half (`06`), the last row the white half
/// (`01`) — and the row_order flip re-anchors the guide (bottom-up starts
/// at `01`).
fn step_build_8x8_and_rows(harness: &mut Harness) {
    harness.set("width", "8");
    harness.set("height", "8");
    harness.build();
    let cells = harness
        .eval("document.querySelectorAll('#rows td').length")
        .expect("count instruction cells")
        .as_u64()
        .expect("cell count is a number");
    assert_eq!(cells, 64, "8×8 build must render 64 instruction cells");
    let first = row_symbol(harness, "#rows tbody tr td span");
    let last = row_symbol(harness, "#rows tbody tr:last-child td span");
    assert_eq!(first, "06", "top-down guide row 1 must be the black half");
    assert_eq!(last, "01", "top-down guide last row must be the white half");
    harness.set("row_order", "bottom");
    let flipped = row_symbol(harness, "#rows tbody tr td span");
    assert_eq!(
        flipped, "01",
        "bottom-up guide must start at the white base row"
    );
    harness.set("row_order", "top");
}

/// Exports: CSV download (suggested name `mosaic-parts.csv`, exact
/// `"black","06"` / `"white","01"` rows at quantity 32), guide SVG and the
/// flat mosaic SVG.
fn step_exports(harness: &mut Harness) {
    let csv = harness.download_text("export-csv", "mosaic-parts.csv");
    assert!(
        csv_line_has_quantity(&csv, "\"black\",\"06\"", "32"),
        "csv must carry the black half at 32 tiles: {csv}"
    );
    assert!(
        csv_line_has_quantity(&csv, "\"white\",\"01\"", "32"),
        "csv must carry the white half at 32 tiles: {csv}"
    );
    let guide = harness.download_text("export-guide", "mosaic-guide.svg");
    assert!(guide.contains("<svg"), "guide must be an SVG");
    assert!(guide.contains(">06<"), "guide must carry symbol 06 cells");
    assert!(guide.contains("Black"), "guide must carry the color name");
    let mosaic = harness.download_text("export-svg", "mosaic.svg");
    assert!(mosaic.contains("<rect"), "mosaic svg must be flat");
}

/// Exclusions: uncheck White, rebuild — the CSV drops white, black keeps
/// its symbol; the setting survives palette switches (per-palette
/// persistence keyed by the palette id). Unchecking every color is refused
/// with the "at least one" guard; Reset clears the state.
fn step_exclusions_and_guard(harness: &mut Harness) {
    harness.uncheck_swatch(0);
    harness.set("palette", "extended");
    harness.set("palette", "mosaic-maker");
    let checked = harness
        .eval("document.querySelector('#exclusions input[data-color=\"0\"]').checked")
        .expect("read white checkbox")
        .as_bool()
        .expect("checkbox state is boolean");
    assert!(!checked, "exclusion must survive palette switches");
    harness.build();
    let csv = harness.download_text("export-csv", "mosaic-parts.csv");
    assert!(
        !csv.contains("\"white\""),
        "csv must not contain excluded white: {csv}"
    );
    assert!(
        csv.contains("\"black\",\"06\""),
        "csv must still carry black: {csv}"
    );
    for index in 1..=3 {
        harness.uncheck_swatch(index);
    }
    harness.click("#exclusions input[data-color=\"4\"]");
    let checked = harness
        .eval("document.querySelectorAll('#exclusions input:checked').length")
        .expect("count checked swatches")
        .as_u64()
        .expect("count is a number");
    assert_eq!(checked, 1, "the guard must keep exactly one color enabled");
    let error = harness.text_of("#error");
    assert!(
        error.contains("at least one"),
        "all-excluded must warn: {error}"
    );
    harness.click("#reset");
}

/// Byte-identical parity: rebuild the (reset, default-settings) 48×48
/// studio SVG and compare with the same binary's `convert` output for the
/// same input image — the canvas drew rgb(0,0,0) / rgb(255,255,255)
/// halves, so the CLI gets the identical pixels from a PNG written with
/// the `image` crate. UI defaults and CLI defaults must agree on size,
/// palette, fit and padding for the bytes to line up.
fn step_cli_parity(harness: &mut Harness) {
    harness.build();
    let studio_svg = harness.mosaic_svg();
    let source = harness.root.join("halves.png");
    let image = image::RgbaImage::from_fn(64, 64, |_x, y| {
        if y < 32 {
            image::Rgba([0, 0, 0, 255]) // canvas 'black' top half
        } else {
            image::Rgba([255, 255, 255, 255]) // canvas 'white' bottom half
        }
    });
    image.save(&source).expect("write halves png");
    let cli_output = harness.root.join("cli-mosaic.svg");
    run_convert(
        &[
            "convert".to_string(),
            source.display().to_string(),
            "--output".to_string(),
            cli_output.display().to_string(),
        ],
        &cli_output,
        "cli convert for parity",
    );
    let cli_svg = std::fs::read_to_string(&cli_output).expect("read cli svg");
    assert_eq!(
        cli_svg, studio_svg,
        "CLI and browser must produce byte-identical SVG"
    );
}

/// Local-only guarantee: the studio must only ever issue GET requests on
/// this page (no uploads, no third-party calls — the .cjs request log,
/// captured via the fetch shim `Harness::goto` installs).
fn step_local_only_requests(harness: &mut Harness) {
    let non_get = harness
        .eval("window.__smokeRequests.filter(r => r.method !== 'GET').length")
        .expect("read request log")
        .as_u64()
        .expect("count is a number");
    assert_eq!(non_get, 0, "the studio must only send GET requests");
    let off_origin = harness
        .eval(
            "window.__smokeRequests.filter( \
               r => /^https?:/.test(r.url) && !r.url.startsWith(location.origin)).length",
        )
        .expect("read request log")
        .as_u64()
        .expect("count is a number");
    assert_eq!(off_origin, 0, "the studio must stay on its own origin");
}

#[test]
fn repeated_image_selection_refreshes_source_and_crop_overlay() {
    require_chromium!();
    let mut harness = Harness::new("replacement");
    harness.goto("/");
    harness.set("width", "16");
    harness.set("height", "8");
    harness.set("fit", "crop");

    let first = solid_png(64, 32, [220, 30, 30, 255]);
    upload_png_and_wait(&mut harness, &first, "first.png", 64, 32);
    let first_geometry = crop_geometry(&mut harness);
    let first_wrap_ratio = first_geometry[0] / first_geometry[1];
    let first_frame_ratio = first_geometry[2] / first_geometry[3];
    assert!(
        (first_wrap_ratio - 2.0).abs() < 0.1,
        "first source layout must use the 2:1 raster: {first_geometry:?}"
    );
    assert!(
        (first_frame_ratio - 2.0).abs() < 0.1,
        "first crop frame must follow the 2:1 raster: {first_geometry:?}"
    );
    harness.build();
    let first_mosaic = harness.mosaic_svg();
    let first_mosaic_url = harness
        .eval("document.querySelector('#mosaic-image').src")
        .expect("read first mosaic source")
        .as_str()
        .expect("first mosaic source is a string")
        .to_string();
    assert!(
        first_mosaic_url.starts_with("blob:"),
        "the first mosaic must retain a Blob URL: {first_mosaic_url}"
    );

    let second = solid_png(32, 64, [30, 30, 220, 255]);
    upload_png_and_wait(&mut harness, &second, "second.png", 32, 64);
    let second_geometry = crop_geometry(&mut harness);
    let second_wrap_ratio = second_geometry[0] / second_geometry[1];
    let second_frame_ratio = second_geometry[2] / second_geometry[3];
    assert!(
        (second_wrap_ratio - 0.5).abs() < 0.1,
        "replacement source layout must use the 1:2 raster: {second_geometry:?}"
    );
    assert!(
        (second_frame_ratio - 2.0).abs() < 0.1,
        "replacement crop frame must retain the 2:1 grid aspect: {second_geometry:?}"
    );
    assert_ne!(
        second_geometry, first_geometry,
        "replacement must refresh crop geometry after image load"
    );
    let old_mosaic_url_gone = harness
        .eval_async(&format!(
            "fetch({}).then(() => false, () => true)",
            js_string(&first_mosaic_url)
        ))
        .expect("probe revoked first mosaic URL")
        .as_bool()
        .expect("revocation probe is boolean");
    assert!(
        old_mosaic_url_gone,
        "replacing the source must revoke the previous mosaic Blob URL"
    );
    harness.build();
    let second_mosaic = harness.mosaic_svg();
    assert_ne!(
        second_mosaic, first_mosaic,
        "the retained raster must rebuild from the replacement image"
    );
    harness.assert_no_page_errors();
}

// ---------------------------------------------------------------------------
// Test 2 — the photo_smoke.cjs property: real-photo CLI/browser parity
// across all three fits (preset photo, extended palette, 64×64). The .cjs
// took photo paths from argv (no photos are committed); this port
// generates a deterministic 97×61 gradient photo in the test instead.
// ---------------------------------------------------------------------------

#[test]
fn photo_parity_across_fits_matches_cli_bytes() {
    require_chromium!();
    let mut harness = Harness::new("photo");
    harness.goto("/");
    harness.wait_ready();

    // Deterministic "photo": non-square, smooth gradients plus a diagonal
    // structure — enough for every fit to produce a distinct build.
    let source = harness.root.join("gradient.png");
    let photo = image::RgbaImage::from_fn(97, 61, |x, y| {
        let r = u8::try_from(x * 255 / 96).unwrap_or(u8::MAX);
        let g = u8::try_from(y * 255 / 60).unwrap_or(u8::MAX);
        let b = u8::try_from((x * 37 + y * 61) % 256).unwrap_or(u8::MAX);
        image::Rgba([r, g, b, 255])
    });
    photo.save(&source).expect("write gradient png");
    let mut bytes = Vec::new();
    std::fs::File::open(&source)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .expect("read gradient png");
    upload_png(&mut harness, &bytes, "gradient.png");

    harness.set("preset", "photo");
    harness.set("palette", "extended");
    harness.set("width", "64");
    harness.set("height", "64");

    for fit in ["contain", "crop", "stretch"] {
        harness.set("fit", fit);
        harness.build();
        let studio_svg = harness.mosaic_svg();
        assert!(
            studio_svg.contains("<rect"),
            "{fit}: mosaic svg must be flat"
        );
        let cli_output = harness.root.join(format!("photo-{fit}.svg"));
        run_convert(
            &[
                "convert".to_string(),
                source.display().to_string(),
                "--output".to_string(),
                cli_output.display().to_string(),
                "--preset".to_string(),
                "photo".to_string(),
                "--palette".to_string(),
                "extended".to_string(),
                "--size".to_string(),
                "64".to_string(),
                "--fit".to_string(),
                fit.to_string(),
            ],
            &cli_output,
            &format!("cli convert {fit}"),
        );
        let cli_svg = std::fs::read_to_string(&cli_output).expect("read cli svg");
        assert_eq!(
            cli_svg, studio_svg,
            "{fit}: CLI and browser must produce byte-identical SVG"
        );
    }
    harness.assert_no_page_errors();
}
