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

An earlier native caption-drag rerun timed out against the old retained server.
The repaired server's fresh run passed: the passive native harness observed
drag start, 29 continuing move events and release, with the window's logical
origin changing from (26.4, 26.4) to (128.8, 103.2). HWND/PID identity and 1.25
DPI scaling remained stable. All 12,544 sampled bottom-right preview pixels
matched the fixture palette. No existing process was stopped or unlocked.
Later that fixture HWND became invalid while its process remained alive;
post-drag capture and that lifecycle transition are not established by the
persisted drag proof. No further input or retry was sent to the stale handle.
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

The initial GUI input run stalled without an acknowledgement. On 2026-10-03,
the repaired source passed a fresh isolated WebView2 input run: Get started
received a delivery acknowledgement in 34 ms and advanced to Sign in. A new
Skip click after contention received an acknowledgement in 8 ms and advanced
to Agents. Actual Skip, Continue, Access other machines and Start using controls
completed onboarding into the normal Spaces view. The Done startup checkbox
remained off, and no agents, host services or real accounts were set up.
JS, CSS and WASM loaded successfully, and selected-window H.264
decoded without errors. These checks use the official driver/media protocol;
DOM inspection only verifies the resulting page state.

Foreground mouse delivery no longer joins another window's input queue or
changes its styles/z-order. It verifies the exact PID/window and foreground
before injection, uses physical coordinates, and releases owned button edges
on failures. Admission to the desktop coordinator and pointer activity lock is
bounded to five seconds. Media input has a bounded queue and rejects expired
work before native dispatch; an already executing native call retains its
guard until it actually completes. These deadlines do not cancel or unlock a
running Windows call.

A real ten-second drag into an owned fixture retained its native button edge
for 10,062 ms and released normally. Concurrent core activation returned an
explicit `input_busy` refusal; a GUI Skip input returned
`delivered:false / rate_limited / input_busy` after 5,149 ms. The GUI remained
on Sign in after the owner completed and a further observation period: the
refused click did not execute later. A new click then succeeded. A separate
500 ms drag produced native DOWN/UP 515 ms apart at different endpoints, with
buttons and modifiers clear before and after. The fixture does not log every
mouse-move event, and driver acceptance alone is not application-effect proof.

The exact blocked call in the older retained process was not established.
A separate owned-window probe demonstrated cross-thread synchronous style and
position calls blocking for 1,203 ms, but that mechanism probe is not a stack
trace of the old process. Background legacy style-delivery paths are outside
this foreground repair. Complete connect-by-address interaction inside the
GUI subsequently passed: normal controls opened the address form, entered the
loopback fixture address and its synthetic token, and submitted Add Space.
The form closed, a machine row appeared, the portal changed from zero to one
Space, and the connected detail created its desktop canvas.
The frontend reported `streaming` and `frameRendered=true`; the latter is set
by the actual rendered-frame callback, not by creating a canvas element.
The token was
typed through the official MCP/driver from memory, with no clipboard, token
command-line argument, logged value or real-account setup. No default desktop
canvas screenshot was saved. Automatic GUI recovery and control through every
guest window still require separate validation.

| Capability | Windows evidence / status |
| --- | --- |
| Native Tauri/WebView2 client | Compiled; actual onboarding completed with acknowledged input |
| Direct server connection and registry | Actual GUI address/token submission connected; AppCore add/list/cleanup passed |
| Files | Actual 3 MiB upload, SHA-256 and readback passed |
| H.264 and native input | Decoding, background/foreground typing and view-only refusal passed |
| Same-session socket reconnect | Frames, new input and held-input release passed |
| Selected-window previews | Final DPI/pixel verification passed |
| Native caption drag | Fresh repaired run passed start, 29 moves, release and changed bounds |
| GUI connection / automatic reconnect | Address-form connection passed; automatic recovery not certified |
| Keyvault IPC | Two-process synthetic fixture passed; production credential access refuses by default |
| Windows Hello approval | DeviceNotPresent; successful native approval unverified |
| Browser account/session teleport | Not certified; no profile/cookie copying |
| VM/container boot and volume mounts | Not certified; no host installation or feature changes |
| macOS guest / Lume | Requires macOS; not implemented as a Windows host feature |

This is an unsigned development port, not an official Windows release. No
installer or host service was installed to perform these checks.

## User workflow completion (2026-10-03)

The current feature batch fixes stale connection indicators, displays real
input/stream refusals in single- and multi-window viewers, and adds native
folder selection plus verified selected-item transfer progress. Failed batches
retain completed receipts; an explicit retry sends only failed and remaining
items. An interrupted folder may have partial writes, so the UI does not claim
rollback or invent byte progress. Transfer state survives changing tabs and
uses a synchronous per-Space lock to prevent duplicate submissions.

Saved addresses and desktop-media connections are separate: a successful
cached client no longer proves server reachability, and manually disconnecting
a viewer does not delete its Space. A desktop is labelled connected only
after an actual frame callback. Source-level fixes and a successful build do
not certify the complete user workflow; final GUI end-to-end acceptance remains
pending until an approved restricted Space is available.

The earlier inventory claim that macOS hides the source window after a
successful teleport was incorrect. The released macOS flow retains the app
entry, not a source HWND, and leaves the original window unchanged. The Tauri
drag ghost is likewise additive. This port does not introduce a Windows-only
minimize/hide action to satisfy a feature the macOS release does not have.

No unit tests are written or run for this batch, including existing suites.
Only compilation is used to produce the candidate; final acceptance follows
the real GUI workflow. The three user-approved wide-authority test services
were stopped and will not be restarted by this work. Their credential files
are retained; no new token use, host setup, VM installation, volume mount, or
system security/network change is authorized by the feature fixes.
