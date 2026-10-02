// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Stateful Windows delivery for the shared interactive-input contract.
//! Background sessions post to an exact window, never activate it, and refuse
//! toolkits known to discard posted events. Foreground sessions use SendInput
//! only after confirming the exact window. Native acceptance is not a claim
//! that the application's state changed; embedders still verify that state.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::Instant;

use cua_driver_core::interactive_input::{
    denormalize, extract_integral, validate_batch, InteractiveDeliveryMode, InteractiveInputBatch,
    InteractiveInputError, InteractiveInputEvent, InteractiveInputReceipt, KeyState, Modifier,
    PointerButton, PointerPhase,
};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::{delivery, keyboard, mouse};

#[derive(Clone, Copy, Debug)]
pub struct InteractiveInputConfig {
    /// Exact native target identity, required together for a window session.
    pub window: Option<(u32, u64)>,
    /// Whole-display capture rectangle, in physical desktop pixels.
    pub region: Option<(i32, i32, u32, u32)>,
    pub delivery_mode: InteractiveDeliveryMode,
}

pub struct InteractiveInputSession {
    config: InteractiveInputConfig,
    state: Mutex<Option<State>>,
}

#[derive(Default)]
struct State {
    // Remember each original recipient so release never follows changed focus.
    keys: BTreeMap<u16, u64>,
    explicit_modifiers: BTreeSet<u16>,
    buttons: BTreeMap<u8, u64>,
    last_point: (i32, i32),
    scroll_x: f64,
    scroll_y: f64,
}

fn native(error: impl std::fmt::Display) -> InteractiveInputError {
    InteractiveInputError::Native(error.to_string())
}

fn hwnd(id: u64) -> HWND {
    HWND(id as *mut _)
}

impl InteractiveInputSession {
    pub fn open(config: InteractiveInputConfig) -> Result<Self, InteractiveInputError> {
        if config.window.is_some() == config.region.is_some() {
            return Err(InteractiveInputError::InvalidTarget(
                "exactly one window or display region is required".into(),
            ));
        }
        if config.window.is_none() && config.delivery_mode == InteractiveDeliveryMode::Background {
            return Err(InteractiveInputError::WouldRequireActivation(
                "a display has no window to address background input to".into(),
            ));
        }
        let session = Self {
            config,
            state: Mutex::new(Some(State::default())),
        };
        session.geometry()?;
        // Opening does not steal focus; activation is paid on actual input.
        if let Some((_, id)) = config.window {
            if !session.foreground()
                && delivery::would_be_silently_dropped(id, delivery::EventKind::MouseMove)
            {
                return Err(InteractiveInputError::WouldRequireActivation(
                    "this target does not accept posted interactive pointer events".into(),
                ));
            }
        }
        Ok(session)
    }

    fn foreground(&self) -> bool {
        self.config.delivery_mode == InteractiveDeliveryMode::PersistentForeground
    }

    fn validate_target(&self) -> Result<(), InteractiveInputError> {
        if let Some((pid, id)) = self.config.window {
            let mut actual_pid = 0;
            let live = unsafe { IsWindow(hwnd(id)) }.as_bool();
            let thread = unsafe { GetWindowThreadProcessId(hwnd(id), Some(&mut actual_pid)) };
            if !live || thread == 0 || actual_pid != pid {
                return Err(InteractiveInputError::InvalidTarget(
                    "the exact window or its owning process is gone".into(),
                ));
            }
            if let Some(message) = super::post_message_blocked_by_uipi(id) {
                return Err(native(message));
            }
        }
        Ok(())
    }

    fn geometry(&self) -> Result<(i32, i32, u32, u32), InteractiveInputError> {
        self.validate_target()?;
        if let Some((_, id)) = self.config.window {
            let mut rect = RECT::default();
            // Match the extended frame used by the window capture provider.
            unsafe {
                if DwmGetWindowAttribute(
                    hwnd(id),
                    DWMWA_EXTENDED_FRAME_BOUNDS,
                    &mut rect as *mut RECT as *mut _,
                    std::mem::size_of::<RECT>() as u32,
                )
                .is_err()
                {
                    GetWindowRect(hwnd(id), &mut rect).map_err(native)?;
                }
            }
            if rect.right <= rect.left || rect.bottom <= rect.top {
                return Err(InteractiveInputError::InvalidTarget(
                    "window has no geometry".into(),
                ));
            }
            Ok((
                rect.left,
                rect.top,
                (rect.right - rect.left) as u32,
                (rect.bottom - rect.top) as u32,
            ))
        } else {
            let region = self.config.region.ok_or(InteractiveInputError::Closed)?;
            if region.2 == 0 || region.3 == 0 {
                return Err(InteractiveInputError::InvalidTarget(
                    "empty display region".into(),
                ));
            }
            Ok(region)
        }
    }

