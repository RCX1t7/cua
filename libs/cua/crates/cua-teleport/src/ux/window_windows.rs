// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Current-user desktop windows. WinEvent hooks are out of context: no DLL
//! injection, input hook, elevated token, or cross-account access. Previews
//! ask only the selected window to paint itself; never capture the desktop.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc, Arc, Mutex, OnceLock,
};
use std::time::Duration;

use super::window::{
    MouseEvent, Rect, WindowDragEvent, WindowDragTracker, WindowInfo, WindowSource,
};
use super::UxError;

type Handle = isize;
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct NativeRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Point {
    x: i32,
    y: i32,
}
#[repr(C)]
#[derive(Default)]
struct Message {
    hwnd: Handle,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: Point,
    private: u32,
}
#[repr(C)]
struct BitmapHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    xppm: i32,
    yppm: i32,
    used: u32,
    important: u32,
}
type EventProc = unsafe extern "system" fn(Handle, u32, Handle, i32, i32, u32, u32);

#[link(name = "user32")]
unsafe extern "system" {
    fn SetThreadDpiAwarenessContext(context: Handle) -> Handle;
    fn GetWindowDpiAwarenessContext(hwnd: Handle) -> Handle;
    fn EnumWindows(callback: unsafe extern "system" fn(Handle, isize) -> i32, data: isize) -> i32;
    fn IsWindow(hwnd: Handle) -> i32;
    fn IsWindowVisible(hwnd: Handle) -> i32;
    fn IsIconic(hwnd: Handle) -> i32;
    fn IsHungAppWindow(hwnd: Handle) -> i32;
    fn GetWindowTextW(hwnd: Handle, text: *mut u16, count: i32) -> i32;
    fn GetWindowRect(hwnd: Handle, rect: *mut NativeRect) -> i32;
    fn GetWindowThreadProcessId(hwnd: Handle, pid: *mut u32) -> u32;
    fn GetCursorPos(point: *mut Point) -> i32;
    fn GetAsyncKeyState(key: i32) -> i16;
    fn MonitorFromWindow(hwnd: Handle, flags: u32) -> Handle;
    fn SetWinEventHook(
        min: u32,
        max: u32,
        module: Handle,
        callback: EventProc,
        pid: u32,
        thread: u32,
        flags: u32,
    ) -> Handle;
    fn UnhookWinEvent(hook: Handle) -> i32;
    fn PeekMessageW(msg: *mut Message, hwnd: Handle, min: u32, max: u32, remove: u32) -> i32;
    fn TranslateMessage(msg: *const Message) -> i32;
    fn DispatchMessageW(msg: *const Message) -> isize;
    fn GetDC(hwnd: Handle) -> Handle;
    fn ReleaseDC(hwnd: Handle, dc: Handle) -> i32;
    fn PrintWindow(hwnd: Handle, dc: Handle, flags: u32) -> i32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
    fn QueryFullProcessImageNameW(
        process: Handle,
        flags: u32,
        name: *mut u16,
        size: *mut u32,
    ) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
}
#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmGetWindowAttribute(hwnd: Handle, attribute: u32, value: *mut c_void, size: u32) -> i32;
}
#[link(name = "shcore")]
unsafe extern "system" {
    fn GetDpiForMonitor(monitor: Handle, kind: i32, x: *mut u32, y: *mut u32) -> i32;
}
#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateCompatibleDC(dc: Handle) -> Handle;
    fn CreateDIBSection(
        dc: Handle,
        info: *const BitmapHeader,
        usage: u32,
        bits: *mut *mut c_void,
        section: Handle,
        offset: u32,
    ) -> Handle;
    fn SelectObject(dc: Handle, object: Handle) -> Handle;
    fn DeleteObject(object: Handle) -> i32;
    fn DeleteDC(dc: Handle) -> i32;
}

// Read physical coordinates consistently even in a DPI-unaware CLI client.
// This changes only this adapter thread, and restores its prior context.
struct DpiContext(Handle);
impl DpiContext {
    fn enter() -> Self {
        Self(unsafe { SetThreadDpiAwarenessContext(-4) })
    }
    fn for_window(hwnd: Handle) -> Self {
        // PrintWindow executes the selected app's painter. An unaware app
        // paints in its virtualized dimensions; using our physical dimensions
        // would leave unpainted right/bottom margins on scaled monitors.
        let context = unsafe { GetWindowDpiAwarenessContext(hwnd) };
        Self(unsafe { SetThreadDpiAwarenessContext(if context == 0 { -4 } else { context }) })
    }
}
impl Drop for DpiContext {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe {
                SetThreadDpiAwarenessContext(self.0);
            }
        }
    }
}

