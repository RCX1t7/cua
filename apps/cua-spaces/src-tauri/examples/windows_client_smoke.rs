// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Opt-in Windows E2E for the same AppCore used by the Tauri commands.
//! Start an official Windows spacesd in the interactive desktop session, then
//! supply CUA_SPACES_APP_E2E_URL (loopback only),
//! CUA_SPACES_APP_E2E_TOKEN_FILE, CUA_SPACES_APP_E2E_HOME (a scratch directory),
//! and CUA_BIN (the built Spaces CLI). Run with:
//! `cargo run --example windows_client_smoke`.
//! To also exercise real input, disconnect cleanup and reattachment to the
//! SAME media ticket/session, supply CUA_SPACES_APP_E2E_PAD_PID,
//! CUA_SPACES_APP_E2E_PAD_TITLE and CUA_SPACES_APP_E2E_PAD_LOG for a dedicated
//! upstream test_pad built with key/button-up logging. The runner never starts
//! a fixture or chooses another window when that exact fixture is absent.
//! No VM, host service, account sign-in, Keyvault or app-session export runs.
//! The scratch home is retained so the caller can stop its daemon afterward.
//! Errors name only the failed stage; bearer tokens and media URLs are never
//! written to output.

use cua_media_protocol::{
    v2, InputKeyState, InputModifier, InputPointerButton, InputPointerPhase, InteractiveInputBatch,
    InteractiveInputEvent, WindowSessionId,
};
use cua_spaces_lib::core::{AppCore, CoreConfig, DaemonMode, StreamOpts, StreamTargetArg};
use futures_util::{SinkExt, StreamExt};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

type SmokeResult<T> = Result<T, &'static str>;
type MediaSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct PadFixture {
    pid: u32,
    title: String,
    log: PathBuf,
}

fn pad_fixture() -> SmokeResult<Option<PadFixture>> {
    let keys = [
        "CUA_SPACES_APP_E2E_PAD_PID",
        "CUA_SPACES_APP_E2E_PAD_TITLE",
        "CUA_SPACES_APP_E2E_PAD_LOG",
    ];
    if keys.iter().all(|key| std::env::var_os(key).is_none()) {
        return Ok(None);
    }
    let pid = required(keys[0])?
        .parse::<u32>()
        .map_err(|_| "invalid dedicated pad PID")?;
    let title = required(keys[1])?;
    let log = PathBuf::from(required(keys[2])?);
    if pid == 0 || !log.is_absolute() || !log.is_file() {
        return Err("a dedicated instrumented pad PID and absolute existing log are required");
    }
    Ok(Some(PadFixture { pid, title, log }))
}

fn fixture_offset(pad: &PadFixture) -> SmokeResult<usize> {
    usize::try_from(
        std::fs::metadata(&pad.log)
            .map_err(|_| "pad log is unreadable")?
            .len(),
    )
    .map_err(|_| "pad log is too large")
}