    fn prepare(&self) -> Result<(), InteractiveInputError> {
        self.validate_target()?;
        if let (true, Some((_, id))) = (self.foreground(), self.config.window) {
            if unsafe { GetForegroundWindow() } != hwnd(id)
                && !super::force_foreground_assisted(hwnd(id)).0
            {
                return Err(InteractiveInputError::WouldRequireActivation(
                    "Windows did not activate the exact interactive target; no input was sent"
                        .into(),
                ));
            }
            if unsafe { GetForegroundWindow() } != hwnd(id) {
                return Err(native("interactive target lost foreground before dispatch"));
            }
        }
        Ok(())
    }

    fn admit_background(&self, kind: delivery::EventKind) -> Result<(), InteractiveInputError> {
        self.validate_target()?;
        if !self.foreground() {
            let (_, id) = self.config.window.ok_or(InteractiveInputError::Closed)?;
            if delivery::would_be_silently_dropped(id, kind) {
                return Err(InteractiveInputError::WouldRequireActivation(format!(
                    "target discards posted {}; foreground delivery must be explicitly requested",
                    kind.name()
                )));
            }
        }
        Ok(())
    }

    pub fn dispatch(
        &self,
        batch: &InteractiveInputBatch,
    ) -> Result<InteractiveInputReceipt, InteractiveInputError> {
        let through = validate_batch(batch)?;
        let started = Instant::now();
        let mut guard = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = guard.as_mut().ok_or(InteractiveInputError::Closed)?;
        for event in &batch.events {
            // Recheck identity and foreground per event, not just at batch entry.
            if let Err(error) = self.prepare().and_then(|_| self.event(state, event)) {
                self.release_state(state);
                return Err(error);
            }
        }
        Ok(InteractiveInputReceipt {
            through_sequence: through,
            event_count: batch.events.len(),
            dispatch_micros: started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64,
        })
    }

    fn event(
        &self,
        state: &mut State,
        event: &InteractiveInputEvent,
    ) -> Result<(), InteractiveInputError> {
        match event {
            InteractiveInputEvent::TextCommit { text } => {
                self.admit_background(delivery::EventKind::TextInput)?;
                if self.foreground() {
                    // Composed text bypasses keyboard-layout/dead-key replay.
                    for unit in text.encode_utf16() {
                        if let Err(error) = self.send(&[
                            keyboard::unicode_key_input(unit, false),
                            keyboard::unicode_key_input(unit, true),
                        ]) {
                            // A partial SendInput must not leave VK_PACKET
                            // down even though the batch receives a nack.
                            let _ = self.send(&[keyboard::unicode_key_input(unit, true)]);
                            return Err(error);
                        }
                    }
                } else {
                    let (_, root) = self.config.window.ok_or(InteractiveInputError::Closed)?;
                    let target = keyboard::focused_descendant(hwnd(root)).unwrap_or(hwnd(root));
                    for unit in text.encode_utf16() {
                        unsafe { PostMessageW(target, WM_CHAR, WPARAM(unit as usize), LPARAM(1)) }
                            .map_err(native)?;
                    }
                }
            }
            InteractiveInputEvent::Key {
                key,
                state: edge,
                modifiers,
                repeat,
            } => {
                let vk = key_vk(key)?;
                self.admit_background(
                    if modifiers.is_empty() && state.explicit_modifiers.is_empty() {
                        delivery::EventKind::Keystroke
                    } else {
                        delivery::EventKind::KeyCombo
                    },
                )?;
                let modifier = [VK_LWIN, VK_SHIFT, VK_MENU, VK_CONTROL].contains(&vk);
                // Explicit key edges (including the viewer's chord menu)
                // survive events with an empty modifier snapshot. Snapshot
                // modifiers are reconciled independently for pointer/chords.
                if !modifier {
                    self.modifiers(state, modifiers)?;
                }
                self.key_edge(state, vk, *edge == KeyState::Down, *repeat)?;
                if modifier {
                    if *edge == KeyState::Down {
                        state.explicit_modifiers.insert(vk.0);
                    } else {
                        state.explicit_modifiers.remove(&vk.0);
                    }
                }
            }
            InteractiveInputEvent::Pointer {
                phase,
                button,
                x_normalized,
                y_normalized,
                modifiers,
            } => {
                self.admit_background(if *phase == PointerPhase::Move {
                    delivery::EventKind::MouseMove
                } else {
                    delivery::EventKind::MouseClick
                })?;
                self.modifiers(state, modifiers)?;
                state.last_point = self.point(*x_normalized, *y_normalized)?;
                if *phase == PointerPhase::Cancel {
                    self.release_buttons(state);
                } else {
                    self.pointer(state, *phase, *button)?;
                }
            }
            InteractiveInputEvent::Scroll {
                x_normalized,
                y_normalized,
                delta_x,
                delta_y,
                precise,
                ..
            } => {
                self.admit_background(delivery::EventKind::MouseScroll)?;
                state.last_point = self.point(*x_normalized, *y_normalized)?;
                // Win32 wheel messages preserve sub-notch precision; pixel
                // deltas use the same 40px/line convention as other adapters.
                let scale = if *precise { 120.0 / 40.0 } else { 120.0 };
                state.scroll_x += delta_x * scale;
                state.scroll_y -= delta_y * scale;
                let dx = extract_integral(&mut state.scroll_x);
                let dy = extract_integral(&mut state.scroll_y);
                self.wheel(state, dx, true)?;
                self.wheel(state, dy, false)?;
            }
        }
        Ok(())
    }

