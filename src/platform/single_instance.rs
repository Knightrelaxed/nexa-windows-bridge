//! Win32 Single-Instance Mutex & Instance Arbiter (Milestone 7.4)
//!
//! Ensures only one instance of N.E.X.A Windows Bridge runs at any given time.
//! If a second instance is launched (e.g. from shortcut or Start Menu),
//! it signals the existing instance to bring its window to foreground and terminates cleanly.

#![allow(dead_code)]

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use tracing::{info, warn};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE,
};

const DEFAULT_MUTEX_NAME: &str = "Local\\NexaWindowsBridge_SingleInstance_Mutex";
const DEFAULT_WAKE_EVENT: &str = "Local\\NexaWindowsBridge_WakeEvent";

/// RAII Guard holding the single instance Win32 kernel mutex & wake event
pub struct SingleInstanceGuard {
    handle: HANDLE,
    wake_event: HANDLE,
    is_unique: bool,
}

impl SingleInstanceGuard {
    /// Try to acquire the single instance lock using the default mutex name
    pub fn acquire() -> Self {
        Self::acquire_with_name(DEFAULT_MUTEX_NAME, DEFAULT_WAKE_EVENT)
    }

    /// Try to acquire single instance lock with specific names (useful for testing)
    pub fn acquire_with_name(mutex_name: &str, wake_event_name: &str) -> Self {
        unsafe {
            let wide_name: Vec<u16> = OsStr::new(mutex_name)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();

            let handle = CreateMutexW(std::ptr::null(), 1, wide_name.as_ptr());
            if handle.is_null() {
                warn!("[INSTANCE] CreateMutexW returned NULL, proceeding with caution");
                return Self {
                    handle: std::ptr::null_mut(),
                    wake_event: std::ptr::null_mut(),
                    is_unique: true,
                };
            }

            let last_error = GetLastError();
            if last_error == ERROR_ALREADY_EXISTS {
                info!("[INSTANCE] Another instance is already running! Focusing existing instance...");

                // Signal the wake event to the running primary instance
                let wide_wake: Vec<u16> = OsStr::new(wake_event_name)
                    .encode_wide()
                    .chain(std::iter::once(0))
                    .collect();
                let wake_handle = OpenEventW(EVENT_MODIFY_STATE, 0, wide_wake.as_ptr());
                if !wake_handle.is_null() {
                    SetEvent(wake_handle);
                    CloseHandle(wake_handle);
                }

                // Attempt to bring existing Nexa Bridge window to the foreground
                let title: Vec<u16> = OsStr::new("Nexa Bridge")
                    .encode_wide()
                    .chain(std::iter::once(0))
                    .collect();

                let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
                if !hwnd.is_null() {
                    ShowWindow(hwnd, SW_RESTORE);
                    SetForegroundWindow(hwnd);
                }

                CloseHandle(handle);
                return Self {
                    handle: std::ptr::null_mut(),
                    wake_event: std::ptr::null_mut(),
                    is_unique: false,
                };
            }

            // Primary instance: Create wake event
            let wide_wake: Vec<u16> = OsStr::new(wake_event_name)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let wake_event = CreateEventW(std::ptr::null(), 0, 0, wide_wake.as_ptr());

            info!("[INSTANCE] Single instance lock acquired successfully ({})", mutex_name);
            Self {
                handle,
                wake_event,
                is_unique: true,
            }
        }
    }

    /// Check if this instance is the primary/unique running instance
    pub fn is_unique(&self) -> bool {
        self.is_unique
    }

    /// Spawn background thread to listen for wake signals from secondary instances
    pub fn spawn_wake_listener<F>(&self, on_wake: F)
    where
        F: Fn() + Send + 'static,
    {
        if !self.is_unique || self.wake_event.is_null() {
            return;
        }
        let wake_event = self.wake_event as usize;
        std::thread::spawn(move || {
            let handle = wake_event as HANDLE;
            loop {
                let wait_res = unsafe { WaitForSingleObject(handle, 0xFFFFFFFF) };
                if wait_res == WAIT_OBJECT_0 {
                    info!("[INSTANCE] 🔔 Wake signal received from secondary instance. Restoring window.");
                    on_wake();
                } else {
                    break;
                }
            }
        });
    }
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            if !self.wake_event.is_null() {
                CloseHandle(self.wake_event);
                self.wake_event = std::ptr::null_mut();
            }
            if !self.handle.is_null() {
                CloseHandle(self.handle);
                self.handle = std::ptr::null_mut();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_instance_acquisition_and_duplication() {
        let unique_mutex = format!("Local\\NexaTestMutex_{}", std::process::id());
        let unique_event = format!("Local\\NexaTestWake_{}", std::process::id());
        let guard1 = SingleInstanceGuard::acquire_with_name(&unique_mutex, &unique_event);
        assert!(guard1.is_unique());

        // A second acquisition on the same named mutex must report not unique
        let guard2 = SingleInstanceGuard::acquire_with_name(&unique_mutex, &unique_event);
        assert!(!guard2.is_unique());

        // Dropping guard1 releases the mutex
        drop(guard1);
        drop(guard2);

        // Subsequent acquisition succeeds
        let guard3 = SingleInstanceGuard::acquire_with_name(&unique_mutex, &unique_event);
        assert!(guard3.is_unique());
    }
}
