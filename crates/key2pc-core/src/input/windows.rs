use std::cell::RefCell;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Receiver, Sender};
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{
    GetCurrentThread, GetCurrentThreadId, SetThreadPriority, THREAD_PRIORITY_HIGHEST,
    THREAD_PRIORITY_TIME_CRITICAL,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_ABSOLUTE,
    MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
    MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, GetSystemMetrics, PostQuitMessage,
    PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, KBDLLHOOKSTRUCT,
    LLKHF_ALTDOWN, LLKHF_EXTENDED, LLKHF_INJECTED, LLKHF_UP, MSG, SM_CXSCREEN, SM_CXVIRTUALSCREEN,
    SM_CYSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, WH_KEYBOARD_LL, WM_APP,
};

use crate::error::{Key2PcError, Result};
use crate::input::processor::HookProcessor;
use crate::input::types::{
    DesktopBounds, HookControl, HookEvent, HookOptions, InputSink, PressedKey, PressedKeySet,
    KEY2PC_EXTRA_INFO,
};
use crate::protocol::{KeyAction, KeyEvent, MouseAction, MouseEvent};

const WM_APP_HOOK_CONTROL: u32 = WM_APP + 10;
const XBUTTON1: u16 = 0x0001;
const XBUTTON2: u16 = 0x0002;

thread_local! {
    static HOOK_STATE: RefCell<Option<HookThreadContext>> = const { RefCell::new(None) };
}

struct HookThreadContext {
    processor: HookProcessor,
    control_rx: Receiver<HookControl>,
}

pub fn boost_current_thread_priority(time_critical: bool) {
    let level = if time_critical {
        THREAD_PRIORITY_TIME_CRITICAL
    } else {
        THREAD_PRIORITY_HIGHEST
    };
    unsafe {
        SetThreadPriority(GetCurrentThread(), level);
    }
}

fn is_vk_only_key(vk: u16) -> bool {
    matches!(vk, 0x13 | 0x5B | 0x5C | 0x5D | 0xA6..=0xB7)
}

fn build_key_input(
    vk: u16,
    scan_code: u16,
    extended: bool,
    is_up: bool,
    prefer_scancode: bool,
) -> INPUT {
    let use_scancode = prefer_scancode && scan_code != 0 && !is_vk_only_key(vk);
    let mut flags: KEYBD_EVENT_FLAGS = 0;
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if is_up {
        flags |= KEYEVENTF_KEYUP;
    }
    if use_scancode {
        flags |= KEYEVENTF_SCANCODE;
    }

    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: if use_scancode { 0 } else { vk },
                wScan: scan_code,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: KEY2PC_EXTRA_INFO,
            },
        },
    }
}

fn build_mouse_input(dx: i32, dy: i32, mouse_data: u32, dw_flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: mouse_data,
                dwFlags: dw_flags,
                time: 0,
                dwExtraInfo: KEY2PC_EXTRA_INFO,
            },
        },
    }
}