    fn point(&self, x: f64, y: f64) -> Result<(i32, i32), InteractiveInputError> {
        let (left, top, width, height) = self.geometry()?;
        Ok((
            left.saturating_add(denormalize(x, width)),
            top.saturating_add(denormalize(y, height)),
        ))
    }

    fn send(&self, events: &[INPUT]) -> Result<(), InteractiveInputError> {
        // No SendInput path is reachable from a background session.
        if !self.foreground() {
            return Err(native("global input is forbidden by session policy"));
        }
        let sent = unsafe { SendInput(events, std::mem::size_of::<INPUT>() as i32) };
        if sent as usize != events.len() {
            return Err(native(format!(
                "SendInput accepted {sent}/{} events; input desktop or UIPI blocked delivery",
                events.len()
            )));
        }
        Ok(())
    }

    fn modifiers(
        &self,
        state: &mut State,
        modifiers: &[Modifier],
    ) -> Result<(), InteractiveInputError> {
        let desired = modifiers
            .iter()
            .map(|modifier| match modifier {
                Modifier::Command => Ok(VK_LWIN),
                Modifier::Shift => Ok(VK_SHIFT),
                Modifier::Option => Ok(VK_MENU),
                Modifier::Control => Ok(VK_CONTROL),
                Modifier::Function => Err(InteractiveInputError::InvalidBatch(
                    "Windows has no synthetic Fn modifier".into(),
                )),
            })
            .collect::<Result<Vec<_>, _>>()?;
        for vk in [VK_LWIN, VK_SHIFT, VK_MENU, VK_CONTROL] {
            if state.keys.contains_key(&vk.0)
                && !desired.contains(&vk)
                && !state.explicit_modifiers.contains(&vk.0)
            {
                self.key_edge(state, vk, false, false)?;
            }
        }
        for vk in desired {
            if !state.keys.contains_key(&vk.0) {
                self.key_edge(state, vk, true, false)?;
            }
        }
        Ok(())
    }

    fn recipient_live(&self, recipient: u64) -> bool {
        let Some((pid, root)) = self.config.window else {
            return true;
        };
        let mut actual = 0;
        unsafe {
            IsWindow(hwnd(recipient)).as_bool()
                && GetWindowThreadProcessId(hwnd(recipient), Some(&mut actual)) != 0
                && actual == pid
                && (recipient == root || IsChild(hwnd(root), hwnd(recipient)).as_bool())
        }
    }

