// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Opt-in native window UX proof, restricted to an explicitly identified
//! cua-spacesd-test-pad process/window. Never captures the desktop or injects
//! input. A caller supplies a real caption drag through the official Cua Driver.
//! Required: CUA_WINDOW_E2E_PID, CUA_WINDOW_E2E_HWND, CUA_WINDOW_E2E_DIR.
//! Optional: CUA_WINDOW_E2E_WAIT_SECS (default 90, maximum 180).
//! The output folder retains a selected-window PNG and facts/events JSON.

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("Windows native desktop required");
    std::process::exit(1);
}
#[cfg(target_os = "windows")]
fn main() {
    if let Err(stage) = windows::run() {
        eprintln!("FAIL {stage}");
        std::process::exit(1);
    }
}
#[cfg(target_os = "windows")]
mod windows {
    use cua_spaces_app_core::notch::drag_trigger::{classify, DragKind};
    use cua_spaces_lib::{
        geometry::{logical_monitor, LogicalRect},
        window_drag::windows_drag_display,
    };
    use cua_teleport::ux::window::{
        capture_thumbnail_png, list_user_windows, DragPhase, Rect, WindowDragMonitor,
    };
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    };
    use std::time::{Duration, Instant};
    type Result<T> = std::result::Result<T, &'static str>;
    #[repr(C)]
    #[derive(Default)]
    struct NativeRect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct MonitorInfo {
        size: u32,
        monitor: NativeRect,
        work: NativeRect,
        flags: u32,
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn SetThreadDpiAwarenessContext(context: isize) -> isize;
        fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
        fn GetWindowRect(hwnd: isize, rect: *mut NativeRect) -> i32;
        fn MonitorFromWindow(hwnd: isize, flags: u32) -> isize;
        fn GetMonitorInfoW(monitor: isize, info: *mut MonitorInfo) -> i32;
    }
    #[link(name = "shcore")]
    unsafe extern "system" {
        fn GetDpiForMonitor(monitor: isize, kind: i32, x: *mut u32, y: *mut u32) -> i32;
    }
    fn env(name: &str) -> Result<String> {
        std::env::var(name)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .ok_or("missing required opt-in fixture environment")
    }
    fn logical(rect: Rect) -> LogicalRect {
        LogicalRect::new(rect.x, rect.y, rect.width, rect.height)
    }
    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 0.5
    }
    pub fn run() -> Result<()> {
        let owner: u32 = env("CUA_WINDOW_E2E_PID")?
            .parse()
            .map_err(|_| "invalid fixture PID")?;
        let hwnd: isize = env("CUA_WINDOW_E2E_HWND")?
            .parse()
            .map_err(|_| "invalid fixture HWND")?;
        let out = PathBuf::from(env("CUA_WINDOW_E2E_DIR")?);
        if !out.is_absolute() || out.file_name().is_none_or(|n| n == ".cua") {
            return Err("output must be an absolute scratch directory");
        }
        std::fs::create_dir_all(&out).map_err(|_| "cannot create evidence directory")?;
        let seconds = std::env::var("CUA_WINDOW_E2E_WAIT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(90)
            .clamp(1, 180);
        // Read-only physical geometry, no process/global DPI configuration.
        let previous = unsafe { SetThreadDpiAwarenessContext(-4) };
        let result = (|| {
            let mut actual = 0;
            unsafe {
                GetWindowThreadProcessId(hwnd, &mut actual);
            }
            if actual != owner {
                return Err("fixture HWND does not belong to requested PID");
            }
            let windows =
                list_user_windows().map_err(|_| "production window enumeration failed")?;
            let selected = windows
                .into_iter()
                .find(|w| {
                    w.pid == owner as i64
                        && w.owner == "cua-spacesd-test-pad"
                        && w.title.starts_with("Cua Windows Port E2E Scratch Pad")
                })
                .ok_or("explicit task fixture not found")?;
            let again = list_user_windows()
                .map_err(|_| "repeated window enumeration failed")?
                .into_iter()
                .find(|w| w.pid == owner as i64 && w.title == selected.title)
                .ok_or("fixture disappeared")?;
            if again.window_id != selected.window_id || selected.window_id as isize == hwnd {
                return Err("opaque fixture ID not stable/distinct from native handle");
            }
            let bounds = selected.bounds.ok_or("fixture bounds missing")?;
            let mut native = NativeRect::default();
            if unsafe { GetWindowRect(hwnd, &mut native) } == 0 {
                return Err("native fixture geometry failed");
            }
            let monitor = unsafe { MonitorFromWindow(hwnd, 2) };
            let (mut dx, mut dy) = (96, 96);
            if unsafe { GetDpiForMonitor(monitor, 0, &mut dx, &mut dy) } < 0 || dx == 0 {
                return Err("native monitor DPI unavailable");
            }
            let scale = dx as f64 / 96.0;
            if !near(bounds.x, native.left as f64 / scale)
                || !near(bounds.y, native.top as f64 / scale)
                || !near(bounds.width, (native.right - native.left) as f64 / scale)
                || !near(bounds.height, (native.bottom - native.top) as f64 / scale)
            {
                return Err("production logical bounds do not match native DPI geometry");
            }
            let mut info = MonitorInfo {
                size: std::mem::size_of::<MonitorInfo>() as u32,
                ..Default::default()
            };
            if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
                return Err("native monitor geometry unavailable");
            }
            let display = windows_drag_display(logical_monitor(
                info.monitor.left,
                info.monitor.top,
                (info.monitor.right - info.monitor.left) as u32,
                (info.monitor.bottom - info.monitor.top) as u32,
                scale,
            ));
            if !near(display.notch.y, display.frame.y)
                || !near(display.prompt.y, display.frame.y)
                || !near(display.expanded.y, display.frame.y)
                || display.notch.height != 34.0
            {
                return Err("Windows top-edge adapter geometry mismatch");
            }
            let png = capture_thumbnail_png(selected.window_id, 320)
                .map_err(|_| "production thumbnail API failed")?
                .ok_or("production window content could not be captured")?;
            let decoder = png::Decoder::new(std::io::Cursor::new(&png));
            let mut reader = decoder
                .read_info()
                .map_err(|_| "thumbnail PNG header invalid")?;
            let mut pixels = vec![0; reader.output_buffer_size()];
            let image = reader
                .next_frame(&mut pixels)
                .map_err(|_| "thumbnail PNG pixels invalid")?;
            if image.color_type != png::ColorType::Rgba
                || image.width == 0
                || image.width > 320
                || image.height == 0
            {
                return Err("thumbnail dimensions or format invalid");
            }
            let palette = [
                [0x18, 0x20, 0x20],
                [0x28, 0x20, 0x18],
                [0x18, 0x20, 0x28],
                [0x18, 0x28, 0x18],
                [0x22, 0x18, 0x28],
                [0x18, 0x22, 0x22],
            ];
            let (mut matched, mut sampled) = (0u64, 0u64);
            // The bottom-right interior catches DPI-unaware apps painting
            // logical content into an oversized physical DIB: a center-only
            // check would miss the unpainted black right/bottom padding.
            for y in image.height / 2..image.height * 9 / 10 {
                for x in image.width / 2..image.width * 9 / 10 {
                    let i = ((y * image.width + x) * 4) as usize;
                    let rgb = [pixels[i], pixels[i + 1], pixels[i + 2]];
                    sampled += 1;
                    if palette.contains(&rgb) {
                        matched += 1;
                    }
                }
            }
            if sampled == 0 || matched * 10 < sampled * 8 {
                return Err("thumbnail lacks the owned fixture's real interior palette");
            }
            std::fs::write(out.join("fixture-thumbnail.png"), &png)
                .map_err(|_| "cannot retain fixture preview")?;
            let (tx, rx) = mpsc::channel();
            let active = Arc::new(AtomicBool::new(false));
            let watching = active.clone();
            let opaque = selected.window_id;
            let monitor = WindowDragMonitor::start(Box::new(move |event| {
                if event.phase == DragPhase::Start {
                    watching.store(
                        event
                            .window
                            .as_ref()
                            .is_some_and(|w| w.pid == owner as i64 && w.window_id == opaque),
                        Ordering::Relaxed,
                    );
                }
                if watching.load(Ordering::Relaxed) {
                    let end = event.phase == DragPhase::End;
                    let _ = tx.send(event);
                    if end {
                        watching.store(false, Ordering::Relaxed);
                    }
                }
            }))
            .map_err(|_| "production passive window monitor failed to start")?;
            println!(
                "READY fixture_pid={owner} fixture_hwnd={hwnd} opaque_id={opaque} scale={scale}"
            );
            let deadline = Instant::now() + Duration::from_secs(seconds);
            let (mut start, mut end) = (None, None);
            let mut moves = 0;
            let mut events = Vec::new();
            while Instant::now() < deadline {
                let Ok(event) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                else {
                    break;
                };
                match event.phase {
                    DragPhase::Start => start = Some(event.clone()),
                    DragPhase::Move => moves += 1,
                    DragPhase::End => end = Some(event.clone()),
                }
                events.push(event);
                if end.is_some() {
                    break;
                }
            }
            monitor.stop();
            std::fs::write(
                out.join("fixture-drag-events.json"),
                serde_json::to_vec_pretty(&events).map_err(|_| "cannot encode events")?,
            )
            .map_err(|_| "cannot retain events")?;
            let start = start.ok_or("no real fixture drag start observed")?;
            let end = end.ok_or("no real fixture drag release observed")?;
            if moves < 2 {
                return Err("real fixture drag lacked continuing move events");
            }
            let first = start.start_frame.ok_or("drag origin missing")?;
            let moved = start.frame.ok_or("drag moved frame missing")?;
            if classify(&logical(first), &logical(moved)) != DragKind::Move {
                return Err("native drag was not a moved window");
            }
            let final_frame = end.frame.ok_or("drag final frame missing")?;
            if near(first.x, final_frame.x) && near(first.y, final_frame.y) {
                return Err("real fixture window did not move");
            }
            let result = json!({"result":"pass","fixturePid":owner,"fixtureHwnd":hwnd,"opaqueId":opaque,"scaleFactor":scale,"logicalBounds":bounds,"topEdge":display,"pngWidth":image.width,"pngHeight":image.height,"interiorPalettePixels":matched,"interiorSampledPixels":sampled,"dragMoveEvents":moves,"dragStartFrame":first,"dragEndFrame":final_frame,"verification":"production Windows window backend, selected task fixture only; no input injected by harness"});
            std::fs::write(
                out.join("result.json"),
                serde_json::to_vec_pretty(&result).map_err(|_| "cannot encode result")?,
            )
            .map_err(|_| "cannot retain result")?;
            println!("PASS native fixture window list, opaque ID/PID, DPI bounds, real-content PNG, top-edge geometry, passive drag start/move/end");
            Ok(())
        })();
        if previous != 0 {
            unsafe {
                SetThreadDpiAwarenessContext(previous);
            }
        }
        result
    }
}
