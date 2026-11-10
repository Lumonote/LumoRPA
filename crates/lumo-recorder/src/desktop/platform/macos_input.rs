//! Listen-only Quartz event tap. Every capture owns its CFRunLoop and releases
//! the tap, source and callback context before `stop` returns.

use std::ffi::{c_char, c_void, CString};
use std::ptr;
use std::sync::mpsc;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::Duration;

use crate::desktop::input::{CaptureGuard, InputEmitter};

type Ref = *const c_void;
type Event = *mut c_void;
#[repr(C)]
struct Point {
    x: f64,
    y: f64,
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        mask: u64,
        callback: extern "C" fn(Ref, u32, Event, *mut c_void) -> Event,
        user: *mut c_void,
    ) -> Ref;
    fn CGEventTapEnable(tap: Ref, enable: bool);
    fn CGEventGetLocation(event: Event) -> Point;
    fn CGEventGetIntegerValueField(event: Event, field: u32) -> i64;
    fn CGEventGetFlags(event: Event) -> u64;
    fn AXUIElementCreateSystemWide() -> Ref;
    fn AXUIElementCopyAttributeValue(element: Ref, attribute: Ref, value: *mut Ref) -> i32;
    fn AXUIElementGetPid(element: Ref, pid: *mut i32) -> i32;
    fn AXUIElementSetMessagingTimeout(element: Ref, timeout_seconds: f32) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFRunLoopCommonModes: Ref;
    static kCFRunLoopDefaultMode: Ref;
    fn CFMachPortCreateRunLoopSource(allocator: Ref, port: Ref, order: i64) -> Ref;
    fn CFMachPortInvalidate(port: Ref);
    fn CFRunLoopGetCurrent() -> Ref;
    fn CFRunLoopAddSource(run_loop: Ref, source: Ref, mode: Ref);
    fn CFRunLoopRunInMode(mode: Ref, seconds: f64, return_after_source: bool) -> i32;
    fn CFRelease(object: Ref);
    fn CFRetain(object: Ref) -> Ref;
    fn CFGetTypeID(object: Ref) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFStringGetLength(string: Ref) -> isize;
    fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
    fn CFStringCreateWithCString(allocator: Ref, text: *const c_char, encoding: u32) -> Ref;
    fn CFStringGetCString(string: Ref, buffer: *mut c_char, size: isize, encoding: u32) -> bool;
}

struct Context {
    emitter: InputEmitter,
    tap: Ref,
}

pub fn start(emitter: InputEmitter) -> anyhow::Result<CaptureGuard> {
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let running = Arc::new(AtomicBool::new(true));
    let worker_running = running.clone();
    let worker = thread::Builder::new().name("lumo-input-macos".into()).spawn(move || unsafe {
        let mut context = Box::new(Context { emitter, tap: ptr::null() });
        let mask = [1, 2, 3, 4, 10, 11, 22, 25, 26].iter().fold(0_u64, |m, n| m | 1_u64 << n);
        let tap = CGEventTapCreate(1, 0, 1, mask, callback, (&mut *context as *mut Context).cast());
        if tap.is_null() {
            let _ = ready_tx.send(Err("Desktop recording requires macOS Input Monitoring and Accessibility permission. Enable LumoRPA in System Settings → Privacy & Security, then retry."));
            return;
        }
        context.tap = tap;
        let source = CFMachPortCreateRunLoopSource(ptr::null(), tap, 0);
        if source.is_null() {
            CFMachPortInvalidate(tap);
            CFRelease(tap);
            let _ = ready_tx.send(Err("Could not attach the desktop input event source"));
            return;
        }
        let run_loop = CFRunLoopGetCurrent();
        CFRetain(run_loop);
        CFRunLoopAddSource(run_loop, source, kCFRunLoopCommonModes);
        CGEventTapEnable(tap, true);
            if ready_tx.send(Ok(())).is_ok() {
                while worker_running.load(Ordering::Acquire) {
                    CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.05, false);
                }
        }
        CGEventTapEnable(tap, false);
        CFMachPortInvalidate(tap);
        CFRelease(source);
        CFRelease(tap);
        CFRelease(run_loop);
    })?;
    match ready_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => {}
        Ok(Err(message)) => {
            let _ = worker.join();
            anyhow::bail!(message);
        }
        Err(error) => {
            running.store(false, Ordering::Release);
            anyhow::bail!("Desktop input capture did not initialize: {error}");
        }
    }
    Ok(CaptureGuard::new(move || {
        running.store(false, Ordering::Release);
        let _ = worker.join();
    }))
}