// Public IDs are opaque u32s, not truncated 64-bit HWNDs. Remember the owner
// and reject stale IDs if the OS reuses a handle for another process.
#[derive(Default)]
struct Ids {
    next: u32,
    entries: HashMap<u32, (Handle, u32)>,
}
fn ids() -> &'static Mutex<Ids> {
    static IDS: OnceLock<Mutex<Ids>> = OnceLock::new();
    IDS.get_or_init(|| Mutex::new(Ids::default()))
}
fn pid(hwnd: Handle) -> u32 {
    let mut value = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut value);
    }
    value
}
fn public_id(hwnd: Handle, owner: u32) -> Option<u32> {
    let mut store = ids().lock().unwrap_or_else(|p| p.into_inner());
    if let Some((&id, _)) = store
        .entries
        .iter()
        .find(|(_, value)| **value == (hwnd, owner))
    {
        return Some(id);
    }
    store.next = store.next.checked_add(1)?;
    let id = store.next;
    store.entries.insert(id, (hwnd, owner));
    Some(id)
}
fn resolve(id: u32) -> Option<Handle> {
    let (hwnd, owner) = *ids()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .entries
        .get(&id)?;
    (unsafe { IsWindow(hwnd) != 0 } && pid(hwnd) == owner).then_some(hwnd)
}
fn scale(hwnd: Handle) -> f64 {
    let (mut x, mut y) = (96, 96);
    if unsafe { GetDpiForMonitor(MonitorFromWindow(hwnd, 2), 0, &mut x, &mut y) } >= 0 && x > 0 {
        x as f64 / 96.0
    } else {
        1.0
    }
}
fn frame(hwnd: Handle) -> Option<NativeRect> {
    let mut rect = NativeRect::default();
    (unsafe { GetWindowRect(hwnd, &mut rect) } != 0).then_some(rect)
}
fn process_name(owner: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(0x1000, 0, owner); // QUERY_LIMITED_INFORMATION
        if process == 0 {
            return None;
        }
        let mut buffer = vec![0; 32768];
        let mut length = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut length);
        CloseHandle(process);
        if ok == 0 {
            return None;
        }
        let path = String::from_utf16_lossy(&buffer[..length as usize]);
        let name = std::path::Path::new(&path)
            .file_stem()?
            .to_string_lossy()
            .into_owned();
        Some(match name.to_ascii_lowercase().as_str() {
            "code" => "Visual Studio Code".into(),
            "chrome" => "Google Chrome".into(),
            "msedge" => "Microsoft Edge".into(),
            "firefox" => "Firefox".into(),
            _ => name,
        })
    }
}
fn info(hwnd: Handle) -> Option<WindowInfo> {
    unsafe {
        if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
            return None;
        }
        let mut cloaked: u32 = 0;
        if DwmGetWindowAttribute(hwnd, 14, &mut cloaked as *mut _ as *mut c_void, 4) >= 0
            && cloaked != 0
        {
            return None;
        }
        let owner = pid(hwnd);
        if owner == 0 {
            return None;
        }
        let native = frame(hwnd)?;
        let dpi = scale(hwnd);
        let mut text = [0u16; 4096];
        let length = GetWindowTextW(hwnd, text.as_mut_ptr(), text.len() as i32).max(0) as usize;
        Some(WindowInfo {
            window_id: public_id(hwnd, owner)?,
            pid: owner as i64,
            owner: process_name(owner).unwrap_or_else(|| format!("Process {owner}")),
            title: String::from_utf16_lossy(&text[..length]),
            layer: 0,
            alpha: 1.0,
            bounds: Some(Rect {
                x: native.left as f64 / dpi,
                y: native.top as f64 / dpi,
                width: (native.right - native.left) as f64 / dpi,
                height: (native.bottom - native.top) as f64 / dpi,
            }),
            visible: true,
            // A Windows executable is not a macOS bundle or a session source.
            // Classification uses the actual process name; never export it.
            bundle_path: None,
        })
    }
}
pub fn all_windows() -> Vec<WindowInfo> {
    let _dpi = DpiContext::enter();
    unsafe extern "system" fn collect(hwnd: Handle, data: isize) -> i32 {
        if let Some(window) = info(hwnd) {
            unsafe {
                (&mut *(data as *mut Vec<WindowInfo>)).push(window);
            }
        }
        1
    }
    let mut windows = Vec::new();
    unsafe {
        EnumWindows(collect, &mut windows as *mut _ as isize);
    }
    // Drop closed/reused entries to keep long-running desktop sessions bounded.
    ids()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .entries
        .retain(|_, (hwnd, owner)| unsafe { IsWindow(*hwnd) != 0 } && pid(*hwnd) == *owner);
    windows
}

