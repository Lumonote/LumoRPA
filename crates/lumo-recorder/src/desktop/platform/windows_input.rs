//! Session-owned Win32 low-level input hooks, with password-aware edit reads.

use std::cell::RefCell;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::desktop::input::{CaptureGuard, InputEmitter};

#[repr(C)]
#[derive(Default)]
struct Point {
    x: i32,
    y: i32,
}
#[repr(C)]
#[derive(Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}
#[repr(C)]
struct Message {
    hwnd: isize,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    point: Point,
    private: u32,
}
#[repr(C)]
struct Mouse {
    point: Point,
    data: u32,
    flags: u32,
    time: u32,
    extra: usize,
}
#[repr(C)]
struct Keyboard {
    code: u32,
    scan: u32,
    flags: u32,
    time: u32,
    extra: usize,
}
#[repr(C)]
#[derive(Default)]
struct GuiInfo {
    size: u32,
    flags: u32,
    active: isize,
    focus: isize,
    capture: isize,
    menu: isize,
    move_size: isize,
    caret: isize,
    rect: Rect,
}

#[link(name = "user32")]
extern "system" {
    fn SetWindowsHookExW(
        id: i32,
        callback: Option<extern "system" fn(i32, usize, isize) -> isize>,
        module: isize,
        thread: u32,
    ) -> isize;
    fn UnhookWindowsHookEx(hook: isize) -> i32;
    fn CallNextHookEx(hook: isize, code: i32, wparam: usize, lparam: isize) -> isize;
    fn GetMessageW(message: *mut Message, hwnd: isize, min: u32, max: u32) -> i32;
    fn PeekMessageW(message: *mut Message, hwnd: isize, min: u32, max: u32, remove: u32) -> i32;
    fn PostThreadMessageW(thread: u32, message: u32, wparam: usize, lparam: isize) -> i32;
    fn GetAsyncKeyState(key: i32) -> i16;
    fn GetGUIThreadInfo(thread: u32, info: *mut GuiInfo) -> i32;
    fn GetClassNameW(hwnd: isize, text: *mut u16, max: i32) -> i32;
    fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
    fn SendMessageTimeoutW(
        hwnd: isize,
        message: u32,
        wparam: usize,
        lparam: isize,
        flags: u32,
        timeout: u32,
        result: *mut usize,
    ) -> isize;
}
#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentThreadId() -> u32;
}

thread_local! { static EMITTER: RefCell<Option<InputEmitter>> = const { RefCell::new(None) }; }

pub fn start(emitter: InputEmitter) -> anyhow::Result<CaptureGuard> {
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let worker = thread::Builder::new().name("lumo-input-windows".into()).spawn(move || unsafe {
        EMITTER.with(|slot| *slot.borrow_mut() = Some(emitter));
        let mouse = SetWindowsHookExW(14, Some(mouse_callback), 0, 0);
        let keyboard = SetWindowsHookExW(13, Some(key_callback), 0, 0);
        if mouse == 0 || keyboard == 0 {
            if mouse != 0 { UnhookWindowsHookEx(mouse); }
            if keyboard != 0 { UnhookWindowsHookEx(keyboard); }
            let _ = ready_tx.send(Err("Cannot install desktop input hooks. Run LumoRPA in an interactive desktop session."));
            return;
        }
        let mut message: Message = std::mem::zeroed();
        PeekMessageW(&mut message, 0, 0, 0, 0);
        if ready_tx.send(Ok(GetCurrentThreadId())).is_ok() {
            while GetMessageW(&mut message, 0, 0, 0) > 0 {}
        }
        UnhookWindowsHookEx(mouse);
        UnhookWindowsHookEx(keyboard);
        EMITTER.with(|slot| *slot.borrow_mut() = None);
    })?;
    let thread_id = match ready_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(thread_id)) => thread_id,
        Ok(Err(message)) => {
            let _ = worker.join();
            anyhow::bail!(message);
        }
        Err(error) => anyhow::bail!("Desktop input capture did not initialize: {error}"),
    };
    Ok(CaptureGuard::new(move || {
        unsafe {
            PostThreadMessageW(thread_id, 0x12, 0, 0);
        }
        let _ = worker.join();
    }))
}