extern "C" fn callback(_: Ref, event_type: u32, event: Event, user: *mut c_void) -> Event {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let context = &*(user as *const Context);
        if event_type == u32::MAX || event_type == u32::MAX - 1 {
            CGEventTapEnable(context.tap, true);
            return;
        }
        let now = chrono::Utc::now().timestamp_millis();
        match event_type {
            1 | 3 | 25 => {
                let p = CGEventGetLocation(event);
                let button = button_name(event_type, event);
                context.emitter.with(|r| {
                    r.pointer_down(button, p.x, p.y, now);
                    None
                });
            }
            2 | 4 | 26 => {
                let p = CGEventGetLocation(event);
                let button = button_name(event_type, event);
                context
                    .emitter
                    .with(|r| r.pointer_up(button, p.x, p.y, now));
            }
            22 => {
                let dy = -CGEventGetIntegerValueField(event, 11);
                let dx = -CGEventGetIntegerValueField(event, 12);
                context.emitter.with(|r| r.scroll(dx, dy));
            }
            10 | 11 => {
                let code = CGEventGetIntegerValueField(event, 9);
                let flags = CGEventGetFlags(event);
                let mut mods = Vec::new();
                for (mask, name) in [
                    (1 << 17, "shift"),
                    (1 << 18, "ctrl"),
                    (1 << 19, "alt"),
                    (1 << 20, "cmd"),
                ] {
                    if flags & mask != 0 {
                        mods.push(name);
                    }
                }
                let focused = focused_control();
                let safe = focused.as_ref().is_some_and(|c| c.safe);
                if event_type == 10 {
                    if let Some(key) = key_name(code) {
                        // Unknown controls may be password entry widgets. Only
                        // non-text navigation/shortcuts can pass without a role.
                        let secure = focused.as_ref().is_none_or(|c| c.secure);
                        context.emitter.with(|r| r.key(key, &mods, secure));
                    }
                    if let Some(c) = focused.filter(|c| c.safe) {
                        context.emitter.with(|r| {
                            r.baseline_text(&c.field, &c.value, true);
                            None
                        });
                    }
                } else if safe {
                    if let Some(c) = focused {
                        context
                            .emitter
                            .with(|r| r.text(&c.field, &c.value, c.safe, "cmd+a"));
                    }
                }
            }
            _ => {}
        }
    }));
    event
}

unsafe fn button_name(event_type: u32, event: Event) -> &'static str {
    match event_type {
        1 | 2 => "left",
        3 | 4 => "right",
        _ if CGEventGetIntegerValueField(event, 3) == 2 => "middle",
        _ => "unsupported",
    }
}

struct FocusedControl {
    field: String,
    value: String,
    safe: bool,
    secure: bool,
}

