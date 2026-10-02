# Windows port

This fork builds on the official Tauri 2 app in `apps/cua-spaces`, released
as source in `cua-spaces-v0.3.0` (`bfe38d86af97e91152845103830a2a1516e71705`). The macOS SwiftUI app remains
in `apps/cua-spaces-macos`. Spaces is licensed under **FSL-1.1-MIT**; retain
the upstream license and copyright notices. See [`../../LICENSING.md`](../../LICENSING.md).

The Windows client uses the existing shared Spaces SDK and app core. A real
Space requires a reachable `cua-spacesd` server with the advertised features.
The standalone browser preview uses fixtures and is not evidence of a working
remote desktop. Desktop viewing requires `desktop_stream`; an old server shows
an explicit image-update message.

## Build from source

Use a Visual Studio developer PowerShell with C++ build tools, the Windows SDK,
WebView2, Node 20+, pnpm 10+, the Rust toolchain pinned by `rust-toolchain.toml`,
the `wasm32-unknown-unknown` target, wasm-bindgen 0.2.126 and protoc 29.x.
Keep protoc's `include` directory beside its `bin` directory, or set
`PROTOC_INCLUDE` explicitly. From `apps/cua-spaces` run:

```powershell
./scripts/build-windows.ps1
# Faster development executable (with the real Spaces CLI staged):
./scripts/build-windows.ps1 -DebugBuild -NoBundle
```

The script builds `cua-spaces-cli`, stages `cua-<target>.exe`, builds the
production webview assets and invokes Tauri's existing NSIS/MSI packaging.
It does not run the installer. The Windows port config turns off updater
artifact signing and the official release feed, so an unsigned fork build
does not silently replace itself with an upstream release. All source licenses
and copyright notices remain in place.

## Use on Windows

- Open **New Space → Connect by address** to connect an existing server.
  Use its address and token supplied by its owner. Normal account sign-in
  uses the existing Cua OAuth device flow.
- Open a connected Space's desktop to view and control it. The viewer is
  interactive; picture-in-picture is a viewing surface. **Ctrl+0** shows the
  actual display size and **Ctrl+9** fits it to the window.
- Send selected files or drop files into the selected Space's file area.
  Delivery uses the upstream file transfer and digest verification path.
- **Launch Agent** installs and opens the selected coding agent in a terminal
  in the Space. On Windows this includes Claude Code: sign in there using the
  agent's normal flow. This action does not import this PC's login session.
- **Ctrl+N** opens New Space; **Ctrl+,** opens Settings. Closing the main
  window hides it; the notification-area icon reopens it.
- The app uses the bundled `cua.exe` directly. First launch on Windows does
  not copy it to another directory or edit the account's PATH. To use the CLI
  in a terminal, invoke that executable by its full path.

These describe code paths, not a certification that every server, codec,
agent harness, or operating system has passed Windows end-to-end testing.
Use the fork's build and validation evidence for the tested combinations.

## Current boundaries

- The upstream Windows window-drag monitor, local-window enumeration and
  local-window thumbnails now have native Windows implementations. Enumeration
  checks HWND/PID identity; selected-window thumbnails use bounded PrintWindow
  workers; drag tracking observes native move/size events. The top-edge affordance
  uses physical monitor coordinates. The **Open windows** picker is enabled;
  remote Space window streaming is a separate path. Preview rendering uses the
  selected window's DPI context, so DPI-unaware windows do not gain black margins.
- App installation and sign-in transfer depend on each upstream provider's
  source and target capabilities. Windows browser-session teleport is not
  certified here. No copied browser profile, cookie extraction or weakened
  authentication boundary is needed for the remote-terminal path.
- Sensitive Windows actions use UserConsentVerifier through its desktop HWND
  interface. Only the native Verified result authorizes an action. The test
  machine currently reports DeviceNotPresent in the active interactive session;
  a real Windows Hello prompt and successful approval remain unverified. There
  is no synthetic success, environment-variable bypass, or implicit password
  fallback.
- Windows Keyvault uses a current-user/session named pipe, kernel-reported peer
  PID, retained process/image handles, and encrypted framing. Authenticode file
  evidence does not prove the running process's loaded code: Windows identities
  remain `os_verified=false`. Production first-party trust and OS vault access
  therefore refuse by default. A separately labelled debug fixture tests only
  synthetic passphrase-encrypted data and rejection paths; it does not certify
  Windows account credential transfer or browser session teleport.
- Local sandbox creation depends on already available runtime backends.
  Lume and macOS guest virtualization are macOS-specific. Building the client
  does not install or configure Windows virtualization, containers or a host
  service. QEMU process ownership, WHPX capability discovery and Docker named-pipe
  discovery have Windows implementations; a successful VM/container boot is not
  established by those checks. Volume mounting remains experimental and is off
  by default; no WinFsp, NFS client or virtualization feature is installed here.
- Starting a Windows host daemon, sharing this machine, or installing host
  services is a separate setup action. It has not been authorized or validated
  merely by building this client.