extern "system" fn mouse_callback(code: i32, message: usize, data: isize) -> isize {
    let _ = std::panic::catch_unwind(|| unsafe {
        if code < 0 {
            return;
        }
        let data = &*(data as *const Mouse);
        if data.flags & 1 != 0 {
            return;
        }
        EMITTER.with(|slot| {
            let slot = slot.borrow();
            let Some(emitter) = slot.as_ref() else {
                return;
            };
            let now = chrono::Utc::now().timestamp_millis();
            let (x, y) = (data.point.x as f64, data.point.y as f64);
            match message {
                0x201 | 0x204 | 0x207 => {
                    let button = match message {
                        0x201 => "left",
                        0x204 => "right",
                        _ => "middle",
                    };
                    emitter.with(|r| {
                        r.pointer_down(button, x, y, now);
                        None
                    });
                }
                0x202 | 0x205 | 0x208 => {
                    let button = match message {
                        0x202 => "left",
                        0x205 => "right",
                        _ => "middle",
                    };
                    emitter.with(|r| r.pointer_up(button, x, y, now));
                }
                0x20a | 0x20e => {
                    let delta = (data.data >> 16) as i16 as i64 / 120;
                    emitter.with(|r| {
                        if message == 0x20a {
                            r.scroll(0, -delta)
                        } else {
                            r.scroll(delta, 0)
                        }
                    });
                }
                _ => {}
            }
        });
    });
    unsafe { CallNextHookEx(0, code, message, data) }
}

extern "system" fn key_callback(code: i32, message: usize, data: isize) -> isize {
    let _ = std::panic::catch_unwind(|| unsafe {
        if code < 0 {
            return;
        }
        let data = &*(data as *const Keyboard);
        if data.flags & 0x10 != 0 {
            return;
        }
        EMITTER.with(|slot| {
            let slot = slot.borrow();
            let Some(emitter) = slot.as_ref() else {
                return;
            };
            let focused = focused_edit();
            let mut mods = Vec::new();
            for (key, name) in [
                (0x10, "shift"),
                (0x11, "ctrl"),
                (0x12, "alt"),
                (0x5b, "cmd"),
            ] {
                if GetAsyncKeyState(key) < 0 {
                    mods.push(name);
                }
            }
            if matches!(message, 0x100 | 0x104) {
                if let Some(key) = key_name(data.code) {
                    emitter.with(|r| r.key(&key, &mods, focused.as_ref().is_none_or(|f| f.secure)));
                }
                if let Some(f) = focused.filter(|f| f.safe) {
                    emitter.with(|r| {
                        r.baseline_text(&f.field, &f.value, true);
                        None
                    });
                }
            } else if matches!(message, 0x101 | 0x105) {
                if let Some(f) = focused.filter(|f| f.safe) {
                    emitter.with(|r| r.text(&f.field, &f.value, true, "ctrl+a"));
                }
            }
        });
    });
    unsafe { CallNextHookEx(0, code, message, data) }
}

struct FocusedEdit {
    field: String,
    value: String,
    safe: bool,
    secure: bool,
}

unsafe fn focused_edit() -> Option<FocusedEdit> {
    let mut info = GuiInfo {
        size: std::mem::size_of::<GuiInfo>() as u32,
        ..GuiInfo::default()
    };
    if GetGUIThreadInfo(0, &mut info) == 0 || info.focus == 0 {
        return None;
    }
    let mut class = [0_u16; 256];
    let n = GetClassNameW(info.focus, class.as_mut_ptr(), class.len() as i32);
    let class = String::from_utf16_lossy(&class[..n.max(0) as usize]).to_lowercase();
    let editable = class == "edit" || class.starts_with("richedit");
    let secure = editable && GetWindowLongPtrW(info.focus, -16) & 0x20 != 0;
    let safe = editable && !secure;
    let mut value = String::new();
    if safe {
        let mut count = 0;
        if SendMessageTimeoutW(info.focus, 0xe, 0, 0, 2, 100, &mut count) == 0 || count > 262144 {
            return None;
        }
        let mut text = vec![0_u16; count + 1];
        let mut n = 0;
        if SendMessageTimeoutW(
            info.focus,
            0xd,
            text.len(),
            text.as_mut_ptr() as isize,
            2,
            100,
            &mut n,
        ) == 0
        {
            return None;
        }
        value = String::from_utf16_lossy(&text[..n.min(count)]);
    }
    Some(FocusedEdit {
        field: format!("win32:{}", info.focus),
        value,
        safe,
        secure,
    })
}

fn key_name(code: u32) -> Option<String> {
    Some(match code {
        0x30..=0x39 | 0x41..=0x5a => char::from_u32(code)?.to_ascii_lowercase().to_string(),
        0x70..=0x7b => format!("f{}", code - 0x6f),
        0x08 => "backspace".into(),
        0x09 => "tab".into(),
        0x0d => "enter".into(),
        0x1b => "esc".into(),
        0x20 => "space".into(),
        0x21 => "pageup".into(),
        0x22 => "pagedown".into(),
        0x23 => "end".into(),
        0x24 => "home".into(),
        0x25 => "left".into(),
        0x26 => "up".into(),
        0x27 => "right".into(),
        0x28 => "down".into(),
        0x2e => "delete".into(),
        _ => return None,
    })
}
