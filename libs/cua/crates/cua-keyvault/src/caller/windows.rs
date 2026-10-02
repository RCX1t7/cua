// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Windows local pipe identity and Authenticode FILE evidence.
//!
//! PID and SID come from the kernel. A retained process handle prevents later
//! PID reuse from substituting a peer, and a read-only image handle denies
//! subsequent writes/renames while the connection exists. WinVerifyTrust checks
//! that file's chain and signature without UI or network retrieval.
//!
//! Windows has no macOS audit-token/SecCode equivalent here: a disk path could
//! have changed BEFORE it was opened. Consequently even a pinned signed file
//! does not attest the loaded process image: `os_verified` and production
//! `first_party` stay FALSE. A future packaged/kernel-attested identity must
//! close that gap before production Keyvault clients can gain authority.
//! Exact image hashes may identify explicitly marked isolated DEBUG fixtures;
//! they are unverified and must never be used with the OS credential protector.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::path::Path;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{LocalFree, HANDLE, HLOCAL, HWND, WAIT_TIMEOUT};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
};
use windows::Win32::Security::Cryptography::{
    CertGetCertificateContextProperty, CertGetNameStringW, CERT_NAME_SIMPLE_DISPLAY_TYPE,
    CERT_SHA256_HASH_PROP_ID,
};
use windows::Win32::Security::WinTrust::*;
use windows::Win32::Security::{
    GetTokenInformation, TokenSessionId, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
    TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::System::Pipes::{GetNamedPipeClientProcessId, GetNamedPipeServerProcessId};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
    WaitForSingleObject, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE,
};

use super::{CallerIdentity, PeerError, Signing, TrustPolicy};

fn error(e: impl std::fmt::Display) -> PeerError {
    PeerError::Unavailable(e.to_string())
}

fn handle(owned: &OwnedHandle) -> HANDLE {
    HANDLE(owned.as_raw_handle())
}

/// Retains the exact peer process and the locked file throughout a connection.
pub struct Peer {
    /// Kernel process/SID identity and explicitly unverified file evidence.
    pub identity: CallerIdentity,
    /// Whether the validated FILE certificate matched an explicit public pin.
    /// This is evidence only; it is never a live process authority.
    pub certificate_pinned: bool,
    process: OwnedHandle,
    _image: File,
}

impl Peer {
    /// Refuses a disconnected/dead identity rather than reusing its PID.
    pub fn ensure_alive(&self) -> Result<(), PeerError> {
        // SAFETY: owned process handle has SYNCHRONIZE access and is retained.
        if unsafe { WaitForSingleObject(handle(&self.process), 0) } == WAIT_TIMEOUT {
            Ok(())
        } else {
            Err(error("the verified pipe peer has exited"))
        }
    }
}

fn token_identity(process: HANDLE) -> Result<(String, u32), PeerError> {
    let mut token = HANDLE::default();
    // SAFETY: opens a read-only query token for a live process handle.
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.map_err(error)?;
    // SAFETY: OpenProcessToken returned an owned handle.
    let token = unsafe { OwnedHandle::from_raw_handle(token.0) };
    let mut length = 0u32;
    // Initial query obtains the required buffer size; insufficient buffer is
    // expected. usize storage supplies TOKEN_USER's required alignment.
    let _ = unsafe { GetTokenInformation(handle(&token), TokenUser, None, 0, &mut length) };
    if length == 0 || length > 1024 * 1024 {
        return Err(error("Windows token user size is invalid"));
    }
    let mut buffer = vec![0usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
    // SAFETY: buffer is aligned and large enough for the requested TOKEN_USER.
    unsafe {
        GetTokenInformation(
            handle(&token),
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            length,
            &mut length,
        )
    }
    .map_err(error)?;
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let mut text = PWSTR::null();
    unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) }.map_err(error)?;
    let sid = unsafe { text.to_string() }.map_err(error);
    // SAFETY: ConvertSidToStringSid allocates with LocalAlloc; consume then free.
    unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
    let sid = sid?;
    let mut session = 0u32;
    unsafe {
        GetTokenInformation(
            handle(&token),
            TokenSessionId,
            Some((&mut session as *mut u32).cast()),
            4,
            &mut length,
        )
    }
    .map_err(error)?;
    Ok((sid, session))
}