    fn key_edge(
        &self,
        state: &mut State,
        vk: VIRTUAL_KEY,
        down: bool,
        repeat: bool,
    ) -> Result<(), InteractiveInputError> {
        if !down && !state.keys.contains_key(&vk.0) {
            return Ok(());
        }
        if down && state.keys.contains_key(&vk.0) && !repeat {
            return Ok(());
        }
        let target = if let Some(target) = state.keys.get(&vk.0) {
            *target
        } else if let Some((_, root)) = self.config.window {
            keyboard::focused_descendant(hwnd(root))
                .unwrap_or(hwnd(root))
                .0 as usize as u64
        } else {
            0
        };
        if self.foreground() {
            // Do not acquire/release a physical key already held by the user.
            if down
                && !state.keys.contains_key(&vk.0)
                && unsafe { GetAsyncKeyState(vk.0 as i32) } < 0
            {
                return Err(native(
                    "key is already physically held; no synthetic edge was sent",
                ));
            }
            self.send(&[keyboard::key_input(vk, !down)])?;
        } else {
            if !self.recipient_live(target) {
                return Err(InteractiveInputError::InvalidTarget(
                    "keyboard recipient is gone".into(),
                ));
            }
            let scan = unsafe { MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC) };
            let mut bits = 1u32 | (scan << 16);
            if keyboard::is_extended(vk) {
                bits |= 1 << 24;
            }
            if repeat || !down {
                bits |= 1 << 30;
            }
            if !down {
                bits |= 1 << 31;
            }
            let alt = vk == VK_MENU || state.keys.contains_key(&VK_MENU.0);
            if alt {
                bits |= 1 << 29;
            }
            let message = match (alt, down) {
                (true, true) => WM_SYSKEYDOWN,
                (true, false) => WM_SYSKEYUP,
                (false, true) => WM_KEYDOWN,
                (false, false) => WM_KEYUP,
            };
            unsafe {
                PostMessageW(
                    hwnd(target),
                    message,
                    WPARAM(vk.0 as usize),
                    LPARAM(bits as isize),
                )
            }
            .map_err(native)?;
        }
        if down {
            state.keys.insert(vk.0, target);
        } else {
            state.keys.remove(&vk.0);
        }
        Ok(())
    }

    fn pointer(
        &self,
        state: &mut State,
        phase: PointerPhase,
        button: Option<PointerButton>,
    ) -> Result<(), InteractiveInputError> {
        let index = button.map(button_index);
        if phase != PointerPhase::Move && index.is_none() {
            return Err(InteractiveInputError::InvalidBatch(
                "pointer edge requires a button".into(),
            ));
        }
        if let Some(index) = index {
            // Never release a physical button on an orphan remote up, or
            // acquire the same edge twice after a repeated down.
            if (phase == PointerPhase::Up && !state.buttons.contains_key(&index))
                || (phase == PointerPhase::Down && state.buttons.contains_key(&index))
            {
                return Ok(());
            }
        }
        let (sx, sy) = state.last_point;
        if self.foreground() {
            // Mouse input is coordinate-routed: don't deliver to an occluding
            // unrelated window, even if our target still owns keyboard focus.
            if let Some((_, root)) = self.config.window {
                let hit = unsafe { WindowFromPoint(POINT { x: sx, y: sy }) };
                if hit != hwnd(root) && !unsafe { IsChild(hwnd(root), hit) }.as_bool() {
                    return Err(InteractiveInputError::WouldRequireActivation(
                        "target is occluded at pointer position".into(),
                    ));
                }
            }
            let (dx, dy) = unsafe {
                crate::virtualdesk::to_virtualdesk_absolute(
                    sx,
                    sy,
                    GetSystemMetrics(SM_XVIRTUALSCREEN),
                    GetSystemMetrics(SM_YVIRTUALSCREEN),
                    GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1),
                    GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1),
                )
            };
            let mut flags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
            if phase != PointerPhase::Move {
                if let Some(index) = index {
                    flags |= button_flags(index, phase == PointerPhase::Down);
                }
            }
            self.send(&[mouse_input(dx, dy, 0, flags)])?;
            if let Some(index) = index {
                if phase == PointerPhase::Down {
                    state.buttons.insert(index, 0);
                }
                if phase == PointerPhase::Up {
                    state.buttons.remove(&index);
                }
            }
        } else {
            let (_, root) = self.config.window.ok_or(InteractiveInputError::Closed)?;
            let captured = index
                .and_then(|index| state.buttons.get(&index))
                .copied()
                .or_else(|| state.buttons.values().next().copied());
            let (target, point) = if let Some(target) = captured {
                if !self.recipient_live(target) {
                    return Err(InteractiveInputError::InvalidTarget(
                        "pointer recipient is gone".into(),
                    ));
                }
                let mut point = POINT { x: sx, y: sy };
                if !unsafe { ScreenToClient(hwnd(target), &mut point) }.as_bool() {
                    return Err(native("ScreenToClient failed"));
                }
                (hwnd(target), point)
            } else {
                mouse::deepest_child(hwnd(root), POINT { x: sx, y: sy })
            };
            let message = match (phase, index) {
                (PointerPhase::Move, _) => WM_MOUSEMOVE,
                (PointerPhase::Down, Some(0)) => WM_LBUTTONDOWN,
                (PointerPhase::Up, Some(0)) => WM_LBUTTONUP,
                (PointerPhase::Down, Some(1)) => WM_RBUTTONDOWN,
                (PointerPhase::Up, Some(1)) => WM_RBUTTONUP,
                (PointerPhase::Down, Some(2)) => WM_MBUTTONDOWN,
                (PointerPhase::Up, Some(2)) => WM_MBUTTONUP,
                _ => {
                    return Err(InteractiveInputError::InvalidBatch(
                        "invalid pointer transition".into(),
                    ))
                }
            };
            let mut bits = mouse_bits(state);
            if let Some(index) = index {
                let mask = match index {
                    0 => 1,
                    1 => 2,
                    _ => 16,
                };
                if phase == PointerPhase::Down {
                    bits |= mask;
                }
                if phase == PointerPhase::Up {
                    bits &= !mask;
                }
            }
            let packed = crate::lparam::pack_xy(point.x, point.y).map_err(native)?;
            unsafe {
                PostMessageW(
                    target,
                    message,
                    WPARAM(bits as usize),
                    LPARAM(packed as isize),
                )
            }
            .map_err(native)?;
            if let Some(index) = index {
                if phase == PointerPhase::Down {
                    state.buttons.insert(index, target.0 as usize as u64);
                }
                if phase == PointerPhase::Up {
                    state.buttons.remove(&index);
                }
            }
        }
        Ok(())
    }

    fn wheel(
        &self,
        state: &State,
        delta: i32,
        horizontal: bool,
    ) -> Result<(), InteractiveInputError> {
        if delta == 0 {
            return Ok(());
        }
        if self.foreground() {
            self.pointer_move_for_wheel(state)?;
            self.send(&[mouse_input(
                0,
                0,
                delta as u32,
                if horizontal {
                    MOUSEEVENTF_HWHEEL
                } else {
                    MOUSEEVENTF_WHEEL
                },
            )])
        } else {
            let (_, root) = self.config.window.ok_or(InteractiveInputError::Closed)?;
            let (x, y) = state.last_point;
            let (target, _) = mouse::deepest_child(hwnd(root), POINT { x, y });
            let word = mouse_bits(state) | ((delta as u16 as u32) << 16);
            let packed = crate::lparam::pack_xy(x, y).map_err(native)?;
            unsafe {
                PostMessageW(
                    target,
                    if horizontal {
                        WM_MOUSEHWHEEL
                    } else {
                        WM_MOUSEWHEEL
                    },
                    WPARAM(word as usize),
                    LPARAM(packed as isize),
                )
            }
            .map_err(native)
        }
    }

    fn pointer_move_for_wheel(&self, state: &State) -> Result<(), InteractiveInputError> {
        let mut moving = State {
            last_point: state.last_point,
            ..State::default()
        };
        self.pointer(&mut moving, PointerPhase::Move, None)
    }

    fn release_buttons(&self, state: &mut State) {
        let buttons = std::mem::take(&mut state.buttons);
        for (index, target) in buttons {
            if self.foreground() {
                let _ = self.send(&[mouse_input(0, 0, 0, button_flags(index, false))]);
            } else if self.recipient_live(target) && self.validate_target().is_ok() {
                let mut point = POINT {
                    x: state.last_point.0,
                    y: state.last_point.1,
                };
                if unsafe { ScreenToClient(hwnd(target), &mut point) }.as_bool() {
                    let message = match index {
                        0 => WM_LBUTTONUP,
                        1 => WM_RBUTTONUP,
                        _ => WM_MBUTTONUP,
                    };
                    if let Ok(packed) = crate::lparam::pack_xy(point.x, point.y) {
                        let _ = unsafe {
                            PostMessageW(hwnd(target), message, WPARAM(0), LPARAM(packed as isize))
                        };
                    }
                }
            }
        }
    }

    fn release_state(&self, state: &mut State) {
        self.release_buttons(state);
        let keys = state.keys.keys().copied().collect::<Vec<_>>();
        for key in keys {
            // Cleanup never activates a window; global key-up releases only
            // session-owned edges, even if foreground changed on disconnect.
            if self.foreground() || self.validate_target().is_ok() {
                let _ = self.key_edge(state, VIRTUAL_KEY(key), false, false);
            }
        }
        state.keys.clear();
        state.explicit_modifiers.clear();
    }

    /// Release held inputs on ownership loss or detach without invalidating
    /// the lease. A reattached viewer can continue using the same session.
    pub fn release_all(&self) {
        if let Some(state) = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
        {
            self.release_state(state);
            state.scroll_x = 0.0;
            state.scroll_y = 0.0;
        }
    }
}

