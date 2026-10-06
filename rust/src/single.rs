//! Single instance: named mutex + named event for "show yourself".

use std::ffi::c_void;
use std::ptr;

const ERROR_ALREADY_EXISTS: u32 = 183;
const EVENT_MODIFY_STATE: u32 = 0x0002;
const WAIT_OBJECT_0: u32 = 0;

#[link(name = "kernel32")]
extern "system" {
    fn CreateMutexW(attrs: *mut c_void, initial_owner: i32, name: *const u16) -> *mut c_void;
    fn CreateEventW(
        attrs: *mut c_void,
        manual_reset: i32,
        initial_state: i32,
        name: *const u16,
    ) -> *mut c_void;
    fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> *mut c_void;
    fn SetEvent(handle: *mut c_void) -> i32;
    fn WaitForSingleObject(handle: *mut c_void, ms: u32) -> u32;
    fn GetLastError() -> u32;
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// Two instances would fight over adapters and routes: the second one only
// asks the first one to show up and exits.
const MUTEX_NAME: &str = "Local\\vpn-rs-instancia-unica";
const EVENT_NAME: &str = "Local\\vpn-rs-mostrar";

/// Windows object name. VPN_INSTANCIA (tests only) separates a test
/// instance from the everyday app open in the same session.
fn nome(base: &str) -> Vec<u16> {
    match std::env::var("VPN_INSTANCIA") {
        Ok(sufixo) if !sufixo.is_empty() => wide(&format!("{base}-{sufixo}")),
        _ => wide(base),
    }
}

/// Returns true if this is the first instance (and keeps the mutex alive for
/// the rest of the process). If another one exists, signals it and returns false.
pub fn acquire_or_signal() -> bool {
    let name = nome(MUTEX_NAME);
    let handle = unsafe { CreateMutexW(ptr::null_mut(), 0, name.as_ptr()) };
    let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    // handle intentionally "leaked": lives until the process ends
    let _ = handle;
    if !already {
        return true;
    }
    let ev_name = nome(EVENT_NAME);
    let ev = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, ev_name.as_ptr()) };
    if !ev.is_null() {
        unsafe { SetEvent(ev) };
    }
    false
}

/// Creates the "show yourself" event and a thread that restores the main window
/// (straight through the Windows API - works even with the UI hidden/paused)
/// whenever a second instance signals.
pub fn spawn_show_listener() {
    let ev_name = nome(EVENT_NAME);
    let ev = unsafe { CreateEventW(ptr::null_mut(), 0, 0, ev_name.as_ptr()) } as usize;
    if ev == 0 {
        return;
    }
    std::thread::spawn(move || loop {
        let r = unsafe { WaitForSingleObject(ev as *mut c_void, 60_000) };
        if r == WAIT_OBJECT_0 {
            crate::show_main_window();
        }
    });
}