/// The pipe name is bound to the current account and resolved home path.
pub fn pipe_name(path: &Path) -> Result<String, PeerError> {
    let (sid, session) = token_identity(unsafe { GetCurrentProcess() })?;
    let parent = path
        .parent()
        .ok_or_else(|| error("Keyvault endpoint has no parent"))?;
    let parent = std::fs::canonicalize(parent).map_err(error)?;
    let endpoint = parent.join(
        path.file_name()
            .ok_or_else(|| error("Keyvault endpoint has no name"))?,
    );
    let bytes = format!(
        "{sid}|session:{session}|{}",
        endpoint.to_string_lossy().to_ascii_lowercase()
    );
    Ok(format!(
        r"\\.\pipe\cua-keyvault-{}",
        hex::encode(crate::crypto::sha256(bytes.as_bytes()))
    ))
}

/// Creates a local-only pipe with an explicit current-user DACL. No Everyone,
/// anonymous, remote-user, inheritable or browser transport permission exists.
pub fn create_pipe(
    name: &str,
    first: bool,
) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    use tokio::net::windows::named_pipe::ServerOptions;
    let (sid, _) = token_identity(unsafe { GetCurrentProcess() }).map_err(std::io::Error::other)?;
    let sddl: Vec<u16> = format!("O:{sid}D:P(A;;GA;;;{sid})")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            1,
            &mut descriptor,
            None,
        )
    }
    .map_err(std::io::Error::other)?;
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    // SAFETY: descriptor/attributes are alive for CreateNamedPipe, which copies
    // them. first_pipe_instance prevents silently occupying an existing server.
    let result = unsafe {
        ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(
                name,
                (&mut attributes as *mut SECURITY_ATTRIBUTES).cast(),
            )
    };
    unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
    result
}

/// Identifies either end before any request bytes are sent or consumed.
pub fn identify_pipe(
    pipe: RawHandle,
    server_end: bool,
    policy: &TrustPolicy,
) -> Result<Peer, PeerError> {
    let mut pid = 0u32;
    let get_pid = |out: &mut u32| unsafe {
        if server_end {
            GetNamedPipeClientProcessId(HANDLE(pipe), out)
        } else {
            GetNamedPipeServerProcessId(HANDLE(pipe), out)
        }
    };
    get_pid(&mut pid).map_err(error)?;
    let process = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            false,
            pid,
        )
    }
    .map_err(error)?;
    let process = unsafe { OwnedHandle::from_raw_handle(process.0) };
    let ours = token_identity(unsafe { GetCurrentProcess() })?;
    let theirs = token_identity(handle(&process))?;
    if ours != theirs {
        return Err(error(
            "Keyvault peer belongs to a different Windows account or logon session",
        ));
    }
    let mut name = vec![0u16; 32768];
    let mut length = name.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            handle(&process),
            PROCESS_NAME_WIN32,
            PWSTR(name.as_mut_ptr()),
            &mut length,
        )
    }
    .map_err(error)?;
    let path = std::ffi::OsString::from_wide(&name[..length as usize]);
    // Deny modifications and deletion while verifying and using this image.
    let mut image = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&path)
        .map_err(error)?;
    let sha = executable_sha256(&mut image).map_err(error)?;
    let signing = file_signing(&image, Path::new(&path), &sha).unwrap_or(Signing::Unsigned);
    let certificate_pinned = match &signing {
        Signing::WindowsSigned {
            certificate_sha256, ..
        } => policy
            .windows_certificate_sha256
            .iter()
            .any(|pin| pin.eq_ignore_ascii_case(certificate_sha256)),
        _ => false,
    };
    let mut second_pid = 0u32;
    get_pid(&mut second_pid).map_err(error)?;
    if second_pid != pid {
        return Err(error("Keyvault pipe peer changed during verification"));
    }
    let fixture = cfg!(debug_assertions)
        && policy.is_test_policy
        && policy
            .windows_test_executable_sha256
            .iter()
            .any(|expected| expected.len() == 64 && expected.eq_ignore_ascii_case(&sha));
    let peer = Peer {
        identity: CallerIdentity {
            pid: i32::try_from(pid).map_err(error)?,
            // Unix uid is not an authority on Windows; SID/session were compared
            // above using tokens. Per-account pipe names also isolate grants.
            uid: 0,
            path: Some(Path::new(&path).to_string_lossy().into_owned()),
            signing,
            first_party: fixture,
            os_verified: false,
            launched_by: None,
            verified_name: None,
        },
        certificate_pinned,
        process,
        _image: image,
    };
    peer.ensure_alive()?;
    Ok(peer)
}