// PrintWindow is synchronous and can block inside the selected app. Each
// worker owns its GDI resources until the OS call returns; a timeout never
// frees an in-use bitmap/DC or kills a thread. Bound outstanding workers so
// unsupported/hung windows cannot accumulate unlimited captures.
static CAPTURES: AtomicUsize = AtomicUsize::new(0);
struct CapturePermit;
impl Drop for CapturePermit {
    fn drop(&mut self) {
        CAPTURES.fetch_sub(1, Ordering::AcqRel);
    }
}
pub fn capture(id: u32, max_width: usize) -> Option<Vec<u8>> {
    let hwnd = resolve(id)?;
    let owner = pid(hwnd);
    info(hwnd)?;
    if unsafe { IsHungAppWindow(hwnd) } != 0 {
        return None;
    }
    CAPTURES
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
            (active < 2).then_some(active + 1)
        })
        .ok()?;
    let permit = CapturePermit;
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("cua-window-preview".into())
        .spawn(move || {
            let _permit = permit;
            let png = if pid(hwnd) == owner {
                capture_window(hwnd, max_width)
            } else {
                None
            };
            let _ = tx.send(png);
        })
        .ok()?;
    rx.recv_timeout(Duration::from_secs(1)).ok().flatten()
}
fn capture_window(hwnd: Handle, max_width: usize) -> Option<Vec<u8>> {
    let _dpi = DpiContext::for_window(hwnd);
    info(hwnd)?;
    if unsafe { IsHungAppWindow(hwnd) } != 0 {
        return None;
    }
    let rect = frame(hwnd)?;
    let (width, height) = (
        rect.right.checked_sub(rect.left)?,
        rect.bottom.checked_sub(rect.top)?,
    );
    if width <= 0 || height <= 0 {
        return None;
    }
    let length = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)?;
    if length > 128 * 1024 * 1024 {
        return None;
    }
    unsafe {
        let source = GetDC(hwnd);
        if source == 0 {
            return None;
        }
        let dc = CreateCompatibleDC(source);
        let header = BitmapHeader {
            size: 40,
            width,
            height: -height,
            planes: 1,
            bit_count: 32,
            compression: 0,
            size_image: 0,
            xppm: 0,
            yppm: 0,
            used: 0,
            important: 0,
        };
        let mut bits = std::ptr::null_mut();
        let bitmap = if dc != 0 {
            CreateDIBSection(source, &header, 0, &mut bits, 0, 0)
        } else {
            0
        };
        let result = if bitmap != 0 && !bits.is_null() {
            let old = SelectObject(dc, bitmap);
            std::ptr::write_bytes(bits, 0, length);
            // PW_RENDERFULLCONTENT asks DWM/GDI to render this window,
            // including its client content. Never fall back to desktop pixels.
            let painted = PrintWindow(hwnd, dc, 2);
            let pixels = std::slice::from_raw_parts_mut(bits as *mut u8, length);
            let png = if painted != 0
                && pixels
                    .chunks_exact(4)
                    .any(|p| p[0] != 0 || p[1] != 0 || p[2] != 0)
            {
                for pixel in pixels.chunks_exact_mut(4) {
                    pixel[3] = 255;
                }
                super::window::thumbnail_rgba(
                    pixels,
                    width as usize,
                    height as usize,
                    width as usize * 4,
                    max_width,
                )
                .and_then(|(rgba, w, h)| super::window::encode_png(&rgba, w, h))
            } else {
                None
            };
            SelectObject(dc, old);
            png
        } else {
            None
        };
        if bitmap != 0 {
            DeleteObject(bitmap);
        }
        if dc != 0 {
            DeleteDC(dc);
        }
        ReleaseDC(hwnd, source);
        result
    }
}

