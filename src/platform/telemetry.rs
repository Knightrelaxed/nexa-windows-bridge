// ============================================================
// WINDOWS SYSTEM TELEMETRY MONITOR
// Zero-RAM Win32 Native CPU & RAM Sampler for J.A.R.V.I.S HUD
// ============================================================

use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::GetSystemTimes;

pub struct SystemTelemetry {
    prev_idle: u64,
    prev_kernel: u64,
    prev_user: u64,
}

impl Default for SystemTelemetry {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemTelemetry {
    pub fn new() -> Self {
        let (idle, kernel, user) = Self::read_system_times();
        Self {
            prev_idle: idle,
            prev_kernel: kernel,
            prev_user: user,
        }
    }

    fn filetime_to_u64(ft: FILETIME) -> u64 {
        ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64)
    }

    fn read_system_times() -> (u64, u64, u64) {
        unsafe {
            let mut idle = std::mem::zeroed();
            let mut kernel = std::mem::zeroed();
            let mut user = std::mem::zeroed();
            if GetSystemTimes(&mut idle, &mut kernel, &mut user) != 0 {
                (
                    Self::filetime_to_u64(idle),
                    Self::filetime_to_u64(kernel),
                    Self::filetime_to_u64(user),
                )
            } else {
                (0, 0, 0)
            }
        }
    }

    /// Read CPU usage percentage (0 to 100) since previous sample
    pub fn sample_cpu_percent(&mut self) -> u32 {
        let (idle, kernel, user) = Self::read_system_times();
        let delta_idle = idle.saturating_sub(self.prev_idle);
        let delta_kernel = kernel.saturating_sub(self.prev_kernel);
        let delta_user = user.saturating_sub(self.prev_user);

        self.prev_idle = idle;
        self.prev_kernel = kernel;
        self.prev_user = user;

        let delta_total = delta_kernel + delta_user;
        if delta_total == 0 {
            return 0;
        }

        let busy = delta_total.saturating_sub(delta_idle);
        ((busy * 100) / delta_total).min(100) as u32
    }

    /// Read physical RAM usage percentage (0 to 100)
    pub fn sample_ram_percent() -> u32 {
        unsafe {
            let mut mem_status: MEMORYSTATUSEX = std::mem::zeroed();
            mem_status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
            if GlobalMemoryStatusEx(&mut mem_status) != 0 {
                mem_status.dwMemoryLoad
            } else {
                0
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_telemetry_samples() {
        let mut telemetry = SystemTelemetry::new();
        let cpu = telemetry.sample_cpu_percent();
        let ram = SystemTelemetry::sample_ram_percent();

        assert!(cpu <= 100);
        assert!(ram <= 100);
        assert!(ram > 0); // Active OS always uses > 0% RAM
    }
}