impl Drop for InteractiveInputSession {
    fn drop(&mut self) {
        if let Some(mut state) = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            self.release_state(&mut state);
        }
    }
}

fn key_vk(key: &str) -> Result<VIRTUAL_KEY, InteractiveInputError> {
    // DOM physical codes are used by the viewer; reuse the driver's native
    // mapping after reducing the DOM aliases to its key vocabulary.
    let mapped = key
        .strip_prefix("Key")
        .filter(|s| s.len() == 1)
        .or_else(|| key.strip_prefix("Digit").filter(|s| s.len() == 1))
        .or_else(|| key.strip_prefix("Arrow"))
        .unwrap_or(key);
    let lowered = mapped.to_ascii_lowercase();
    let mapped = match lowered.as_str() {
        "shiftleft" | "shiftright" => "shift",
        "controlleft" | "controlright" => "control",
        "altleft" | "altright" => "alt",
        "metaleft" | "metaright" => "win",
        "arrowleft" => "left",
        "arrowright" => "right",
        "arrowup" => "up",
        "arrowdown" => "down",
        "backquote" => "`",
        "minus" => "-",
        "equal" => "=",
        "bracketleft" => "[",
        "bracketright" => "]",
        "backslash" => "\\",
        "semicolon" => ";",
        "quote" => "'",
        "comma" => ",",
        "period" => ".",
        "slash" => "/",
        _ => lowered.as_str(),
    };
    if mapped.chars().count() != 1
        && !matches!(
            mapped,
            "enter"
                | "return"
                | "tab"
                | "escape"
                | "esc"
                | "space"
                | "backspace"
                | "delete"
                | "del"
                | "insert"
                | "ins"
                | "home"
                | "end"
                | "pageup"
                | "pgup"
                | "pagedown"
                | "pgdn"
                | "up"
                | "down"
                | "left"
                | "right"
                | "f1"
                | "f2"
                | "f3"
                | "f4"
                | "f5"
                | "f6"
                | "f7"
                | "f8"
                | "f9"
                | "f10"
                | "f11"
                | "f12"
                | "ctrl"
                | "control"
                | "shift"
                | "alt"
                | "win"
                | "windows"
                | "meta"
                | "command"
                | "cmd"
                | "capslock"
                | "numlock"
        )
    {
        return Err(InteractiveInputError::InvalidBatch(format!(
            "unsupported interactive key: {key}"
        )));
    }
    keyboard::key_name_to_vk(mapped)
        .map_err(|error| InteractiveInputError::InvalidBatch(error.to_string()))
}

fn button_index(button: PointerButton) -> u8 {
    match button {
        PointerButton::Left => 0,
        PointerButton::Right => 1,
        PointerButton::Middle => 2,
    }
}

fn button_flags(index: u8, down: bool) -> MOUSE_EVENT_FLAGS {
    match (index, down) {
        (0, true) => MOUSEEVENTF_LEFTDOWN,
        (0, false) => MOUSEEVENTF_LEFTUP,
        (1, true) => MOUSEEVENTF_RIGHTDOWN,
        (1, false) => MOUSEEVENTF_RIGHTUP,
        (_, true) => MOUSEEVENTF_MIDDLEDOWN,
        (_, false) => MOUSEEVENTF_MIDDLEUP,
    }
}

fn mouse_bits(state: &State) -> u32 {
    let mut bits = 0;
    for index in state.buttons.keys() {
        bits |= match index {
            0 => 1,
            1 => 2,
            _ => 16,
        };
    }
    if state.keys.contains_key(&VK_SHIFT.0) {
        bits |= 4;
    }
    if state.keys.contains_key(&VK_CONTROL.0) {
        bits |= 8;
    }
    bits
}

fn mouse_input(dx: i32, dy: i32, data: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}
