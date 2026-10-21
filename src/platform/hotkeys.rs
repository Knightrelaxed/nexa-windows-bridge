// ============================================================
// WINDOWS GLOBAL HOTKEY LISTENER
// Native Win32 RegisterHotKey for Instant Background Shortcuts
// ============================================================

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use tracing::{error, info, warn};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_NOREPEAT, MOD_WIN, VK_F11,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, PostMessageW,
    RegisterClassW, TranslateMessage, MSG, WM_DESTROY, WM_HOTKEY, WM_USER, WNDCLASSW,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    BossKey,           // Win + Alt + N (Toggle Summon / Dismiss Window)
    ToggleMute,        // Win + Alt + M (Mute / Unmute Microphone)
    ToggleFullscreen,  // F11 or Win + Alt + F (Toggle Fullscreen / Windowed)
    ToggleScreenShare, // Win + Alt + S (Toggle Direct3D 11 Screen Sharing)
}

type HotkeyCallback = Box<dyn Fn(HotkeyAction) + Send + Sync>;
static HOTKEY_CALLBACK: Mutex<Option<HotkeyCallback>> = Mutex::new(None);
static HOTKEY_LISTENER_ACTIVE: AtomicBool = AtomicBool::new(false);

const WM_STOP_HOTKEYS: u32 = WM_USER + 301;

const ID_HOTKEY_BOSS: i32 = 1;         // Win + Alt + N
const ID_HOTKEY_MUTE: i32 = 2;         // Win + Alt + M
const ID_HOTKEY_FS_F11: i32 = 3;       // F11
const ID_HOTKEY_FS_WIN_F: i32 = 4;     // Win + Alt + F
const ID_HOTKEY_SCREEN_SHARE: i32 = 5; // Win + Alt + S

pub struct HotkeyListener {
    hwnd: Option<isize>,
    thread_handle: Option<thread::JoinHandle<()>>,
}

