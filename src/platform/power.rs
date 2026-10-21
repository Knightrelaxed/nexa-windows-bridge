// ============================================================
// N.E.X.A 3.3 — WINDOWS POWER BROADCAST LISTENER
// Detects Laptop Sleep & Wake Events (ThinkPad Lid Close/Open)
// ============================================================

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use tracing::{error, info};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, PostMessageW,
    PostQuitMessage, RegisterClassW, TranslateMessage, HWND_MESSAGE, MSG, WM_DESTROY,
    WM_POWERBROADCAST, WM_USER, WNDCLASSW,
};

// Standard Win32 WM_POWERBROADCAST event codes
const PBT_APMSUSPEND: u32 = 0x0004;
const PBT_APMRESUMESUSPEND: u32 = 0x0007;
const PBT_APMRESUMEAUTOMATIC: u32 = 0x0012;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerStateEvent {
    Suspend,
    Resume,
}

static LISTENER_ACTIVE: AtomicBool = AtomicBool::new(false);
static mut GLOBAL_CALLBACK: Option<Box<dyn Fn(PowerStateEvent) + Send + Sync>> = None;

const WM_STOP_LISTENER: u32 = WM_USER + 101;

pub struct PowerListener {
    hwnd: Option<isize>,
    thread_handle: Option<thread::JoinHandle<()>>,
}

impl PowerListener {
    pub fn spawn<F>(callback: F) -> Self
    where
        F: Fn(PowerStateEvent) + Send + Sync + 'static,
    {
        let (hwnd_tx, hwnd_rx) = std::sync::mpsc::channel::<Option<isize>>();

        let handle = thread::spawn(move || {
            unsafe {
                GLOBAL_CALLBACK = Some(Box::new(callback));
            }
            LISTENER_ACTIVE.store(true, Ordering::SeqCst);

            let class_name: Vec<u16> = "NexaPowerListenerClass\0".encode_utf16().collect();

            unsafe {
                let wnd_class = WNDCLASSW {
                    style: 0,
                    lpfnWndProc: Some(power_wnd_proc),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: 0 as _,
                    hIcon: 0 as _,
                    hCursor: 0 as _,
                    hbrBackground: 0 as _,
                    lpszMenuName: std::ptr::null(),
                    lpszClassName: class_name.as_ptr(),
                };

                let _ = RegisterClassW(&wnd_class);

                // Create a message-only window attached to HWND_MESSAGE
                let hwnd = CreateWindowExW(
                    0,
                    class_name.as_ptr(),
                    class_name.as_ptr(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    HWND_MESSAGE,
                    0 as _,
                    0 as _,
                    std::ptr::null(),
                );

                if hwnd.is_null() {
                    error!("[POWER] Failed to create message-only window for power listener");
                    let _ = hwnd_tx.send(None);
                    return;
                }

                info!("[POWER] Registered WM_POWERBROADCAST listener window successfully.");
                let _ = hwnd_tx.send(Some(hwnd as isize));

                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, 0 as _, 0, 0) > 0 {
                    if msg.message == WM_STOP_LISTENER {
                        DestroyWindow(hwnd);
                        break;
                    }
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }

            LISTENER_ACTIVE.store(false, Ordering::SeqCst);
            info!("[POWER] Power listener thread exited cleanly.");
        });

        let hwnd = hwnd_rx.recv().unwrap_or(None);

        Self {
            hwnd,
            thread_handle: Some(handle),
        }
    }
}

impl Drop for PowerListener {
    fn drop(&mut self) {
        if let Some(hwnd) = self.hwnd {
            unsafe {
                PostMessageW(hwnd as HWND, WM_STOP_LISTENER, 0, 0);
            }
        }
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

unsafe extern "system" fn power_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_POWERBROADCAST => {
            let event_type = wparam as u32;
            if event_type == PBT_APMSUSPEND {
                info!("[POWER] ⚡ WM_POWERBROADCAST: PBT_APMSUSPEND (Laptop Sleep / Lid Closed)");
                if let Some(ref cb) = GLOBAL_CALLBACK {
                    cb(PowerStateEvent::Suspend);
                }
                return 1;
            } else if event_type == PBT_APMRESUMEAUTOMATIC || event_type == PBT_APMRESUMESUSPEND {
                info!("[POWER] ⚡ WM_POWERBROADCAST: PBT_APMRESUME (Laptop Woken / Lid Opened)");
                if let Some(ref cb) = GLOBAL_CALLBACK {
                    cb(PowerStateEvent::Resume);
                }
                return 1;
            }
            1
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_power_listener_spawn_and_drop() {
        let listener = PowerListener::spawn(|_| {});
        assert!(listener.hwnd.is_some());
        drop(listener);
    }
}