unsafe fn focused_control() -> Option<FocusedControl> {
    let root = AXUIElementCreateSystemWide();
    if root.is_null() {
        return None;
    }
    // A hung application must not pin the native event callback or prevent a
    // recorder stop. Unknown/timeout controls are omitted, never keylogged.
    AXUIElementSetMessagingTimeout(root, 0.1);
    let element = copy_attribute(root, "AXFocusedUIElement");
    CFRelease(root);
    let element = element?;
    AXUIElementSetMessagingTimeout(element, 0.1);
    let role = string_attribute(element, "AXRole").unwrap_or_default();
    let subrole = string_attribute(element, "AXSubrole").unwrap_or_default();
    let name = string_attribute(element, "AXTitle")
        .or_else(|| string_attribute(element, "AXDescription"))
        .unwrap_or_default();
    let identifier = string_attribute(element, "AXIdentifier").unwrap_or_default();
    let secure = format!("{role} {subrole} {name}").to_lowercase();
    let secure = ["secure", "password", "passwd", "密码", "口令"]
        .iter()
        .any(|s| secure.contains(s));
    let safe = !secure && matches!(role.as_str(), "AXTextField" | "AXTextArea" | "AXComboBox");
    let value = if safe {
        string_attribute(element, "AXValue").unwrap_or_default()
    } else {
        String::new()
    };
    let mut pid = 0;
    AXUIElementGetPid(element, &mut pid);
    CFRelease(element);
    Some(FocusedControl {
        field: format!("{pid}:{role}:{identifier}:{name}"),
        value,
        safe,
        secure,
    })
}

unsafe fn copy_attribute(element: Ref, name: &str) -> Option<Ref> {
    let name = CString::new(name).ok()?;
    let name = CFStringCreateWithCString(ptr::null(), name.as_ptr(), 0x0800_0100);
    let mut value = ptr::null();
    let result = AXUIElementCopyAttributeValue(element, name, &mut value);
    CFRelease(name);
    (result == 0 && !value.is_null()).then_some(value)
}

unsafe fn string_attribute(element: Ref, name: &str) -> Option<String> {
    let value = copy_attribute(element, name)?;
    if CFGetTypeID(value) != CFStringGetTypeID() {
        CFRelease(value);
        return None;
    }
    let capacity = CFStringGetMaximumSizeForEncoding(CFStringGetLength(value), 0x0800_0100);
    if !(0..=1_048_576).contains(&capacity) {
        CFRelease(value);
        return None;
    }
    let mut buffer = vec![0_u8; capacity as usize + 1];
    let valid = CFStringGetCString(
        value,
        buffer.as_mut_ptr().cast(),
        buffer.len() as isize,
        0x0800_0100,
    );
    CFRelease(value);
    if !valid {
        return None;
    }
    let end = buffer.iter().position(|b| *b == 0)?;
    String::from_utf8(buffer[..end].to_vec()).ok()
}

fn key_name(code: i64) -> Option<&'static str> {
    Some(match code {
        0 => "a",
        1 => "s",
        2 => "d",
        3 => "f",
        4 => "h",
        5 => "g",
        6 => "z",
        7 => "x",
        8 => "c",
        9 => "v",
        11 => "b",
        12 => "q",
        13 => "w",
        14 => "e",
        15 => "r",
        16 => "y",
        17 => "t",
        18 => "1",
        19 => "2",
        20 => "3",
        21 => "4",
        22 => "6",
        23 => "5",
        25 => "9",
        26 => "7",
        28 => "8",
        29 => "0",
        31 => "o",
        32 => "u",
        34 => "i",
        35 => "p",
        36 => "enter",
        37 => "l",
        38 => "j",
        40 => "k",
        45 => "n",
        46 => "m",
        48 => "tab",
        49 => "space",
        51 => "backspace",
        53 => "esc",
        96 => "f5",
        97 => "f6",
        98 => "f7",
        99 => "f3",
        100 => "f8",
        101 => "f9",
        103 => "f11",
        109 => "f10",
        111 => "f12",
        115 => "home",
        116 => "pageup",
        117 => "delete",
        118 => "f4",
        119 => "end",
        120 => "f2",
        121 => "pagedown",
        122 => "f1",
        123 => "left",
        124 => "right",
        125 => "down",
        126 => "up",
        _ => return None,
    })
}