fn fixture_events(pad: &PadFixture, offset: usize) -> SmokeResult<Vec<serde_json::Value>> {
    let bytes = std::fs::read(&pad.log).map_err(|_| "pad log read failed")?;
    let tail = bytes
        .get(offset..)
        .ok_or("pad log was truncated during the run")?;
    Ok(String::from_utf8_lossy(tail)
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

async fn wait_fixture(
    pad: &PadFixture,
    offset: usize,
    matches: impl Fn(&[serde_json::Value]) -> bool,
    failure: &'static str,
) -> SmokeResult<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if matches(&fixture_events(pad, offset)?) {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(failure);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn typed(events: &[serde_json::Value]) -> String {
    events
        .iter()
        .filter(|event| event["event"] == "char")
        .filter_map(|event| event["char"].as_str())
        .collect()
}

async fn media_ready(socket: &mut MediaSocket, expected: &str) -> SmokeResult<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut hello = false;
    let mut opened = false;
    let mut first_video = None;
    let mut videos = 0;
    loop {
        let message = tokio::time::timeout_at(deadline, socket.next())
            .await
            .map_err(|_| "reconnect media greeting or frames timed out")?
            .ok_or("reconnect media socket closed")?
            .map_err(|_| "reconnect media read failed")?;
        match message {
            Message::Text(text) => {
                match v2::decode_server_text(&text).map_err(|_| "invalid typed media control")? {
                    v2::ServerMessage::Hello(_) => hello = true,
                    v2::ServerMessage::SessionOpened(session) => {
                        if !hello || session.session_id.0 != expected {
                            return Err("reattachment did not greet the original media session");
                        }
                        opened = true;
                    }
                    v2::ServerMessage::Error { .. } => {
                        return Err("media session reported an error")
                    }
                    _ => {}
                }
            }
            Message::Binary(bytes) if bytes.len() >= 8 => {
                let header_len =
                    u32::from_be_bytes(bytes[..4].try_into().map_err(|_| "invalid video header")?)
                        as usize;
                let payload_len =
                    u32::from_be_bytes(bytes[4..8].try_into().map_err(|_| "invalid video length")?)
                        as usize;
                let end = 8usize
                    .checked_add(header_len)
                    .filter(|end| *end <= bytes.len())
                    .ok_or("invalid reconnect video frame")?;
                if end.checked_add(payload_len) != Some(bytes.len()) {
                    return Err("reconnect video payload mismatch");
                }
                let header: serde_json::Value = serde_json::from_slice(&bytes[8..end])
                    .map_err(|_| "invalid reconnect video descriptor")?;
                if header["direction"] == "video" {
                    if payload_len == 0
                        || header["message"]["width_px"].as_u64().unwrap_or(0) == 0
                        || header["message"]["height_px"].as_u64().unwrap_or(0) == 0
                    {
                        return Err("reconnect video had no dimensions or image payload");
                    }
                    first_video
                        .get_or_insert(header["message"]["keyframe"].as_bool().unwrap_or(false));
                    videos += 1;
                }
            }
            Message::Ping(payload) => socket
                .send(Message::Pong(payload))
                .await
                .map_err(|_| "media pong failed")?,
            Message::Close(_) => return Err("media session closed before reconnect evidence"),
            _ => {}
        }
        if hello && opened && videos >= 3 {
            if first_video != Some(true) {
                return Err("reattachment did not begin with a keyframe");
            }
            return Ok(());
        }
    }
}

async fn media_connect(url: &str, expected: &str) -> SmokeResult<MediaSocket> {
    let (mut socket, _) = tokio::time::timeout(
        Duration::from_secs(15),
        tokio_tungstenite::connect_async(url),
    )
    .await
    .map_err(|_| "reconnect socket attach timed out")?
    .map_err(|_| "reconnect socket attach failed")?;
    media_ready(&mut socket, expected).await?;
    Ok(socket)
}

async fn input_batch(
    socket: &mut MediaSocket,
    session: &str,
    first_sequence: u64,
    events: Vec<InteractiveInputEvent>,
) -> SmokeResult<()> {
    let batch = InteractiveInputBatch {
        session_id: WindowSessionId(session.to_owned()),
        first_sequence,
        events,
    };
    let through = batch
        .validate()
        .map_err(|_| "invalid typed E2E input batch")?;
    socket
        .send(Message::Text(
            v2::encode_client_text(&v2::ClientMessage::InteractiveInput(batch)).into(),
        ))
        .await
        .map_err(|_| "media input send failed")?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let message = tokio::time::timeout_at(deadline, socket.next())
            .await
            .map_err(|_| "native input acknowledgement timed out")?
            .ok_or("input socket closed before acknowledgement")?
            .map_err(|_| "input acknowledgement read failed")?;
        match message {
            Message::Text(text) => match v2::decode_server_text(&text)
                .map_err(|_| "invalid typed input acknowledgement")?
            {
                v2::ServerMessage::InteractiveInputAcknowledgement(ack)
                    if ack.session_id.0 == session && ack.through_sequence == through =>
                {
                    return if ack.delivered && ack.error.is_none() {
                        Ok(())
                    } else {
                        Err("native input was refused")
                    };
                }
                v2::ServerMessage::Error { .. } => return Err("input stream reported an error"),
                _ => {}
            },
            Message::Ping(payload) => socket
                .send(Message::Pong(payload))
                .await
                .map_err(|_| "input pong failed")?,
            Message::Close(_) => return Err("input socket closed before acknowledgement"),
            _ => {}
        }
    }
}

async fn same_session_reconnect(url: &str, session: &str, pad: &PadFixture) -> SmokeResult<()> {
    let first_marker = format!("cua_same_session_before_{}", std::process::id());
    let second_marker = format!("cua_same_session_after_{}", std::process::id());
    let first_log = fixture_offset(pad)?;
    let mut socket = media_connect(url, session).await?;
    input_batch(
        &mut socket,
        session,
        1,
        vec![InteractiveInputEvent::TextCommit {
            text: first_marker.clone(),
        }],
    )
    .await?;
    wait_fixture(
        pad,
        first_log,
        |events| typed(events).contains(&first_marker),
        "first socket text did not reach the dedicated pad",
    )
    .await?;
    let held_log = fixture_offset(pad)?;
    let key = |name: &str| InteractiveInputEvent::Key {
        key: name.to_owned(),
        state: InputKeyState::Down,
        modifiers: Vec::new(),
        repeat: false,
    };
    input_batch(
        &mut socket,
        session,
        2,
        vec![
            key("control"),
            key("shift"),
            InteractiveInputEvent::Pointer {
                phase: InputPointerPhase::Down,
                button: Some(InputPointerButton::Left),
                x_normalized: 0.5,
                y_normalized: 0.5,
                modifiers: vec![InputModifier::Control, InputModifier::Shift],
            },
        ],
    )
    .await?;
    wait_fixture(
        pad,
        held_log,
        |events| {
            [16, 17].iter().all(|vk| {
                events
                    .iter()
                    .any(|event| event["event"] == "key" && event["vk"].as_u64() == Some(*vk))
            }) && events
                .iter()
                .any(|event| event["event"] == "click" && event["button"] == "left")
        },
        "held modifiers or button did not reach the pad",
    )
    .await?;
    if fixture_events(pad, held_log)?.iter().any(|event| {
        (event["event"] == "key_up" && matches!(event["vk"].as_u64(), Some(16 | 17)))
            || (event["event"] == "pointer_up" && event["button"] == "left")
    }) {
        return Err("held input was released before socket disconnect");
    }
    let release_log = fixture_offset(pad)?;
    // Deliberately send NO up/cancel events: the host must release held edges
    // when this final socket disconnects, while retaining the session lease.
    socket
        .close(None)
        .await
        .map_err(|_| "first socket disconnect failed")?;
    drop(socket);
    wait_fixture(
        pad,
        release_log,
        |events| {
            [16, 17].iter().all(|vk| {
                events
                    .iter()
                    .any(|event| event["event"] == "key_up" && event["vk"].as_u64() == Some(*vk))
            }) && events
                .iter()
                .any(|event| event["event"] == "pointer_up" && event["button"] == "left")
        },
        "disconnect did not release held modifiers and button",
    )
    .await?;
    let second_log = fixture_offset(pad)?;
    // Reuse the EXACT URL returned by OpenMedia, without another open_stream.
    let mut reattached = media_connect(url, session).await?;
    input_batch(
        &mut reattached,
        session,
        1,
        vec![InteractiveInputEvent::TextCommit {
            text: second_marker.clone(),
        }],
    )
    .await?;
    wait_fixture(
        pad,
        second_log,
        |events| typed(events).contains(&second_marker),
        "same-session reattached input did not reach the pad",
    )
    .await?;
    reattached
        .close(None)
        .await
        .map_err(|_| "reattached socket disconnect failed")?;
    println!("PASS same media session reattached with frames and received text; disconnect released Ctrl, Shift and left button");
    Ok(())
}

fn required(name: &str) -> SmokeResult<String> {
    std::env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or("required E2E environment is missing")
}

fn loopback_url() -> SmokeResult<String> {
    let value = required("CUA_SPACES_APP_E2E_URL")?;
    let url = reqwest::Url::parse(&value).map_err(|_| "invalid server URL")?;
    let host = url.host_str().unwrap_or_default().trim_matches(['[', ']']);
    let local = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    if !local
        || url.scheme() != "http"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("the E2E server must be an HTTP loopback URL without credentials");
    }
    Ok(value)
}

