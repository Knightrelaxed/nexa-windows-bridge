//! Memory Working Set Trimmer & Lean Buffer Optimization (Milestone 7.3)
//!
//! Provides Win32 Process Working Set Trimming via SetProcessWorkingSetSize.
//! Flushes unused memory pages back to OS standby/free pool when N.E.X.A is minimized
//! to the system tray, after stopping screen capture, or during idle standby.

#![allow(dead_code)]

use tracing::{debug, info};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};

/// Trim the working set of the current process, returning unreferenced physical RAM to Windows.
/// Uses the canonical Win32 pattern: SetProcessWorkingSetSize(GetCurrentProcess(), (SIZE_T)-1, (SIZE_T)-1).
pub fn trim_working_set() -> bool {
    unsafe {
        let handle = GetCurrentProcess();
        let success = SetProcessWorkingSetSize(handle, usize::MAX, usize::MAX);
        if success != 0 {
            debug!("[MEMORY] 🧹 Process working set trimmed successfully (RAM flushed to standby pool)");
            true
        } else {
            debug!("[MEMORY] SetProcessWorkingSetSize returned 0");
            false
        }
    }
}

/// Helper to trigger memory compaction if low_ram_mode is enabled
pub fn compact_memory_if_low_ram(low_ram_mode: bool) {
    if low_ram_mode {
        trim_working_set();
        info!("[MEMORY] ⚡ Low-RAM Mode: Memory compacted");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trim_working_set_execution() {
        // Safe to call in test environment; does not panic
        let _ = trim_working_set();
    }

    #[test]
    fn test_compact_memory_if_low_ram() {
        compact_memory_if_low_ram(true);
        compact_memory_if_low_ram(false);
    }
}
