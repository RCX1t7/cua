//! Host introspection and small process/network helpers shared by backends.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};

use crate::error::{Result, VmmError};
use crate::types::Arch;

/// Host operating system.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostOs {
    Macos,
    Linux,
    Windows,
}

impl HostOs {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            HostOs::Macos
        } else if cfg!(target_os = "windows") {
            HostOs::Windows
        } else {
            HostOs::Linux
        }
    }
}

/// `~/.cua` (or `$CUA_HOME`).
pub fn cua_home() -> PathBuf {
    if let Some(h) = std::env::var_os("CUA_HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(h);
    }
    home_dir().join(".cua")
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Locate an executable on `PATH`, then in well-known install prefixes that
/// are often missing from GUI/ssh PATHs (Homebrew, MacPorts, ~/.local/bin).
pub fn which(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) && !name.ends_with(".exe") {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    for extra in [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/opt/local/bin",
        "/usr/bin",
        "/usr/sbin",
    ] {
        dirs.push(PathBuf::from(extra));
    }
    dirs.push(home_dir().join(".local/bin"));
    dirs.push(cua_home().join("bin"));
    dirs.into_iter()
        .map(|d| d.join(&exe))
        .find(|p| is_executable(p))
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        p.metadata()
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

/// A free loopback TCP port chosen by the OS.
/// Writes `data` to `path` readable by the owner only (`0600` on Unix):
/// written to a sibling temp file created with that mode, then renamed, so
/// the contents are never readable by others, even briefly.
pub fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let tmp = path.with_extension(format!(
        "{}tmp",
        path.extension()
            .map(|e| format!("{}.", e.to_string_lossy()))
            .unwrap_or_default()
    ));
    let _ = std::fs::remove_file(&tmp);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    f.write_all(data)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)
}

pub fn free_port() -> Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// `n` distinct free ports (held open while picking so they don't collide).
pub fn free_ports(n: usize) -> Result<Vec<u16>> {
    let listeners: Vec<TcpListener> = (0..n)
        .map(|_| TcpListener::bind("127.0.0.1:0"))
        .collect::<std::io::Result<_>>()?;
    listeners
        .iter()
        .map(|l| Ok(l.local_addr()?.port()))
        .collect()
}

/// A VNC display number `N` whose port `5900+N` is free on loopback.
///
/// QEMU exits with "Failed to find an available port" if the display is
/// taken, so a fixed `:0` breaks the second concurrent sandbox.
pub fn free_vnc_display(start: u16) -> Option<u16> {
    (start..start.saturating_add(100)).find(|d| TcpListener::bind(("127.0.0.1", 5900 + d)).is_ok())
}

/// Run a host command to completion and return stdout; errors carry stderr.
pub async fn run(program: impl AsRef<std::ffi::OsStr>, args: &[&str]) -> Result<String> {
    let program = program.as_ref();
    let out = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                VmmError::missing(program.to_string_lossy(), "not found on PATH")
            } else {
                e.into()
            }
        })?;
    if !out.status.success() {
        return Err(VmmError::Command {
            cmd: format!("{} {}", program.to_string_lossy(), args.join(" ")),
            code: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Whether hardware virtualisation is usable for a guest of `guest` arch, and
/// which QEMU accelerator to use.
pub fn qemu_accel(guest: Arch) -> &'static str {
    let host = Arch::host();
    match HostOs::current() {
        // HVF only virtualises the host's own architecture.
        HostOs::Macos if host == guest => "hvf",
        HostOs::Linux if host == guest && kvm_usable() => "kvm",
        HostOs::Windows if host == guest && whpx_usable() => "whpx",
        _ => "tcg",
    }
}

/// Whether Windows Hypervisor Platform reports a running hypervisor.
/// Loading the system DLL dynamically also supports hosts without WHP installed.
pub fn whpx_usable() -> bool {
    #[cfg(windows)]
    {
        use std::ffi::c_void;

        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
            fn GetProcAddress(
                module: *mut c_void,
                name: *const u8,
            ) -> Option<unsafe extern "system" fn() -> isize>;
            fn FreeLibrary(module: *mut c_void) -> i32;
        }
        struct Library(*mut c_void);
        impl Drop for Library {
            fn drop(&mut self) {
                unsafe { FreeLibrary(self.0) };
            }
        }
        type GetCapability = unsafe extern "system" fn(u32, *mut c_void, u32, *mut u32) -> i32;

        // LOAD_LIBRARY_SEARCH_SYSTEM32 prevents task/PATH DLL substitution.
        let name: Vec<u16> = "WinHvPlatform.dll\0".encode_utf16().collect();
        let module = unsafe { LoadLibraryExW(name.as_ptr(), std::ptr::null_mut(), 0x0000_0800) };
        if module.is_null() {
            return false;
        }
        let module = Library(module);
        let Some(function) =
            (unsafe { GetProcAddress(module.0, c"WHvGetCapability".as_ptr().cast()) })
        else {
            return false;
        };
        // Signature and capability code are defined by the Windows SDK's WinHvPlatform.h.
        let get_capability: GetCapability = unsafe { std::mem::transmute(function) };
        let mut present: i32 = 0; // WHV_CAPABILITY.HypervisorPresent is a Windows BOOL.
        let mut written: u32 = 0;
        let result = unsafe {
            get_capability(
                0, // WHvCapabilityCodeHypervisorPresent
                (&mut present as *mut i32).cast(),
                std::mem::size_of_val(&present) as u32,
                &mut written,
            )
        };
        result >= 0 && written >= std::mem::size_of_val(&present) as u32 && present != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// `/dev/kvm` exists and is openable read/write.
pub fn kvm_usable() -> bool {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/kvm")
        .is_ok()
}

/// Unix seconds.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Whether a process with this pid is alive.
pub fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // kill(pid, 0) via the `kill` utility keeps us free of libc bindings.
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        windows_process::Process::open(pid, false)
            .and_then(|process| process.is_alive())
            .unwrap_or(false)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

/// Identity of one Windows process instance, not merely a reusable PID.
#[cfg(windows)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub creation_time_100ns: u64,
    pub executable: PathBuf,
}

#[cfg(windows)]
pub fn process_identity(pid: u32) -> std::io::Result<ProcessIdentity> {
    windows_process::Process::open(pid, false)?.identity()
}

#[cfg(windows)]
pub fn process_matches(pid: u32, expected: &ProcessIdentity) -> bool {
    windows_process::Process::open(pid, false)
        .and_then(|process| Ok(process.identity()? == *expected && process.is_alive()?))
        .unwrap_or(false)
}

/// Terminate only the recorded process instance. Validation and termination
/// use the same handle, so PID reuse cannot redirect the termination.
#[cfg(windows)]
pub fn terminate_process(pid: u32, expected: &ProcessIdentity) -> std::io::Result<()> {
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::{TerminateProcess, WaitForSingleObject};

    let process = windows_process::Process::open(pid, true)?;
    if process.identity()? != *expected {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "refusing to terminate a process whose creation time or executable differs",
        ));
    }
    if !process.is_alive()? {
        return Ok(());
    }
    // SAFETY: this owned handle has PROCESS_TERMINATE rights and was checked
    // against the persisted identity. No second PID lookup occurs here.
    if unsafe { TerminateProcess(process.handle, 1) } == 0 {
        let error = std::io::Error::last_os_error();
        if process.is_alive()? {
            return Err(error);
        }
    }
    // TerminateProcess is asynchronous; do not report success until it exits.
    if unsafe { WaitForSingleObject(process.handle, 5_000) } != WAIT_OBJECT_0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "the recorded Windows process did not exit after termination",
        ));
    }
    Ok(())
}

