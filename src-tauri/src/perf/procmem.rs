//! Memory of this process, for the stress harness (`perf/stress.rs`) and any
//! future `--perf` sampler. One function, two real implementations: Linux reads
//! `/proc/self/status`, Windows asks `GetProcessMemoryInfo`. Both are compiled on
//! their own platform only because the OS call differs, but the Windows one is
//! type checked from Linux with `cargo check --target x86_64-pc-windows-gnu`.

/// Resident memory of this process now and its peak, in bytes. On Windows these
/// are the working set and the peak working set (the closest thing to what Task
/// Manager shows as memory). They do not include child processes, so the
/// WebView2 processes are not in them: for the "whole process incl. WebView2"
/// budget use Task Manager or the PowerShell line in `docs/perf.md`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rss {
    pub now: u64,
    pub peak: u64,
}

/// Pick a `kB` field out of the text of `/proc/<pid>/status`, in bytes.
/// Pure so it is tested on every platform.
pub fn parse_status_kb(status: &str, field: &str) -> Option<u64> {
    let line = status.lines().find(|l| l.starts_with(field))?;
    let rest = line.strip_prefix(field)?.strip_prefix(':')?;
    let kb: u64 = rest.trim().strip_suffix("kB")?.trim().parse().ok()?;
    kb.checked_mul(1024)
}

/// The memory of this process, or `None` where it cannot be read.
pub fn rss() -> Option<Rss> {
    imp::rss()
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{Rss, parse_status_kb};

    pub fn rss() -> Option<Rss> {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        Some(Rss {
            now: parse_status_kb(&status, "VmRSS")?,
            peak: parse_status_kb(&status, "VmHWM")?,
        })
    }
}

#[cfg(windows)]
mod imp {
    use super::Rss;
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::GetCurrentProcess;

    pub fn rss() -> Option<Rss> {
        let mut c = PROCESS_MEMORY_COUNTERS {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            ..Default::default()
        };
        // SAFETY: `GetCurrentProcess` returns a pseudo handle that needs no close,
        // and `c` is a properly sized, writable PROCESS_MEMORY_COUNTERS.
        unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) }.ok()?;
        Some(Rss {
            now: c.WorkingSetSize as u64,
            peak: c.PeakWorkingSetSize as u64,
        })
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod imp {
    use super::Rss;

    pub fn rss() -> Option<Rss> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &str = "Name:\tclipdump\nVmPeak:\t  900000 kB\nVmHWM:\t   52000 kB\nVmRSS:\t   48000 kB\nThreads:\t12\n";

    #[test]
    fn kb_fields_are_read_in_bytes() {
        assert_eq!(parse_status_kb(STATUS, "VmRSS"), Some(48_000 * 1024));
        assert_eq!(parse_status_kb(STATUS, "VmHWM"), Some(52_000 * 1024));
        assert_eq!(parse_status_kb(STATUS, "VmSwap"), None);
        // `Threads` has no unit: not a kB field.
        assert_eq!(parse_status_kb(STATUS, "Threads"), None);
        assert_eq!(parse_status_kb("VmRSS: lots kB", "VmRSS"), None);
    }

    #[cfg(any(target_os = "linux", windows))]
    #[test]
    fn this_process_has_memory_and_the_peak_is_not_below_it() {
        let r = rss().expect("readable on this platform");
        assert!(r.now > 1024 * 1024, "{r:?}");
        assert!(r.peak >= r.now / 2, "{r:?}");
        // Allocating and touching 64 MB shows up.
        let before = rss().unwrap().now;
        let big = vec![1u8; 64 << 20];
        std::hint::black_box(&big);
        assert!(rss().unwrap().now >= before + (32 << 20));
    }
}
