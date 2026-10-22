// ============================================================
// WINDOWS SYSTEM TRAY CONTROLLER
// Zero-RAM Win32 Shell_NotifyIconW Implementation with Context Menu
// ============================================================

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::thread;
use tracing::{error, info, warn};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, LoadIconW, PostMessageW, RegisterClassW,
    RegisterWindowMessageW, SetForegroundWindow, TrackPopupMenu, TranslateMessage,
    IDI_APPLICATION, MF_SEPARATOR, MF_STRING, MSG, TPM_BOTTOMALIGN, TPM_RIGHTBUTTON,
    WM_COMMAND, WM_DESTROY, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WM_USER, WNDCLASSW,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    ToggleWindow,      // Left click on tray icon
    ShowWindow,        // Context Menu "Buka N.E.X.A" (ID 1001)
    ToggleMute,        // Context Menu "Mute / Unmute" (ID 1002)
    ToggleFullscreen,  // Context Menu "Layar Penuh (F11)" (ID 1003)
    ToggleScreenShare, // Context Menu "Berbagi Layar (Win+Alt+S)" (ID 1006)
    OpenSettings,      // Context Menu "Pengaturan" (ID 1004)
    Quit,              // Context Menu "Keluar" (ID 1005)
}

type TrayCallback = Box<dyn Fn(TrayEvent) + Send + Sync>;
static TRAY_CALLBACK: Mutex<Option<TrayCallback>> = Mutex::new(None);
static TRAY_ACTIVE: AtomicBool = AtomicBool::new(false);
static WM_TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

const WM_TRAYICON: u32 = WM_USER + 201;
const WM_STOP_TRAY: u32 = WM_USER + 202;

const MENU_OPEN: usize = 1001;
const MENU_MUTE: usize = 1002;
const MENU_FULLSCREEN: usize = 1003;
const MENU_SETTINGS: usize = 1004;
const MENU_QUIT: usize = 1005;
const MENU_SCREEN_SHARE: usize = 1006;

pub struct SystemTray {
    hwnd: Option<isize>,
    thread_handle: Option<thread::JoinHandle<()>>,
}

impl SystemTray {
    pub fn spawn<F>(callback: F) -> Self
    where
        F: Fn(TrayEvent) + Send + Sync + 'static,
    {
        let (hwnd_tx, hwnd_rx) = std::sync::mpsc::channel::<Option<isize>>();

        let handle = thread::spawn(move || {
            {
                let mut cb = TRAY_CALLBACK.lock().unwrap();
                *cb = Some(Box::new(callback));
            }
            TRAY_ACTIVE.store(true, Ordering::SeqCst);

            let class_name: Vec<u16> = "NexaSystemTrayClass\0".encode_utf16().collect();

            unsafe {
                let hinstance = GetModuleHandleW(std::ptr::null());
                let app_icon = LoadIconW(hinstance, 1 as _);
                let icon = if !app_icon.is_null() {
                    app_icon
                } else {
                    LoadIconW(0 as _, IDI_APPLICATION)
                };

                let wnd_class = WNDCLASSW {
                    style: 0,
                    lpfnWndProc: Some(tray_wnd_proc),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: hinstance,
                    hIcon: icon,
                    hCursor: 0 as _,
                    hbrBackground: 0 as _,
                    lpszMenuName: std::ptr::null(),
                    lpszClassName: class_name.as_ptr(),
                };

                let _ = RegisterClassW(&wnd_class);

                let hwnd = CreateWindowExW(
                    0,
                    class_name.as_ptr(),
                    class_name.as_ptr(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    0 as _,
                    0 as _,
                    hinstance,
                    std::ptr::null(),
                );

                if hwnd.is_null() {
                    error!("[TRAY] Failed to create hidden window for system tray");
                    let _ = hwnd_tx.send(None);
                    return;
                }

                // Register TaskbarCreated message to handle Explorer shell restarts
                let taskbar_created_str: Vec<u16> = "TaskbarCreated\0".encode_utf16().collect();
                let taskbar_msg = RegisterWindowMessageW(taskbar_created_str.as_ptr());
                WM_TASKBAR_CREATED.store(taskbar_msg, Ordering::SeqCst);

                // Configure NOTIFYICONDATAW
                let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
                nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
                nid.hWnd = hwnd;
                nid.uID = 1001;
                nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
                nid.uCallbackMessage = WM_TRAYICON;
                nid.hIcon = icon;

                let tip_str = "Nexa Bridge\0";
                let tip_encoded: Vec<u16> = tip_str.encode_utf16().collect();
                let tip_len = tip_encoded.len().min(nid.szTip.len());
                nid.szTip[..tip_len].copy_from_slice(&tip_encoded[..tip_len]);

                let add_res = Shell_NotifyIconW(NIM_ADD, &nid);
                if add_res != 0 {
                    info!("[TRAY] ✅ System tray icon registered successfully (0 MB RAM overhead).");
                } else {
                    warn!("[TRAY] Shell_NotifyIconW returned 0 (Shell_TrayWnd not yet available or running in headless session).");
                }

                let _ = hwnd_tx.send(Some(hwnd as isize));

                // Win32 message pump
                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, 0 as _, 0, 0) > 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }

                // Remove icon cleanly on exit
                if add_res != 0 {
                    Shell_NotifyIconW(NIM_DELETE, &nid);
                }
                info!("[TRAY] System tray message loop terminated cleanly.");
            }
        });