impl HotkeyListener {
    pub fn spawn<F>(callback: F) -> Self
    where
        F: Fn(HotkeyAction) + Send + Sync + 'static,
    {
        let (hwnd_tx, hwnd_rx) = std::sync::mpsc::channel::<Option<isize>>();

        let handle = thread::spawn(move || {
            {
                let mut cb = HOTKEY_CALLBACK.lock().unwrap();
                *cb = Some(Box::new(callback));
            }
            HOTKEY_LISTENER_ACTIVE.store(true, Ordering::SeqCst);

            let class_name: Vec<u16> = "NexaHotkeyListenerClass\0".encode_utf16().collect();

            unsafe {
                let hinstance = GetModuleHandleW(std::ptr::null());
                let wnd_class = WNDCLASSW {
                    style: 0,
                    lpfnWndProc: Some(hotkey_wnd_proc),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: hinstance,
                    hIcon: 0 as _,
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
                    error!("[HOTKEYS] Failed to create window for hotkey listener");
                    let _ = hwnd_tx.send(None);
                    return;
                }

                // 1. Boss Key: Win + Alt + N
                let res_boss = RegisterHotKey(
                    hwnd,
                    ID_HOTKEY_BOSS,
                    MOD_WIN | MOD_ALT | MOD_NOREPEAT,
                    'N' as u32,
                );
                if res_boss != 0 {
                    info!("[HOTKEYS] ✅ Registered Boss Key (Win + Alt + N)");
                } else {
                    warn!("[HOTKEYS] ⚠️ Failed to register Boss Key (Win + Alt + N)");
                }

                // 2. Mute Toggle: Win + Alt + M
                let res_mute = RegisterHotKey(
                    hwnd,
                    ID_HOTKEY_MUTE,
                    MOD_WIN | MOD_ALT | MOD_NOREPEAT,
                    'M' as u32,
                );
                if res_mute != 0 {
                    info!("[HOTKEYS] ✅ Registered Mute Toggle (Win + Alt + M)");
                } else {
                    warn!("[HOTKEYS] ⚠️ Failed to register Mute Toggle (Win + Alt + M)");
                }

                // 3. Fullscreen Fallback: Win + Alt + F
                let res_fs_win = RegisterHotKey(
                    hwnd,
                    ID_HOTKEY_FS_WIN_F,
                    MOD_WIN | MOD_ALT | MOD_NOREPEAT,
                    'F' as u32,
                );
                if res_fs_win != 0 {
                    info!("[HOTKEYS] ✅ Registered Fullscreen Toggle (Win + Alt + F)");
                }

                // 4. Global F11 (may be captured by foreground apps, so best-effort)
                let res_fs_f11 = RegisterHotKey(
                    hwnd,
                    ID_HOTKEY_FS_F11,
                    MOD_NOREPEAT,
                    VK_F11 as u32,
                );
                if res_fs_f11 != 0 {
                    info!("[HOTKEYS] ✅ Registered Global F11 Fullscreen");
                }

                // 5. Screen Sharing: Win + Alt + S
                let res_fs_screen = RegisterHotKey(
                    hwnd,
                    ID_HOTKEY_SCREEN_SHARE,
                    MOD_WIN | MOD_ALT | MOD_NOREPEAT,
                    'S' as u32,
                );
                if res_fs_screen != 0 {
                    info!("[HOTKEYS] ✅ Registered Screen Share Toggle (Win + Alt + S)");
                } else {
                    warn!("[HOTKEYS] ⚠️ Failed to register Screen Share Toggle (Win + Alt + S)");
                }

                let _ = hwnd_tx.send(Some(hwnd as isize));

                // Win32 message pump
                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, 0 as _, 0, 0) > 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }

                // Unregister hotkeys on shutdown
                UnregisterHotKey(hwnd, ID_HOTKEY_BOSS);
                UnregisterHotKey(hwnd, ID_HOTKEY_MUTE);
                UnregisterHotKey(hwnd, ID_HOTKEY_FS_WIN_F);
                UnregisterHotKey(hwnd, ID_HOTKEY_FS_F11);
                UnregisterHotKey(hwnd, ID_HOTKEY_SCREEN_SHARE);

                info!("[HOTKEYS] Hotkey listener loop terminated cleanly.");
            }
        });

        let hwnd = hwnd_rx.recv().unwrap_or(None);

        Self {
            hwnd,
            thread_handle: Some(handle),
        }
    }
}

impl Drop for HotkeyListener {
    fn drop(&mut self) {
        HOTKEY_LISTENER_ACTIVE.store(false, Ordering::SeqCst);
        if let Some(hwnd) = self.hwnd {
            unsafe {
                PostMessageW(hwnd as HWND, WM_STOP_HOTKEYS, 0, 0);
            }
        }
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

unsafe extern "system" fn hotkey_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_HOTKEY => {
            let hotkey_id = wparam as i32;
            let action = match hotkey_id {
                ID_HOTKEY_BOSS => Some(HotkeyAction::BossKey),
                ID_HOTKEY_MUTE => Some(HotkeyAction::ToggleMute),
                ID_HOTKEY_FS_F11 | ID_HOTKEY_FS_WIN_F => Some(HotkeyAction::ToggleFullscreen),
                ID_HOTKEY_SCREEN_SHARE => Some(HotkeyAction::ToggleScreenShare),
                _ => None,
            };

            if let Some(act) = action {
                if let Ok(cb) = HOTKEY_CALLBACK.lock() {
                    if let Some(ref callback) = *cb {
                        callback(act);
                    }
                }
            }
            0
        }
        WM_STOP_HOTKEYS => {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hotkey_listener_spawn_and_drop() {
        let listener = HotkeyListener::spawn(|action| {
            println!("Hotkey triggered: {:?}", action);
        });
        assert!(listener.hwnd.is_some());
        assert!(HOTKEY_LISTENER_ACTIVE.load(Ordering::SeqCst));
        drop(listener);
        assert!(!HOTKEY_LISTENER_ACTIVE.load(Ordering::SeqCst));
    }
}
