//! Win32 Single-Instance Mutex & Instance Arbiter (Milestone 7.4)
//!
//! Ensures only one instance of N.E.X.A Windows Bridge runs at any given time.
//! If a second instance is launched (e.g. from shortcut or Start Menu),
//! it signals the existing instance to bring its window to foreground and terminates cleanly.

#![allow(dead_code)]

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use tracing::{info, warn};
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE,
};

const DEFAULT_MUTEX_NAME: &str = "Local\\NexaWindowsBridge_SingleInstance_Mutex";

/// RAII Guard holding the single instance Win32 kernel mutex
pub struct SingleInstanceGuard {
    handle: HANDLE,
    is_unique: bool,
}

impl SingleInstanceGuard {
    /// Try to acquire the single instance lock using the default mutex name
    pub fn acquire() -> Self {
        Self::acquire_with_name(DEFAULT_MUTEX_NAME)
    }

    /// Try to acquire single instance lock with a specific mutex name (useful for testing)
    pub fn acquire_with_name(name: &str) -> Self {
        unsafe {
            let wide_name: Vec<u16> = OsStr::new(name)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();

            let handle = CreateMutexW(std::ptr::null(), 1, wide_name.as_ptr());
            if handle.is_null() {
                warn!("[INSTANCE] CreateMutexW returned NULL, proceeding with caution");
                return Self {
                    handle: std::ptr::null_mut(),
                    is_unique: true,
                };
            }

            let last_error = GetLastError();
            if last_error == ERROR_ALREADY_EXISTS {
                info!("[INSTANCE] Another instance is already running! Focusing existing instance...");

                // Attempt to bring existing N.E.X.A Bridge window to the foreground
                let title: Vec<u16> = OsStr::new("N.E.X.A Bridge")
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
                    is_unique: false,
                };
            }

            info!("[INSTANCE] Single instance lock acquired successfully ({})", name);
            Self {
                handle,
                is_unique: true,
            }
        }
    }

    /// Check if this instance is the primary/unique running instance
    pub fn is_unique(&self) -> bool {
        self.is_unique
    }
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                CloseHandle(self.handle);
            }
            self.handle = std::ptr::null_mut();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_instance_acquisition_and_duplication() {
        let unique_name = format!("Local\\NexaTestMutex_{}", std::process::id());
        let guard1 = SingleInstanceGuard::acquire_with_name(&unique_name);
        assert!(guard1.is_unique());

        // A second acquisition on the same named mutex must report not unique
        let guard2 = SingleInstanceGuard::acquire_with_name(&unique_name);
        assert!(!guard2.is_unique());

        // Dropping guard1 releases the mutex
        drop(guard1);
        drop(guard2);

        // Subsequent acquisition succeeds
        let guard3 = SingleInstanceGuard::acquire_with_name(&unique_name);
        assert!(guard3.is_unique());
    }
}