        let hwnd = hwnd_rx.recv().unwrap_or(None);

        Self {
            hwnd,
            thread_handle: Some(handle),
        }
    }
}

impl Drop for SystemTray {
    fn drop(&mut self) {
        TRAY_ACTIVE.store(false, Ordering::SeqCst);
        if let Some(hwnd) = self.hwnd {
            unsafe {
                PostMessageW(hwnd as HWND, WM_STOP_TRAY, 0, 0);
            }
        }
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

unsafe extern "system" fn tray_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let taskbar_created = WM_TASKBAR_CREATED.load(Ordering::Relaxed);
    if taskbar_created != 0 && msg == taskbar_created {
        // Taskbar recreated (Explorer restart) -> re-add tray icon
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = 1001;
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        nid.uCallbackMessage = WM_TRAYICON;
        nid.hIcon = LoadIconW(0 as _, IDI_APPLICATION);
        let tip_str = "Nexa Bridge\0";
        let tip_encoded: Vec<u16> = tip_str.encode_utf16().collect();
        let tip_len = tip_encoded.len().min(nid.szTip.len());
        nid.szTip[..tip_len].copy_from_slice(&tip_encoded[..tip_len]);
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
        return 0;
    }

    match msg {
        WM_TRAYICON => {
            let event_type = lparam as u32;
            match event_type {
                WM_LBUTTONUP => {
                    if let Ok(cb) = TRAY_CALLBACK.lock() {
                        if let Some(ref callback) = *cb {
                            callback(TrayEvent::ToggleWindow);
                        }
                    }
                }
                WM_RBUTTONUP => {
                    show_context_menu(hwnd);
                }
                _ => {}
            }
            0
        }
        WM_COMMAND => {
            let id = wparam & 0xFFFF;
            let tray_event = match id {
                MENU_OPEN => Some(TrayEvent::ShowWindow),
                MENU_MUTE => Some(TrayEvent::ToggleMute),
                MENU_FULLSCREEN => Some(TrayEvent::ToggleFullscreen),
                MENU_SCREEN_SHARE => Some(TrayEvent::ToggleScreenShare),
                MENU_SETTINGS => Some(TrayEvent::OpenSettings),
                MENU_QUIT => Some(TrayEvent::Quit),
                _ => None,
            };

            if let Some(event) = tray_event {
                if let Ok(cb) = TRAY_CALLBACK.lock() {
                    if let Some(ref callback) = *cb {
                        callback(event);
                    }
                }
            }
            0
        }
        WM_STOP_TRAY => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            windows_sys::Win32::UI::WindowsAndMessaging::PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn show_context_menu(hwnd: HWND) {
    let mut pt: POINT = std::mem::zeroed();
    GetCursorPos(&mut pt);
    SetForegroundWindow(hwnd);

    let hmenu = CreatePopupMenu();
    if hmenu.is_null() {
        return;
    }

    let item_open: Vec<u16> = "👁️ Open Nexa Bridge\0".encode_utf16().collect();
    let item_mute: Vec<u16> = "🎙️ Mute / Unmute Microphone\0".encode_utf16().collect();
    let item_screen: Vec<u16> = "🖥️ Toggle Screen Share (Win+Alt+S)\0".encode_utf16().collect();
    let item_fs: Vec<u16> = "⛶ Fullscreen / Windowed (F11)\0".encode_utf16().collect();
    let item_settings: Vec<u16> = "⚙ Settings (~/.nexa-bridge.json)\0".encode_utf16().collect();
    let item_quit: Vec<u16> = "❌ Quit\0".encode_utf16().collect();

    AppendMenuW(hmenu, MF_STRING, MENU_OPEN, item_open.as_ptr());
    AppendMenuW(hmenu, MF_STRING, MENU_MUTE, item_mute.as_ptr());
    AppendMenuW(hmenu, MF_STRING, MENU_SCREEN_SHARE, item_screen.as_ptr());
    AppendMenuW(hmenu, MF_STRING, MENU_FULLSCREEN, item_fs.as_ptr());
    AppendMenuW(hmenu, MF_STRING, MENU_SETTINGS, item_settings.as_ptr());
    AppendMenuW(hmenu, MF_SEPARATOR, 0, std::ptr::null());
    AppendMenuW(hmenu, MF_STRING, MENU_QUIT, item_quit.as_ptr());

    TrackPopupMenu(
        hmenu,
        TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
        pt.x,
        pt.y,
        0,
        hwnd,
        std::ptr::null(),
    );

    PostMessageW(hwnd, WM_NULL, 0, 0);
    DestroyMenu(hmenu);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_tray_spawn_and_drop() {
        let tray = SystemTray::spawn(|event| {
            println!("Tray event received: {:?}", event);
        });
        assert!(tray.hwnd.is_some());
        assert!(TRAY_ACTIVE.load(Ordering::SeqCst));
        drop(tray);
        assert!(!TRAY_ACTIVE.load(Ordering::SeqCst));
    }
}
