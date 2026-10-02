// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Opt-in Windows E2E for the same AppCore used by the Tauri commands.
//! Start an official Windows spacesd in the interactive desktop session, then
//! supply CUA_SPACES_APP_E2E_URL (loopback only),
//! CUA_SPACES_APP_E2E_TOKEN_FILE, CUA_SPACES_APP_E2E_HOME (a scratch directory),
//! and CUA_BIN (the built Spaces CLI). Run with:
//! `cargo run --example windows_client_smoke`.
//! No VM, host service, account sign-in, Keyvault or app-session export runs.
//! The scratch home is retained so the caller can stop its daemon afterward.
//! Errors name only the failed stage; bearer tokens and media URLs are never
//! written to output.

use cua_spaces_lib::core::{AppCore, CoreConfig, DaemonMode, StreamOpts, StreamTargetArg};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

type SmokeResult<T> = Result<T, &'static str>;

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
