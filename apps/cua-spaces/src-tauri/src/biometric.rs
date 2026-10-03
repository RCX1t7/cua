// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Native device-owner confirmation: Touch ID/passcode on macOS and
//! Windows Hello/PIN on Windows 11 (desktop interop requires build 22000).
//!
//! Used to authorize sensitive settings changes — currently the
//! unattended-teleport permissions, so they cannot be loosened without the
//! device owner present. Mirrors rcdp's app-session biometric gate.
//!
//! The existing macOS test bypass is never honored by the Windows gate.

#[cfg(target_os = "windows")]
pub fn register_app(app: tauri::AppHandle) {
    windows::register_app(app);
}

/// Authorize a sensitive action with the OS. Returns `Ok(())` when the device
/// owner authenticates, else a human-readable error.
pub fn authorize(reason: &str) -> Result<(), String> {
    #[cfg(not(target_os = "windows"))]
    if std::env::var("CUA_SKIP_BIOMETRIC")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        macos::authorize(reason)
    }
    #[cfg(target_os = "windows")]
    {
        windows::authorize(reason)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = reason;
        Err("biometric authorization is only available on macOS".into())
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use std::sync::{mpsc, OnceLock};
    use std::time::{Duration, Instant};

    use ::windows::core::{factory, HSTRING};
    use ::windows::Security::Credentials::UI::{
        UserConsentVerificationResult, UserConsentVerifier, UserConsentVerifierAvailability,
    };
    use ::windows::Win32::System::WinRT::{
        IUserConsentVerifierInterop, RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED,
    };
    use tauri::Manager;
    use windows_future::{AsyncOperationCompletedHandler, IAsyncOperation};

    static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
    const REPLY_TIMEOUT: Duration = Duration::from_secs(120);

    pub(super) fn register_app(app: tauri::AppHandle) {
        let _ = APP.set(app);
    }

    struct Runtime;
    impl Drop for Runtime {
        fn drop(&mut self) {
            // SAFETY: balances successful RoInitialize on this worker thread.
            unsafe { RoUninitialize() };
        }
    }

    /// Called on a blocking worker, never the Tauri event-loop thread. Only
    /// creation of the HWND-associated system prompt runs on the UI thread.
    pub(super) fn authorize(reason: &str) -> Result<(), String> {
        let app = APP
            .get()
            .ok_or("device authentication is not initialized")?;
        // SAFETY: initializes only this worker's WinRT apartment; no OS setting
        // or account credential is changed. Failure is a refusal.
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }
            .map_err(|_| "device authentication could not initialize Windows Runtime")?;
        let _runtime = Runtime;
        let deadline = Instant::now() + REPLY_TIMEOUT;
        let availability = UserConsentVerifier::CheckAvailabilityAsync()
            .map_err(|_| "Windows Hello availability could not be checked")?;
        wait_for(&availability, deadline)?;
        match availability
            .GetResults()
            .map_err(|_| "Windows Hello availability check failed")?
        {
            UserConsentVerifierAvailability::Available => {}
            UserConsentVerifierAvailability::NotConfiguredForUser => {
                return Err("Windows Hello is not configured for this Windows account".into());
            }
            UserConsentVerifierAvailability::DisabledByPolicy => {
                return Err("Windows Hello is disabled by Windows policy".into());
            }
            UserConsentVerifierAvailability::DeviceBusy => {
                return Err("Windows Hello is busy; try again".into());
            }
            _ => return Err("Windows Hello is not available on this device".into()),
        }

        let (tx, rx) = mpsc::channel();
        let owner = app.clone();
        let message = HSTRING::from(reason);
        app.run_on_main_thread(move || {
            let result: Result<IAsyncOperation<UserConsentVerificationResult>, String> = (|| {
                if Instant::now() >= deadline {
                    return Err("authentication timed out".to_string());
                }
                // Use a focused Tauri window owned by this app, rather than
                // whichever unrelated host application happens to be active.
                let window = owner.webview_windows().into_values()
                    .find(|w| w.is_focused().unwrap_or(false))
                    .ok_or("focus Cua Spaces before confirming this action")?;
                let hwnd = window.hwnd().map_err(|_| "authentication window is unavailable")?;
                let interop: IUserConsentVerifierInterop = factory::<UserConsentVerifier, _>()
                    .map_err(|_| "Windows Hello desktop confirmation requires Windows 11 (build 22000 or newer)")?;
                // SAFETY: HWND belongs to the live Tauri owner window, retained
                // above. The OS owns its confirmation UI and verifies the
                // current logged-on Windows account; no password is read.
                unsafe {
                    interop.RequestVerificationForWindowAsync::<IAsyncOperation<UserConsentVerificationResult>>(
                        ::windows::Win32::Foundation::HWND(hwnd.0), &message,
                    )
                }.map_err(|_| "Windows Hello confirmation could not be opened".into())
            })();
            if let Err(mpsc::SendError(Ok(operation))) = tx.send(result) {
                // The worker may have timed out while this UI task was queued.
                // Do not leave a late system prompt outstanding in that case.
                let _ = operation.Cancel();
            }
        }).map_err(|_| "authentication window is unavailable")?;
        let operation = rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| "authentication timed out")??;
        wait_for(&operation, deadline)?;
        match operation
            .GetResults()
            .map_err(|_| "authentication was denied")?
        {
            UserConsentVerificationResult::Verified => Ok(()),
            UserConsentVerificationResult::Canceled => Err("authentication was cancelled".into()),
            UserConsentVerificationResult::DeviceBusy => {
                Err("Windows Hello is busy; try again".into())
            }
            UserConsentVerificationResult::NotConfiguredForUser => {
                Err("Windows Hello is not configured for this Windows account".into())
            }
            UserConsentVerificationResult::DisabledByPolicy => {
                Err("Windows Hello is disabled by Windows policy".into())
            }
            UserConsentVerificationResult::DeviceNotPresent => {
                Err("Windows Hello is not available on this device".into())
            }
            _ => Err("authentication was denied".into()),
        }
    }

    fn wait_for<T: ::windows::core::RuntimeType + Send + 'static>(
        operation: &IAsyncOperation<T>,
        deadline: Instant,
    ) -> Result<(), String> {
        let (tx, rx) = mpsc::channel();
        operation
            .SetCompleted(&AsyncOperationCompletedHandler::new(move |_, _| {
                let _ = tx.send(());
                Ok(())
            }))
            .map_err(|_| "authentication could not wait for Windows Hello")?;
        if rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .is_err()
        {
            let _ = operation.Cancel();
            return Err("authentication timed out".into());
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod macos {
    //! `evaluatePolicy:localizedReason:reply:` is asynchronous — it invokes the
    //! completion block on a framework-private queue. We bridge back to a
    //! synchronous call by handing the block an `mpsc::Sender` and blocking on
    //! the receiver until the reply arrives; `LAContext` and the block are held
    //! on the stack across that wait so the evaluation is not cancelled.

    use std::sync::mpsc;
    use std::time::Duration;

    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSError, NSString};
    use objc2_local_authentication::{LAContext, LAPolicy};

    /// Upper bound on waiting for the user to answer before treating the prompt
    /// as cancelled, so a forgotten dialog cannot wedge the caller forever.
    const REPLY_TIMEOUT: Duration = Duration::from_secs(120);

    pub(super) fn authorize(reason: &str) -> Result<(), String> {
        // Touch ID OR device-passcode fallback (DeviceOwnerAuthentication): robust
        // where no biometric is enrolled but a passcode is set.
        let policy = LAPolicy::DeviceOwnerAuthentication;
        // SAFETY: `LAContext::new` returns a fresh, owned context.
        let context = unsafe { LAContext::new() };

        // Preflight: unevaluatable policy (no passcode / unsupported) is treated
        // as unavailable rather than raising a doomed prompt.
        // SAFETY: `context` is valid; the call only reads state.
        if unsafe { context.canEvaluatePolicy_error(policy) }.is_err() {
            return Err(
                "device authentication is not available (no passcode/biometric enrolled)".into(),
            );
        }

        let ns_reason = NSString::from_str(reason);
        let (tx, rx) = mpsc::channel::<Result<(), String>>();

        // The reply block runs on a private framework queue; `Sender` is `Send`,
        // so handing the result back over the channel is sound.
        let reply = RcBlock::new(move |success: Bool, error: *mut NSError| {
            let result = if success.as_bool() {
                Ok(())
            } else {
                // SAFETY: on failure a valid NSError is passed; guard null.
                let code = if error.is_null() {
                    0
                } else {
                    unsafe { (*error).code() }
                };
                Err(classify(code))
            };
            let _ = tx.send(result);
        });

        // SAFETY: arguments are valid; `reply`/`context` outlive the wait below.
        unsafe {
            context.evaluatePolicy_localizedReason_reply(policy, &ns_reason, &reply);
        }

        match rx.recv_timeout(REPLY_TIMEOUT) {
            Ok(result) => result,
            Err(_) => Err("authentication timed out".into()),
        }
    }

    /// Map an `LAError` code to a message. User/OS cancellations are
    /// distinguished from an outright denial.
    fn classify(code: isize) -> String {
        // LAErrorUserCancel = -2, LAErrorSystemCancel = -4, LAErrorAppCancel = -9.
        match code {
            -2 | -4 | -9 => "authentication was cancelled".into(),
            _ => "authentication was denied".into(),
        }
    }
}
