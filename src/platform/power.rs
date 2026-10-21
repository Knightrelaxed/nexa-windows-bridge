// ============================================================
// N.E.X.A 3.3 — WINDOWS POWER BROADCAST LISTENER
// Detects Laptop Sleep & Wake Events (ThinkPad Lid Close/Open)
// ============================================================

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use tracing::{error, info};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::Power::{
    RegisterSuspendResumeNotification, UnregisterSuspendResumeNotification, HPOWERNOTIFY,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, PostMessageW,
    PostQuitMessage, RegisterClassW, TranslateMessage, MSG, WM_DESTROY,
    WM_POWERBROADCAST, WM_USER, WNDCLASSW,
};

const DEVICE_NOTIFY_WINDOW_HANDLE: u32 = 0;

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
static GLOBAL_CALLBACK: Mutex<Option<Box<dyn Fn(PowerStateEvent) + Send + Sync>>> = Mutex::new(None);

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
            {
                let mut cb = GLOBAL_CALLBACK.lock().unwrap();
                *cb = Some(Box::new(callback));
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

                // Create an invisible top-level window (parent = 0 so it receives OS broadcast messages)
                let hwnd = CreateWindowExW(
                    0,
                    class_name.as_ptr(),
                    class_name.as_ptr(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    0 as _, // NULL parent -> Top-level window
                    0 as _,
                    0 as _,
                    std::ptr::null(),
                );

                if hwnd.is_null() {
                    error!("[POWER] Failed to create window for power listener");
                    let _ = hwnd_tx.send(None);
                    return;
                }

                // Modern Windows 10/11: Register for Suspend/Resume notifications
                let hnotify: HPOWERNOTIFY = RegisterSuspendResumeNotification(
                    hwnd as _,
                    DEVICE_NOTIFY_WINDOW_HANDLE,
                );
                if hnotify == 0 {
                    tracing::warn!("[POWER] RegisterSuspendResumeNotification returned 0, falling back to standard broadcast");
                } else {
                    info!("[POWER] Successfully registered with Windows Suspend/Resume Notification system.");
                }

                info!("[POWER] Registered WM_POWERBROADCAST listener window successfully.");
                let _ = hwnd_tx.send(Some(hwnd as isize));

                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, 0 as _, 0, 0) > 0 {
                    if msg.message == WM_STOP_LISTENER {
                        if hnotify != 0 {
                            let _ = UnregisterSuspendResumeNotification(hnotify);
                        }
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
        if let Ok(mut guard) = GLOBAL_CALLBACK.lock() {
            *guard = None;
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
            let event = if event_type == PBT_APMSUSPEND {
                info!("[POWER] ⚡ WM_POWERBROADCAST: PBT_APMSUSPEND (Laptop Sleep / Lid Closed)");
                Some(PowerStateEvent::Suspend)
            } else if event_type == PBT_APMRESUMEAUTOMATIC || event_type == PBT_APMRESUMESUSPEND {
                info!("[POWER] ⚡ WM_POWERBROADCAST: PBT_APMRESUME (Laptop Woken / Lid Opened)");
                Some(PowerStateEvent::Resume)
            } else {
                None
            };

            if let Some(evt) = event {
                if let Ok(guard) = GLOBAL_CALLBACK.lock() {
                    if let Some(ref cb) = *guard {
                        cb(evt);
                    }
                }
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
