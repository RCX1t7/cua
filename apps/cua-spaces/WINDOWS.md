# Windows port

This fork builds on the official Tauri 2 app in `apps/cua-spaces`, released
as source in `cua-spaces-v0.3.0` (`bfe38d86a`). The macOS SwiftUI app remains
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
  local-window thumbnails are not implemented. The **Open windows** picker
  tab is disabled on Windows, and the file area explains the restriction.
  Listing and streaming windows from a remote Space remain separate features.
- App installation and sign-in transfer depend on each upstream provider's
  source and target capabilities. Windows browser-session teleport is not
  certified here. No copied browser profile, cookie extraction or weakened
  authentication boundary is needed for the remote-terminal path.
- Touch ID authorization is macOS-only. This port does not bypass the native
  authorization gate for sensitive settings or substitute weaker consent.
- Local sandbox creation depends on already available runtime backends.
  Lume and macOS guest virtualization are macOS-specific. Building the client
  does not install or configure Windows virtualization, containers or a host
  service. Some shared-core labels still say **This Mac** for local placement.
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
