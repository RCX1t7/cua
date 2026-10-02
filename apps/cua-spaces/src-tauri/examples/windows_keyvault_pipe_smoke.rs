// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Opt-in, TWO-PROCESS Windows pipe smoke. Transport, kernel peer identity,
//! framing, encryption and broker operations are real. Data and user presence
//! are synthetic, using the existing upstream fixture; this does NOT certify
//! Windows Hello, production signing authority or actual browser credentials.
//!
//! Set CUA_WINDOWS_KEYVAULT_E2E_HOME to a NEW absolute task scratch directory.
//! Run a debug build only. Never run against ~/.cua. The scratch vault/logs stay
//! for review; only this example's child server exits when the probe finishes.

#[cfg(target_os = "windows")]
#[path = "../../../../libs/cua/crates/cua-keyvault/tests/common/mod.rs"]
mod fixtures;

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("This opt-in pipe smoke requires a Windows debug build.");
    std::process::exit(2);
}

#[cfg(target_os = "windows")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::{BufRead, Read};
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::sync::Arc;
    use std::time::Duration;

    use cua_keyvault::broker::{
        BrokerConfig, FakePresence, ImportSpec, InitRequest, UnlockRequest,
    };
    use cua_keyvault::ipc::{self, ConnectError, KeyvaultClient, ServerCheck};
    use cua_keyvault::{Broker, Error, TrustPolicy};

    if !cfg!(debug_assertions) {
        return Err("only an explicitly isolated debug fixture is permitted".into());
    }
    let fixture_home = PathBuf::from(std::env::var("CUA_WINDOWS_KEYVAULT_E2E_HOME")?);
    if !fixture_home.is_absolute()
        || fixture_home.file_name().is_some_and(|n| n == ".cua")
        || fixture_home.parent().is_none()
    {
        return Err("supply a new absolute task scratch directory, never the Cua home".into());
    }
    let executable = std::env::current_exe()?;
    let mut image = std::fs::File::open(&executable)?;
    let mut digest = sha2::Sha256::new();
    use sha2::Digest;
    let mut chunk = [0u8; 65536];
    loop {
        let count = image.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        digest.update(&chunk[..count]);
    }
    let hash = format!("{:x}", digest.finalize());
    let policy = TrustPolicy::for_windows_tests(vec![hash]);
    let endpoint = fixture_home.join("keyvault.sock");

    if std::env::args().nth(1).as_deref() == Some("--fixture-server") {
        let broker = Arc::new(Broker::new(
            BrokerConfig {
                dir: fixture_home.join("vault"),
                keychain_path: None,
                os_protector: false,
            },
            Arc::new(fixtures::FakeBackend::default()),
            Arc::new(FakePresence::new(true)),
        )?);
        let listener = ipc::bind(&endpoint).await?;
        println!("READY: isolated synthetic backend/presence, OS protector disabled");
        use std::io::Write;
        std::io::stdout().flush()?;
        tokio::select! {
            _ = ipc::serve(listener, broker, policy) => {},
            _ = tokio::time::sleep(Duration::from_secs(120)) => {},
        }
        return Ok(());
    }

    if fixture_home.exists() {
        return Err(
            "the fixture directory already exists; preserve it and choose a new directory".into(),
        );
    }
    std::fs::create_dir_all(&fixture_home)?;
    let mut child = Command::new(&executable)
        .arg("--fixture-server")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let server_pid = child.id();
    let stdout = child
        .stdout
        .take()
        .ok_or("fixture server stdout is missing")?;
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let result = std::io::BufReader::new(stdout)
            .read_line(&mut line)
            .map(|_| line.starts_with("READY:"));
        let _ = ready_tx.send(result);
    });
    // Owns ONLY the child created above, never the user's app, daemon, build or
    // another work thread. Scratch outputs are preserved even on failure.
    struct FixtureChild(std::process::Child);
    impl Drop for FixtureChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _child = FixtureChild(child);
    if !ready_rx.recv_timeout(Duration::from_secs(30))?? {
        return Err("fixture server did not become ready".into());
    }

    let production = KeyvaultClient::connect(&endpoint, ServerCheck::default_for_build()).await;
    if !matches!(production, Err(ConnectError::Impostor { .. })) {
        return Err("production policy must refuse the unverified test broker".into());
    }
    println!("PASS production policy refuses unproved running-code authority");
    let unchecked = KeyvaultClient::connect(&endpoint, ServerCheck::Unverified).await;
    if !matches!(unchecked, Err(ConnectError::Other(_))) {
        return Err("unverified Windows transport must be refused".into());
    }
    println!("PASS unchecked Windows transport is refused");
    let wrong_policy = TrustPolicy::for_windows_tests(vec!["00".repeat(32)]);
    let wrong = KeyvaultClient::connect(&endpoint, ServerCheck::Require(wrong_policy)).await;
    if !matches!(wrong, Err(ConnectError::Impostor { .. })) {
        return Err("a different fixture executable hash must be refused".into());
    }
    println!("PASS exact-byte fixture policy refuses the wrong hash");

    let mut client = KeyvaultClient::connect(&endpoint, ServerCheck::Require(policy)).await?;
    let server = client
        .server
        .as_ref()
        .ok_or("kernel server identity is missing")?;
    if u32::try_from(server.pid)? != server_pid || server.os_verified || !server.first_party {
        return Err("server PID/test identity did not match its actual unverified child".into());
    }
    println!("PASS two-process kernel server PID (explicit unverified test identity)");
    let status = client.status().await?;
    if status.initialized || status.os_protector_available || !status.caller_first_party {
        return Err(
            "fresh isolated vault/test caller/disabled OS protector state is incorrect".into(),
        );
    }
    println!("PASS actual pipe status/frame round trip; OS protector unavailable");
    let os_request = client
        .init(InitRequest {
            os_protector: true,
            passphrase: None,
            recovery_key: false,
        })
        .await;
    if !matches!(os_request, Err(Error::Unsupported(_))) {
        return Err("OS credential enrollment must be refused before access".into());
    }
    println!("PASS OS credential enrollment is explicitly refused");

    // A PUBLIC manufactured passphrase for disposable fake data. Never prompt,
    // read user credentials, set an auth bypass or copy a browser profile.
    const PASSPHRASE: &str = "synthetic-pipe-fixture-passphrase-only";
    client
        .init(InitRequest {
            os_protector: false,
            passphrase: Some(PASSPHRASE.into()),
            recovery_key: false,
        })
        .await?;
    let status = client.status().await?;
    if !status.initialized || !status.unlocked || status.os_protector_available {
        return Err("passphrase-only synthetic initialization failed".into());
    }
    println!("PASS passphrase-only encrypted fixture initialization");
    client
        .import(ImportSpec {
            app: "chrome".into(),
            whole_app: true,
            ..Default::default()
        })
        .await?;
    client.browse().await?;
    let page = client.list_items().await?;
    if page.items.is_empty() || !page.names_visible {
        return Err("synthetic capture/browse/list did not round-trip over the pipe".into());
    }
    client
        .delete_items(page.items.iter().map(|item| item.id.clone()).collect())
        .await?;
    if !client.list_items().await?.items.is_empty() {
        return Err("synthetic batch deletion did not round-trip".into());
    }
    println!("PASS synthetic import/browse/list/batch delete over actual pipe");
    client.end_browse().await?;
    client.lock().await?;
    if client.status().await?.unlocked {
        return Err("lock did not close the fixture vault".into());
    }
    client
        .unlock(UnlockRequest {
            passphrase: Some(PASSPHRASE.into()),
            recovery_key: None,
        })
        .await?;
    if !client.status().await?.unlocked {
        return Err("passphrase unlock did not round-trip".into());
    }
    println!("PASS lock/passphrase unlock over actual pipe");
    if !client.verify_audit().await?.ok {
        return Err("fixture audit chain failed verification".into());
    }
    println!("PASS broker audit chain verification");
    std::fs::write(
        fixture_home.join("result.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "transport": "two-process Windows local named pipe", "passed": true,
            "server_pid": server_pid, "os_verified": false, "policy": "explicit exact-byte DEBUG fixture",
            "backend": "upstream synthetic test backend", "presence": "upstream injected FakePresence",
            "os_credentials_accessed": false, "real_browser_profiles_accessed": false,
            "native_authentication_certified": false, "production_keyvault_certified": false
        }))?,
    )?;
    println!(
        "PASS synthetic Windows pipe smoke complete (production/authentication NOT certified)"
    );
    Ok(())
}
