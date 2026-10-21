//! Windows Auto-Start Standby Integration (Milestone 7.2)
//!
//! Manages automatic startup registration in Windows via HKCU Registry Run key:
//! HKCU\Software\Microsoft\Windows\CurrentVersion\Run
//! Enables N.E.X.A Bridge to standby quietly in the System Tray when the ThinkPad boots.

#![allow(dead_code)]

use std::env;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use tracing::{debug, info, warn};
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::*;

const RUN_KEY_SUBKEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run\0";
const VALUE_NAME: &str = "NexaWindowsBridge\0";

/// Helper to convert a string slice to a null-terminated UTF-16 vector
fn to_wide_null(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

/// Check if N.E.X.A Bridge is currently registered in Windows Auto-Start (HKCU Run key)
pub fn is_autostart_enabled() -> bool {
    unsafe {
        let subkey = to_wide_null("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
        let val_name = to_wide_null("NexaWindowsBridge");

        let mut hkey: HKEY = std::ptr::null_mut();
        let status = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_READ,
            &mut hkey,
        );

        if status != ERROR_SUCCESS {
            debug!("[AUTOSTART] RegOpenKeyExW failed with code {}", status);
            return false;
        }

        let mut data_type: u32 = 0;
        let mut data_size: u32 = 0;

        let query_res = RegQueryValueExW(
            hkey,
            val_name.as_ptr(),
            std::ptr::null_mut(),
            &mut data_type,
            std::ptr::null_mut(),
            &mut data_size,
        );

        RegCloseKey(hkey);
        query_res == ERROR_SUCCESS && data_size > 0
    }
}

/// Enable or disable N.E.X.A Bridge in Windows Auto-Start
pub fn set_autostart_enabled(enabled: bool) -> Result<(), String> {
    let subkey = to_wide_null("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    let val_name = to_wide_null("NexaWindowsBridge");

    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut();
        let status = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_WRITE | KEY_READ,
            &mut hkey,
        );

        if status != ERROR_SUCCESS {
            return Err(format!("Failed to open HKCU Run key (code {})", status));
        }

        if enabled {
            let current_exe = env::current_exe()
                .map_err(|e| format!("Failed to determine executable path: {}", e))?;
            
            // Format command line with quotes and --autostart flag
            let cmd = format!("\"{}\" --autostart", current_exe.display());
            let wide_cmd = to_wide_null(&cmd);
            let byte_len = (wide_cmd.len() * std::mem::size_of::<u16>()) as u32;

            let set_res = RegSetValueExW(
                hkey,
                val_name.as_ptr(),
                0,
                REG_SZ,
                wide_cmd.as_ptr() as *const u8,
                byte_len,
            );

            RegCloseKey(hkey);

            if set_res == ERROR_SUCCESS {
                info!("[AUTOSTART] 🚀 Registered Auto-Start entry: {}", cmd);
                Ok(())
            } else {
                Err(format!("Failed to set HKCU Run value (code {})", set_res))
            }
        } else {
            let del_res = RegDeleteValueW(hkey, val_name.as_ptr());
            RegCloseKey(hkey);

            if del_res == ERROR_SUCCESS || del_res == 2 { // 2 = ERROR_FILE_NOT_FOUND (already deleted)
                info!("[AUTOSTART] 🛑 Unregistered Auto-Start entry");
                Ok(())
            } else {
                warn!("[AUTOSTART] RegDeleteValueW returned code {}", del_res);
                Err(format!("Failed to delete HKCU Run value (code {})", del_res))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_wide_null() {
        let wide = to_wide_null("N.E.X.A");
        assert_eq!(wide.last(), Some(&0));
        assert_eq!(wide.len(), 8); // 'N', '.', 'E', '.', 'X', '.', 'A', 0
    }

    #[test]
    fn test_is_autostart_query_safe() {
        // Safe check without panics
        let _ = is_autostart_enabled();
    }
}