fn executable_sha256(file: &mut File) -> std::io::Result<String> {
    file.seek(SeekFrom::Start(0))?;
    let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
    let mut chunk = [0u8; 65536];
    loop {
        let count = file.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        digest.update(&chunk[..count]);
    }
    file.seek(SeekFrom::Start(0))?;
    Ok(hex::encode(digest.finish().as_ref()))
}

fn file_signing(file: &File, path: &Path, sha: &str) -> Result<Signing, PeerError> {
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut info = WINTRUST_FILE_INFO {
        cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: PCWSTR(path.as_ptr()),
        hFile: HANDLE(file.as_raw_handle()),
        ..Default::default()
    };
    let mut data = WINTRUST_DATA {
        cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_WHOLECHAIN,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 { pFile: &mut info },
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL | WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT,
        ..Default::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    // No interactive trust prompt, downloads, relaxed trust or root installation.
    let status = unsafe {
        WinVerifyTrust(
            HWND::default(),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        )
    };
    let result = (|| {
        if status != 0 {
            return Err(error(format!(
                "Authenticode verification failed ({status:#x})"
            )));
        }
        let provider = unsafe { WTHelperProvDataFromStateData(data.hWVTStateData) };
        if provider.is_null() {
            return Err(error("Authenticode provider state is unavailable"));
        }
        let signer = unsafe { WTHelperGetProvSignerFromChain(provider, 0, false, 0) };
        if signer.is_null()
            || unsafe { (*signer).csCertChain == 0 || (*signer).pasCertChain.is_null() }
        {
            return Err(error("Authenticode signing certificate is unavailable"));
        }
        let certificate = unsafe { (*(*signer).pasCertChain).pCert };
        if certificate.is_null() {
            return Err(error("Authenticode signing certificate is unavailable"));
        }
        let mut pin = [0u8; 32];
        let mut size = pin.len() as u32;
        unsafe {
            CertGetCertificateContextProperty(
                certificate,
                CERT_SHA256_HASH_PROP_ID,
                Some(pin.as_mut_ptr().cast()),
                &mut size,
            )
        }
        .map_err(error)?;
        if size != 32 {
            return Err(error("Authenticode certificate hash size is invalid"));
        }
        let mut publisher = vec![0u16; 1024];
        let count = unsafe {
            CertGetNameStringW(
                certificate,
                CERT_NAME_SIMPLE_DISPLAY_TYPE,
                0,
                None,
                Some(&mut publisher),
            )
        };
        if count == 0 || count as usize > publisher.len() {
            return Err(error("Authenticode publisher is unavailable"));
        }
        Ok(Signing::WindowsSigned {
            certificate_sha256: hex::encode(pin),
            publisher: String::from_utf16_lossy(&publisher[..count.saturating_sub(1) as usize]),
            executable_sha256: sha.into(),
        })
    })();
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    // SAFETY: close the provider state allocated by the matching VERIFY call.
    let _ = unsafe {
        WinVerifyTrust(
            HWND::default(),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        )
    };
    result
}

/// No live process signing authority has been established for this port.
/// Called before any Windows Credential Manager access, including auto-unlock.
pub fn require_production_authority() -> crate::Result<()> {
    Err(crate::Error::Unsupported(
        "Windows Keyvault OS credentials require an attested, trusted running app; \
         Authenticode file signatures alone cannot establish that authority in this port. \
         Use only an isolated passphrase test vault until a supported signed/package identity is configured.".into()
    ))
}