#[cfg(windows)]
mod windows_process {
    use std::os::windows::ffi::OsStringExt;

    use windows_sys::Win32::Foundation::{
        CloseHandle, FILETIME, HANDLE, STILL_ACTIVE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, QueryFullProcessImageNameW, WaitForSingleObject,
    };

    pub(super) struct Process {
        pub(super) handle: HANDLE,
    }

    impl Process {
        pub(super) fn open(pid: u32, terminate: bool) -> std::io::Result<Self> {
            let rights = PROCESS_QUERY_LIMITED_INFORMATION
                | PROCESS_SYNCHRONIZE
                | if terminate { PROCESS_TERMINATE } else { 0 };
            // SAFETY: no pointers are supplied, and handle inheritance is off.
            let handle = unsafe { OpenProcess(rights, 0, pid) };
            if handle.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            Ok(Self { handle })
        }

        pub(super) fn is_alive(&self) -> std::io::Result<bool> {
            // A signalled process is dead even if its exit code is 259
            // (STILL_ACTIVE), which GetExitCodeProcess alone cannot distinguish.
            match unsafe { WaitForSingleObject(self.handle, 0) } {
                WAIT_OBJECT_0 => Ok(false),
                WAIT_TIMEOUT => {
                    let mut code = 0;
                    if unsafe { GetExitCodeProcess(self.handle, &mut code) } == 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(code == STILL_ACTIVE as u32)
                }
                _ => Err(std::io::Error::last_os_error()),
            }
        }

        pub(super) fn identity(&self) -> std::io::Result<super::ProcessIdentity> {
            let mut creation = FILETIME::default();
            let mut exit = FILETIME::default();
            let mut kernel = FILETIME::default();
            let mut user = FILETIME::default();
            // SAFETY: all output pointers refer to initialized FILETIME values.
            if unsafe {
                GetProcessTimes(
                    self.handle,
                    &mut creation,
                    &mut exit,
                    &mut kernel,
                    &mut user,
                )
            } == 0
            {
                return Err(std::io::Error::last_os_error());
            }
            let mut image = vec![0u16; 32_768];
            let mut length = image.len() as u32;
            // SAFETY: the buffer holds `length` UTF-16 code units.
            if unsafe {
                QueryFullProcessImageNameW(self.handle, 0, image.as_mut_ptr(), &mut length)
            } == 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(super::ProcessIdentity {
                creation_time_100ns: (u64::from(creation.dwHighDateTime) << 32)
                    | u64::from(creation.dwLowDateTime),
                executable: std::ffi::OsString::from_wide(&image[..length as usize]).into(),
            })
        }
    }

    impl Drop for Process {
        fn drop(&mut self) {
            // SAFETY: the handle was opened by this value and is closed once.
            unsafe { CloseHandle(self.handle) };
        }
    }
}

/// Send a signal (`TERM`, `KILL`, ...) to a pid.
pub fn signal(pid: u32, sig: &str) {
    let _ = std::process::Command::new("kill")
        .args([&format!("-{sig}"), &pid.to_string()])
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_ports_are_distinct() {
        let p = free_ports(4).unwrap();
        let mut d = p.clone();
        d.dedup();
        assert_eq!(p.len(), 4);
        assert!(p.iter().all(|x| *x > 0));
    }

    #[test]
    fn accel_is_tcg_for_foreign_arch() {
        let foreign = match Arch::host() {
            Arch::X86_64 => Arch::Aarch64,
            Arch::Aarch64 => Arch::X86_64,
        };
        assert_eq!(qemu_accel(foreign), "tcg");
        #[cfg(target_os = "macos")]
        assert_eq!(qemu_accel(Arch::host()), "hvf");
    }

    #[test]
    fn which_finds_sh() {
        assert!(which("sh").is_some());
        assert!(which("definitely-not-a-binary-cua").is_none());
    }
}