async fn attach(url: &str) -> SmokeResult<usize> {
    let (mut socket, _) = tokio::time::timeout(
        Duration::from_secs(15),
        tokio_tungstenite::connect_async(url),
    )
    .await
    .map_err(|_| "media connection timed out")?
    .map_err(|_| "media connection failed")?;
    let result = async {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut first_control = true;
        let mut hello = false;
        let mut session = false;
        let mut keyframe = None;
        let mut packets = 0;
        for _ in 0..400 {
            let message = tokio::time::timeout_at(deadline, socket.next())
                .await
                .map_err(|_| "media frame wait timed out")?
                .ok_or("media socket closed before frames")?
                .map_err(|_| "media socket read failed")?;
            match message {
                Message::Text(text) => {
                    let control: serde_json::Value =
                        serde_json::from_str(&text).map_err(|_| "invalid media control")?;
                    let kind = control["type"].as_str().unwrap_or_default();
                    if first_control {
                        if kind != "hello" {
                            return Err("media did not start with hello");
                        }
                        first_control = false;
                    }
                    hello |= kind == "hello";
                    session |= kind == "session_opened";
                }
                Message::Binary(bytes) if bytes.len() >= 8 => {
                    let header_len =
                        u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
                    let payload_len =
                        u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
                    let end = 8usize
                        .checked_add(header_len)
                        .filter(|end| *end <= bytes.len())
                        .ok_or("invalid media frame length")?;
                    if end.checked_add(payload_len) != Some(bytes.len()) {
                        return Err("media payload length mismatch");
                    }
                    let header: serde_json::Value = serde_json::from_slice(&bytes[8..end])
                        .map_err(|_| "invalid media frame header")?;
                    if header["direction"] == "video" {
                        if payload_len == 0
                            || header["message"]["width_px"].as_u64().unwrap_or(0) == 0
                            || header["message"]["height_px"].as_u64().unwrap_or(0) == 0
                        {
                            return Err("video packet had no image payload or dimensions");
                        }
                        keyframe.get_or_insert(
                            header["message"]["keyframe"].as_bool().unwrap_or(false),
                        );
                        packets += 1;
                        if packets >= 3 && hello && session {
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
        if !hello || !session || packets < 3 || keyframe != Some(true) {
            return Err("media hello, session or initial keyframe evidence was missing");
        }
        Ok(packets)
    }
    .await;
    let _ = socket.close(None).await;
    result
}

async fn smoke() -> SmokeResult<()> {
    if !cfg!(windows) {
        return Err("this smoke runner requires Windows");
    }
    let url = loopback_url()?;
    let pad = pad_fixture()?;
    let token = std::fs::read_to_string(required("CUA_SPACES_APP_E2E_TOKEN_FILE")?)
        .map_err(|_| "cannot read the server token file")?;
    let token = token.trim().to_owned();
    if token.is_empty() {
        return Err("the server token file is empty");
    }
    let home = PathBuf::from(required("CUA_SPACES_APP_E2E_HOME")?);
    // A caller must name a dedicated scratch directory, never the default
    // account state. AppCore uses a file-only credential store under this dir.
    if !home.is_absolute() || home.file_name().is_some_and(|name| name == ".cua") {
        return Err("E2E home must be an absolute dedicated scratch directory");
    }
    std::fs::create_dir_all(&home).map_err(|_| "cannot create scratch home")?;
    let cli = PathBuf::from(required("CUA_BIN")?);
    if !cli.is_file() {
        return Err("the built Spaces CLI is missing");
    }
    let mut config = CoreConfig::hermetic(&home);
    config.probe_timeout = Duration::from_secs(20);
    config.daemon = DaemonMode::Auto { cua_bin: Some(cli) };
    let app = AppCore::new(config);
    if !app.daemon_status(true).await.connected {
        return Err("isolated app daemon startup failed");
    }
    println!("PASS app daemon startup");
    let row = app
        .add_space(&url, Some(token), Some("Windows client E2E".into()))
        .await
        .map_err(|_| "add Space failed")?;
    let mut remote_file = None;
    let mut media_session = None;
    let result = async {
        if !row.reachable || row.os.as_deref() != Some("windows") {
            return Err("the registered Space is not a reachable Windows server");
        }
        let rows = app.list_spaces().await.map_err(|_| "list Spaces failed")?;
        if !rows.iter().any(|item| item.id == row.id && item.reachable) {
            return Err("registered Space was missing from the real list");
        }
        println!("PASS add and list Windows Space");
        let shot = app
            .space_screenshot(&row.id, Some(640))
            .await
            .map_err(|_| "real screenshot failed")?;
        if !shot.starts_with("data:image/") || shot.len() < 200 {
            return Err("real screenshot was empty");
        }
        let windows = app
            .list_remote_windows(&row.id)
            .await
            .map_err(|_| "remote window enumeration failed")?;
        if windows.is_empty() {
            return Err("no real desktop windows were enumerated");
        }
        println!("PASS screenshot and {} remote windows", windows.len());
        let scratch = tempfile::tempdir_in(&home).map_err(|_| "scratch file setup failed")?;
        let file = scratch.path().join("windows-client-smoke.bin");
        let bytes: Vec<u8> = (0..3 * 1024 * 1024u32)
            .map(|index| (index * 31 % 251) as u8)
            .collect();
        std::fs::write(&file, &bytes).map_err(|_| "scratch file write failed")?;
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let sent = app
            .send_files(
                &row.id,
                &[file.to_string_lossy().into_owned()],
                Some(format!("cua-windows-e2e-{}", std::process::id())),
            )
            .await
            .map_err(|_| "real file transfer failed")?;
        if sent.len() != 1 {
            return Err("file transfer did not return one file");
        }
        remote_file = Some(sent[0].dest.clone());
        if sent[0].bytes != bytes.len() as u64 || sent[0].sha256 != hash {
            return Err("server file transfer size or SHA-256 mismatch");
        }
        let space = app
            .spaces()
            .space(&row.id)
            .await
            .map_err(|_| "Space lookup failed")?;
        let downloaded = space
            .spacesd()
            .map_err(|_| "server connection lookup failed")?
            .download(&sent[0].dest)
            .await
            .map_err(|_| "transferred file readback failed")?;
        if downloaded.as_ref() != bytes.as_slice() {
            return Err("transferred file bytes differed on readback");
        }
        println!(
            "PASS {} byte transfer, SHA-256 and real readback",
            bytes.len()
        );
        let ticket = app
            .open_stream(
                &row.id,
                StreamTargetArg::Display { display_id: None },
                StreamOpts {
                    max_fps: Some(10),
                    max_dimension: Some(960),
                    audio: Some(false),
                    policy: Some("view_only".into()),
                    ..Default::default()
                },
            )
            .await
            .map_err(|_| "real desktop stream open failed")?;
        media_session = Some(ticket.media_session_id.clone());
        if ticket.via != "direct" || ticket.wire_version != 2 {
            return Err("stream was not a direct wire-v2 desktop stream");
        }
        let packets = attach(&ticket.ws_url).await?;
        app.close_stream(&row.id, &ticket.media_session_id)
            .await
            .map_err(|_| "desktop stream close failed")?;
        media_session = None;
        println!("PASS direct desktop stream hello, session, keyframe and {packets} video packets");
        if let Some(pad) = &pad {
            let windows = app
                .list_remote_windows(&row.id)
                .await
                .map_err(|_| "dedicated pad enumeration failed")?;
            let candidates: Vec<_> = windows
                .iter()
                .filter(|window| {
                    window.pid == Some(pad.pid)
                        && (window.title == pad.title
                            || window.title.starts_with(&format!("{} [", pad.title)))
                })
                .collect();
            if candidates.len() != 1 {
                return Err("expected exactly one dedicated pad window");
            }
            let ticket = app
                .open_stream(
                    &row.id,
                    StreamTargetArg::Window {
                        window_id: candidates[0].id.clone(),
                    },
                    StreamOpts {
                        max_fps: Some(10),
                        max_dimension: Some(960),
                        audio: Some(false),
                        policy: Some("background_only".into()),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|_| "dedicated pad stream open failed")?;
            media_session = Some(ticket.media_session_id.clone());
            if ticket.via != "direct" || ticket.wire_version != 2 {
                return Err("pad stream was not direct wire-v2");
            }
            same_session_reconnect(&ticket.ws_url, &ticket.media_session_id, pad).await?;
            app.close_stream(&row.id, &ticket.media_session_id)
                .await
                .map_err(|_| "reconnect media session close failed")?;
            media_session = None;
        } else {
            println!("SKIP same-session input reconnect: no dedicated instrumented pad supplied");
        }
        Ok(())
    }
    .await;
    // Clean up through the real public interfaces even after a failed stage.
    if let Some(session) = media_session {
        let _ = app.close_stream(&row.id, &session).await;
    }
    let mut cleaned = true;
    if let Some(path) = remote_file {
        if let Ok(space) = app.spaces().space(&row.id).await {
            if let Ok(client) = space.spacesd() {
                cleaned &= client.remove(&path, false).await.is_ok();
            } else {
                cleaned = false;
            }
        } else {
            cleaned = false;
        }
    }
    cleaned &= app.remove_space(&row.id).await.is_ok();
    if cleaned {
        println!("PASS scratch file and Space registry cleanup");
    } else {
        eprintln!("FAIL scratch file or registry cleanup");
    }
    result?;
    if !cleaned {
        return Err("scratch file or registry cleanup failed");
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(stage) = smoke().await {
        eprintln!("FAIL {stage}");
        std::process::exit(1);
    }
}