fn send_inputs(inputs: &[INPUT]) -> Result<()> {
    if inputs.is_empty() {
        return Ok(());
    }
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
    if sent == 0 {
        return Err(Key2PcError::Input(format!(
            "SendInput failed: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(())
}

fn release_keys_locally(keys: &[PressedKey]) {
    if keys.is_empty() {
        return;
    }
    let inputs: Vec<INPUT> = keys
        .iter()
        .rev()
        .map(|key| build_key_input(key.vk, key.scan_code, key.extended, true, false))
        .collect();
    let _ = send_inputs(&inputs);
}

fn query_primary_bounds() -> DesktopBounds {
    let w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    DesktopBounds::new(0, 0, w, h)
}

fn query_virtual_desktop_bounds() -> DesktopBounds {
    let left = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let top = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let w = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let h = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
    if w > 0 && h > 0 {
        DesktopBounds::new(left, top, w, h)
    } else {
        query_primary_bounds()
    }
}

pub struct WinInputInjector {
    prefer_scancode: bool,
    held_keys: PressedKeySet,
    monitor_bounds: DesktopBounds,
    held_mouse_buttons: u8,
}

pub type WinKeyboardInjector = WinInputInjector;

impl WinInputInjector {
    pub fn new(prefer_scancode: bool) -> Self {
        Self {
            prefer_scancode,
            held_keys: PressedKeySet::default(),
            monitor_bounds: query_primary_bounds(),
            held_mouse_buttons: 0,
        }
    }

    fn track_mouse_button(&mut self, action: MouseAction) {
        match action {
            MouseAction::LeftDown => self.held_mouse_buttons |= 1 << 0,
            MouseAction::LeftUp => self.held_mouse_buttons &= !(1 << 0),
            MouseAction::RightDown => self.held_mouse_buttons |= 1 << 1,
            MouseAction::RightUp => self.held_mouse_buttons &= !(1 << 1),
            MouseAction::MiddleDown => self.held_mouse_buttons |= 1 << 2,
            MouseAction::MiddleUp => self.held_mouse_buttons &= !(1 << 2),
            MouseAction::X1Down => self.held_mouse_buttons |= 1 << 3,
            MouseAction::X1Up => self.held_mouse_buttons &= !(1 << 3),
            MouseAction::X2Down => self.held_mouse_buttons |= 1 << 4,
            MouseAction::X2Up => self.held_mouse_buttons &= !(1 << 4),
            _ => {}
        }
    }
}

impl InputSink for WinInputInjector {
    fn handle_key(&mut self, event: KeyEvent) -> Result<()> {
        let is_up = event.action == KeyAction::Up;
        self.held_keys.track(
            PressedKey {
                vk: event.vk,
                scan_code: event.scan_code,
                extended: event.extended,
            },
            is_up,
        );
        let input = build_key_input(
            event.vk,
            event.scan_code,
            event.extended,
            is_up,
            self.prefer_scancode,
        );
        send_inputs(&[input])
    }

    fn handle_mouse(&mut self, event: MouseEvent) -> Result<()> {
        self.track_mouse_button(event.action);
        let virtual_bounds = query_virtual_desktop_bounds();
        let (vx, vy) = self.monitor_bounds.map_normalized_to_virtual(
            event.norm_x,
            event.norm_y,
            virtual_bounds,
        );
        let abs_move = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;

        let input = match event.action {
            MouseAction::Move => build_mouse_input(vx, vy, 0, abs_move),
            MouseAction::LeftDown => build_mouse_input(vx, vy, 0, abs_move | MOUSEEVENTF_LEFTDOWN),
            MouseAction::LeftUp => build_mouse_input(vx, vy, 0, abs_move | MOUSEEVENTF_LEFTUP),
            MouseAction::RightDown => {
                build_mouse_input(vx, vy, 0, abs_move | MOUSEEVENTF_RIGHTDOWN)
            }
            MouseAction::RightUp => build_mouse_input(vx, vy, 0, abs_move | MOUSEEVENTF_RIGHTUP),
            MouseAction::MiddleDown => {
                build_mouse_input(vx, vy, 0, abs_move | MOUSEEVENTF_MIDDLEDOWN)
            }
            MouseAction::MiddleUp => build_mouse_input(vx, vy, 0, abs_move | MOUSEEVENTF_MIDDLEUP),
            MouseAction::X1Down => {
                build_mouse_input(vx, vy, u32::from(XBUTTON1), abs_move | MOUSEEVENTF_XDOWN)
            }
            MouseAction::X1Up => {
                build_mouse_input(vx, vy, u32::from(XBUTTON1), abs_move | MOUSEEVENTF_XUP)
            }
            MouseAction::X2Down => {
                build_mouse_input(vx, vy, u32::from(XBUTTON2), abs_move | MOUSEEVENTF_XDOWN)
            }
            MouseAction::X2Up => {
                build_mouse_input(vx, vy, u32::from(XBUTTON2), abs_move | MOUSEEVENTF_XUP)
            }
            MouseAction::Wheel => build_mouse_input(
                vx,
                vy,
                i32::from(event.wheel_delta) as u32,
                abs_move | MOUSEEVENTF_WHEEL,
            ),
            MouseAction::HWheel => build_mouse_input(
                vx,
                vy,
                i32::from(event.wheel_delta) as u32,
                abs_move | MOUSEEVENTF_HWHEEL,
            ),
        };
        send_inputs(&[input])
    }

    fn set_monitor_bounds(&mut self, bounds: DesktopBounds) {
        self.monitor_bounds = bounds;
    }

    fn release_all(&mut self) -> Result<()> {
        let held = self.held_keys.take_all();
        let mut inputs: Vec<INPUT> = held
            .into_iter()
            .rev()
            .map(|key| {
                build_key_input(
                    key.vk,
                    key.scan_code,
                    key.extended,
                    true,
                    self.prefer_scancode,
                )
            })
            .collect();

        if (self.held_mouse_buttons & (1 << 0)) != 0 {
            inputs.push(build_mouse_input(0, 0, 0, MOUSEEVENTF_LEFTUP));
        }
        if (self.held_mouse_buttons & (1 << 1)) != 0 {
            inputs.push(build_mouse_input(0, 0, 0, MOUSEEVENTF_RIGHTUP));
        }
        if (self.held_mouse_buttons & (1 << 2)) != 0 {
            inputs.push(build_mouse_input(0, 0, 0, MOUSEEVENTF_MIDDLEUP));
        }
        if (self.held_mouse_buttons & (1 << 3)) != 0 {
            inputs.push(build_mouse_input(
                0,
                0,
                u32::from(XBUTTON1),
                MOUSEEVENTF_XUP,
            ));
        }
        if (self.held_mouse_buttons & (1 << 4)) != 0 {
            inputs.push(build_mouse_input(
                0,
                0,
                u32::from(XBUTTON2),
                MOUSEEVENTF_XUP,
            ));
        }
        self.held_mouse_buttons = 0;

        send_inputs(&inputs)
    }

    fn held_count(&self) -> usize {
        self.held_keys.len() + self.held_mouse_buttons.count_ones() as usize
    }
}

pub struct WinKeyboardHook {
    thread_id: u32,
    shared_active: Arc<AtomicBool>,
    control_tx: Sender<HookControl>,
    event_tx: Sender<HookEvent>,
    handle: Option<JoinHandle<()>>,
}

impl WinKeyboardHook {
    pub fn start(options: HookOptions) -> Result<(Self, Receiver<HookEvent>)> {
        let (event_tx, event_rx) = crossbeam_channel::bounded(1024);
        let event_tx_clone = event_tx.clone();
        let (control_tx, control_rx) = crossbeam_channel::unbounded();
        let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<u32>>(1);
        let shared_active = Arc::new(AtomicBool::new(options.start_active));
        let shared_active_clone = Arc::clone(&shared_active);

        let handle = thread::Builder::new()
            .name("key2pc-hook".to_string())
            .spawn(move || {
                boost_current_thread_priority(true);
                let thread_id = unsafe { GetCurrentThreadId() };
                let hmod = unsafe { GetModuleHandleW(ptr::null()) };
                let hook = unsafe {
                    SetWindowsHookExW(WH_KEYBOARD_LL, Some(low_level_keyboard_proc), hmod, 0)
                };
                if hook.is_null() {
                    let err = std::io::Error::last_os_error();
                    let _ = ready_tx.send(Err(Key2PcError::Input(format!(
                        "SetWindowsHookExW failed: {err}"
                    ))));
                    return;
                }

                HOOK_STATE.with(|slot| {
                    *slot.borrow_mut() = Some(HookThreadContext {
                        processor: HookProcessor::new(options, shared_active_clone, event_tx_clone),
                        control_rx,
                    });
                });

                let _ = ready_tx.send(Ok(thread_id));

                let mut msg: MSG = unsafe { std::mem::zeroed() };
                loop {
                    let status = unsafe { GetMessageW(&raw mut msg, ptr::null_mut(), 0, 0) };
                    if status <= 0 {
                        break;
                    }
                    if msg.message == WM_APP_HOOK_CONTROL {
                        drain_hook_controls();
                        continue;
                    }
                    unsafe {
                        TranslateMessage(&raw const msg);
                        DispatchMessageW(&raw const msg);
                    }
                }

                unsafe {
                    UnhookWindowsHookEx(hook);
                }
                HOOK_STATE.with(|slot| {
                    *slot.borrow_mut() = None;
                });
            })
            .map_err(|err| Key2PcError::Input(format!("Failed to spawn hook thread: {err}")))?;

        let thread_id = ready_rx
            .recv()
            .map_err(|_| Key2PcError::Input("Hook thread exited prematurely".to_string()))??;

        Ok((
            Self {
                thread_id,
                shared_active,
                control_tx,
                event_tx,
                handle: Some(handle),
            },
            event_rx,
        ))
    }

    pub fn event_sender(&self) -> Sender<HookEvent> {
        self.event_tx.clone()
    }

    pub fn is_active(&self) -> bool {
        self.shared_active.load(Ordering::Acquire)
    }

    pub fn set_active(&self, active: bool) {
        let _ = self.control_tx.send(HookControl::SetActive(active));
        unsafe {
            PostThreadMessageW(self.thread_id, WM_APP_HOOK_CONTROL, 0, 0);
        }
    }

    pub fn toggle_active(&self) {
        let _ = self.control_tx.send(HookControl::ToggleActive);
        unsafe {
            PostThreadMessageW(self.thread_id, WM_APP_HOOK_CONTROL, 0, 0);
        }
    }

    pub fn release_all_remote(&self) {
        let _ = self.control_tx.send(HookControl::ReleaseAllRemote);
        unsafe {
            PostThreadMessageW(self.thread_id, WM_APP_HOOK_CONTROL, 0, 0);
        }
    }

    pub fn shutdown(&mut self) {
        let _ = self.control_tx.send(HookControl::Shutdown);
        unsafe {
            PostThreadMessageW(self.thread_id, WM_APP_HOOK_CONTROL, 0, 0);
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for WinKeyboardHook {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn drain_hook_controls() {
    HOOK_STATE.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let Some(ctx) = borrow.as_mut() else {
            return;
        };
        while let Ok(cmd) = ctx.control_rx.try_recv() {
            match cmd {
                HookControl::SetActive(next) => {
                    ctx.processor
                        .transition_active(next, &mut release_keys_locally);
                }
                HookControl::ToggleActive => {
                    let next = !ctx.processor.is_active();
                    ctx.processor
                        .transition_active(next, &mut release_keys_locally);
                }
                HookControl::ReleaseAllRemote => {
                    ctx.processor.request_release_all();
                }
                HookControl::Shutdown => unsafe {
                    PostQuitMessage(0);
                },
            }
        }
    });
}

unsafe extern "system" fn low_level_keyboard_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code >= 0 && lparam != 0 {
        let info = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let vk = info.vkCode as u16;
        let scan_code = info.scanCode as u16;
        let is_up = (info.flags & LLKHF_UP) != 0;
        let extended = (info.flags & LLKHF_EXTENDED) != 0;
        let alt_down = (info.flags & LLKHF_ALTDOWN) != 0;
        let injected = (info.flags & LLKHF_INJECTED) != 0;

        let suppress = HOOK_STATE.with(|slot| {
            let mut borrow = slot.borrow_mut();
            let Some(ctx) = borrow.as_mut() else {
                return false;
            };
            ctx.processor.handle_key_raw(
                vk,
                scan_code,
                is_up,
                extended,
                alt_down,
                injected,
                info.dwExtraInfo,
                &mut release_keys_locally,
            )
        });

        if suppress {
            return 1;
        }
    }
    unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) }
}