- Launch at login is an optional current-user Windows Run-key setting. This
  fork leaves the first-run Done checkbox **off on Windows**, and does not
  register startup automatically for an older Windows installation without
  a saved choice. Enable it on Done or in Settings when wanted. Other platforms
  retain upstream behavior; testing must not enable it without the user's choice.
- The upstream release workflow gates Windows and Linux publication behind
  `CUA_SPACES_LINUX_WINDOWS`. A source implementation is distinct from an
  officially published Windows installer.

Provider limits and operating-system authorization checks remain authoritative.
Unsupported capabilities must produce a limitation or refusal, never be
reported as successfully ported from a window opening alone.

## Validation and opt-in desktop checks

On 2026-10-02, Windows x64 builds used MSVC 19.44, Windows SDK 10.0.26100,
Node/pnpm, protoc 29.3 and Rust **1.98.1**. The repository's pinned 1.97.1
toolchain was not certified by this local run. Production WASM/TypeScript/Vite
builds and the existing 433 frontend tests passed; subsequent UI changes also
passed their affected existing suites. No new unit tests were added.

The source-built server passed direct wire-v2 H.264 decoding, native foreground
and background input into an owned Win32 fixture, input after a new connection,
and a negative view-only delivery check. The final AppCore smoke runner exited
0 after seven checks: daemon startup, add/list, screenshot/window reads, actual
3 MiB upload/readback with identical SHA-256, desktop video packets, reconnect
to the **same media URL/session** with frames and new typed text, and scratch
cleanup. The fixture recorded Ctrl, Shift and left-button release after the
viewer disconnected. This is media-socket reattachment, not proof of automatic
GUI recovery.

An earlier native window run observed an actual caption drag's start, movement
and release, with stable HWND/PID mapping and 1.25 DPI scaling. The final preview
run passed its enumeration, DPI, geometry and image checks: all 12,544 sampled
bottom-right pixels matched the fixture palette. Its full drag rerun timed out
waiting for the official driver RPC, with no observed drag events; no retry or
process termination was used. Treat that final drag flow as blocked.
The separate Keyvault example passed two-process pipe, framing, encryption and
synthetic-data operations; it explicitly used a fake presence/backend and does
not establish native presence or production credential access.

Optional native examples are `windows_client_smoke`,
`windows_window_smoke` and `windows_keyvault_pipe_smoke`; each source documents
its required scratch directories and opt-in arguments. They are not started by
ordinary tests. Windows MSVC examples embed a common-controls v6 manifest for
their linked native dialog imports.

For a **debug GUI** validation, create a fresh explicit `CUA_HOME`, then an
existing child directory and set `CUA_SPACES_APP_E2E_AGENT_HOME` to that child.
All in-app agent setup commands then use the existing SDK's hermetic HostEnv:
no inherited agent directories, PATH search, or owner CLI execution. An invalid
child path fails startup instead of falling back to real account configuration.
This flag changes only configuration destinations; it does not change native
authorization, broker trust, or Windows account identity. Explicit Windows homes
also isolate UI/webview data and onboarding state. Do not redirect a real browser
profile or install agents, startup entries or host services during validation.

Socket reattachment and UI automatic recovery are separate evidence. The
client example exercises the former; VM boot, volume mounting, real account
credential transfer and native Hello approval require additional verification.

The final isolated GUI rendered its welcome page in the native WebView2 window,
and its selected-window H.264 capture decoded 13 frames without errors.
Its JS, CSS and WASM loaded successfully, with no reported runtime errors. A
black selected-window capture from an occluded instance was not evidence of a
startup failure. A bounded Get started click returned `delivered:false` with
`no acknowledgement`, and the page did not advance. Complete connect-by-address
interaction inside the GUI and automatic recovery are not certified by this
run. A stalled legacy foreground action also holds the shared driver action
coordinator, causing later activation calls to wait; the precise blocked native
call was not established. The working owned-fixture media tests do not certify
input into every WebView or window.

| Capability | Windows evidence / status |
| --- | --- |
| Native Tauri/WebView2 client | Compiled and actual welcome page rendered |
| Direct server connection and registry | Actual AppCore add/list/cleanup passed |
| Files | Actual 3 MiB upload, SHA-256 and readback passed |
| H.264 and native input | Decoding, background/foreground typing and view-only refusal passed |
| Same-session socket reconnect | Frames, new input and held-input release passed |
| Selected-window previews | Final DPI/pixel verification passed |
| Native caption drag | Earlier run passed; final run blocked on official driver RPC |
| Full GUI connection / automatic reconnect | Not certified |
| Keyvault IPC | Two-process synthetic fixture passed; production credential access refuses by default |
| Windows Hello approval | DeviceNotPresent; successful native approval unverified |
| Browser account/session teleport | Not certified; no profile/cookie copying |
| VM/container boot and volume mounts | Not certified; no host installation or feature changes |
| macOS guest / Lume | Requires macOS; not implemented as a Windows host feature |

This is an unsigned development port, not an official Windows release. No
installer or host service was installed to perform these checks.