const MOVE_START: u32 = 0x000A;
const MOVE_END: u32 = 0x000B;
thread_local! { static EVENTS: RefCell<Vec<(u32, Handle)>> = const { RefCell::new(Vec::new()) }; }
unsafe extern "system" fn event_callback(
    _: Handle,
    event: u32,
    hwnd: Handle,
    _: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    // Do not do blocking reads or call the renderer inside an OS callback.
    EVENTS.with(|events| {
        if let Ok(mut queue) = events.try_borrow_mut() {
            queue.push((event, hwnd));
        }
    });
}
struct Source(Handle);
struct ActiveDrag {
    hwnd: Handle,
    tracker: WindowDragTracker<Source>,
}
impl WindowSource for Source {
    fn windows(&self) -> Vec<WindowInfo> {
        info(self.0).into_iter().collect()
    }
}
fn cursor(hwnd: Handle) -> Option<(f64, f64)> {
    let mut point = Point::default();
    let dpi = scale(hwnd);
    (unsafe { GetCursorPos(&mut point) } != 0)
        .then_some((point.x as f64 / dpi, point.y as f64 / dpi))
}
pub struct Monitor {
    stopped: Arc<AtomicBool>,
}
impl Monitor {
    pub fn start(on_event: Box<dyn Fn(WindowDragEvent) + Send + 'static>) -> Result<Self, UxError> {
        let stopped = Arc::new(AtomicBool::new(false));
        let flag = stopped.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("cua-window-drag".into())
            .spawn(move || {
                let _dpi = DpiContext::enter();
                // OUTOFCONTEXT | SKIPOWNPROCESS: current desktop only, passive.
                let hook =
                    unsafe { SetWinEventHook(MOVE_START, MOVE_END, 0, event_callback, 0, 0, 2) };
                if hook == 0 {
                    let _ = tx.send(false);
                    return;
                }
                let _ = tx.send(true);
                let mut tracker: Option<ActiveDrag> = None;
                while !flag.load(Ordering::Relaxed) {
                    let mut msg = Message::default();
                    unsafe {
                        while PeekMessageW(&mut msg, 0, 0, 0, 1) != 0 {
                            TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                        }
                    }
                    let events = EVENTS.with(|queue| std::mem::take(&mut *queue.borrow_mut()));
                    for (event, hwnd) in events {
                        if event == MOVE_START && unsafe { GetAsyncKeyState(1) } < 0 {
                            let mut next =
                                WindowDragTracker::new(Source(hwnd), std::process::id() as i64);
                            if let Some((x, y)) = cursor(hwnd) {
                                next.handle(MouseEvent::Down, x, y);
                            }
                            tracker = Some(ActiveDrag {
                                hwnd,
                                tracker: next,
                            });
                        } else if event == MOVE_END
                            && tracker.as_ref().is_some_and(|t| t.hwnd == hwnd)
                        {
                            if let Some(mut previous) = tracker.take() {
                                if let Some((x, y)) = cursor(hwnd) {
                                    if let Some(e) =
                                        previous.tracker.handle(MouseEvent::Dragged, x, y)
                                    {
                                        on_event(e);
                                    }
                                    if let Some(e) = previous.tracker.handle(MouseEvent::Up, x, y) {
                                        on_event(e);
                                    }
                                }
                            }
                        }
                    }
                    if let Some(active) = tracker.as_mut() {
                        if let Some((x, y)) = cursor(active.hwnd) {
                            if let Some(e) = active.tracker.handle(MouseEvent::Dragged, x, y) {
                                on_event(e);
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(16));
                }
                unsafe {
                    UnhookWinEvent(hook);
                }
            })
            .map_err(|e| UxError::Io(e.to_string()))?;
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(true) => Ok(Self { stopped }),
            _ => {
                stopped.store(true, Ordering::Relaxed);
                Err(UxError::Io(
                    "Windows desktop move/size observation could not start".into(),
                ))
            }
        }
    }
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
    }
}
impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop();
    }
}
