//! In-process measurements (report §13.1): private bytes, working set, GDI/USER
//! objects, handles, CPU time. Written as one JSON object per run.

use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
use windows_sys::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows_sys::Win32::System::Threading::*;

#[derive(Clone, Copy, Debug, Default)]
pub struct Sample {
    pub private_mb: f64,
    pub ws_mb: f64,
    pub gdi: u32,
    pub user: u32,
    pub handles: u32,
    #[allow(dead_code)]
    pub cpu_ms: f64,
}

fn ft(f: FILETIME) -> u64 {
    (f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64
}

pub fn cpu_ms() -> f64 {
    unsafe {
        let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut c, mut e, mut k, mut u) = (z, z, z, z);
        GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
        (ft(k) + ft(u)) as f64 / 10_000.0
    }
}

/// Milliseconds since the process was created (startup-time metric).
pub fn age_ms() -> f64 {
    unsafe {
        let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut c, mut e, mut k, mut u) = (z, z, z, z);
        GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
        let mut now = z;
        GetSystemTimePreciseAsFileTime(&mut now);
        (ft(now).saturating_sub(ft(c))) as f64 / 10_000.0
    }
}

pub fn sample() -> Sample {
    unsafe {
        let p = GetCurrentProcess();
        let mut m: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        m.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        GetProcessMemoryInfo(p, &mut m as *mut _ as *mut PROCESS_MEMORY_COUNTERS, m.cb);
        let mut handles = 0u32;
        GetProcessHandleCount(p, &mut handles);
        Sample {
            private_mb: m.PrivateUsage as f64 / 1048576.0,
            ws_mb: m.WorkingSetSize as f64 / 1048576.0,
            gdi: GetGuiResources(p, GR_GDIOBJECTS),
            user: GetGuiResources(p, GR_USEROBJECTS),
            handles,
            cpu_ms: cpu_ms(),
        }
    }
}

/// Minimal JSON object builder (no serde dependency).
#[derive(Default)]
pub struct Json(Vec<String>);

impl Json {
    pub fn s(mut self, k: &str, v: &str) -> Self {
        let mut e = String::with_capacity(v.len() + 2);
        for ch in v.chars() {
            match ch {
                '"' => e.push_str("\\\""),
                '\\' => e.push_str("\\\\"),
                c if (c as u32) < 0x20 || !c.is_ascii() => e.push_str(&format!("\\u{:04x}", c as u32 & 0xFFFF)),
                c => e.push(c),
            }
        }
        self.0.push(format!("\"{k}\":\"{e}\""));
        self
    }
    pub fn n(mut self, k: &str, v: f64) -> Self {
        let v = if v.is_finite() { format!("{:.4}", v) } else { "null".into() };
        self.0.push(format!("\"{k}\":{v}"));
        self
    }
    pub fn b(mut self, k: &str, v: bool) -> Self {
        self.0.push(format!("\"{k}\":{v}"));
        self
    }
    pub fn raw(mut self, k: &str, v: String) -> Self {
        self.0.push(format!("\"{k}\":{v}"));
        self
    }
    pub fn sample(self, prefix: &str, s: Sample) -> Self {
        self.n(&format!("{prefix}_private_mb"), s.private_mb)
            .n(&format!("{prefix}_ws_mb"), s.ws_mb)
            .n(&format!("{prefix}_gdi"), s.gdi as f64)
            .n(&format!("{prefix}_user"), s.user as f64)
            .n(&format!("{prefix}_handles"), s.handles as f64)
    }
    pub fn finish(self) -> String {
        format!("{{{}}}", self.0.join(","))
    }
}

pub fn append(path: &str, line: &str) {
    use std::io::Write;
    if path.is_empty() {
        println!("{line}");
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{line}");
    }
}

#[link(name = "kernel32")]
extern "system" {
    fn QueryThreadCycleTime(thread: windows_sys::Win32::Foundation::HANDLE, cycles: *mut u64) -> i32;
    fn QueryProcessCycleTime(process: windows_sys::Win32::Foundation::HANDLE, cycles: *mut u64) -> i32;
}

/// CPU cycles of every thread of this process, ended ones included.
pub fn process_cycles() -> u64 {
    let mut c = 0u64;
    unsafe { QueryProcessCycleTime(windows_sys::Win32::System::Threading::GetCurrentProcess(), &mut c) };
    c
}

/// Cycles per ms of CPU time on this PC (the cycle counter runs at a fixed rate): a 150 ms
/// spin on this thread, the best of three (a spin preempted by another program counts less).
pub fn cycles_per_ms() -> f64 {
    (0..3)
        .map(|_| {
            let (c0, t0) = (thread_cycles(), std::time::Instant::now());
            while t0.elapsed().as_millis() < 150 {
                std::hint::spin_loop();
            }
            (thread_cycles() - c0) as f64 / (t0.elapsed().as_secs_f64() * 1000.0)
        })
        .fold(0.0, f64::max)
}

/// CPU cycles of the calling thread (precise; `GetThreadTimes` counts in 15.6 ms ticks).
pub fn thread_cycles() -> u64 {
    let mut c = 0u64;
    unsafe { QueryThreadCycleTime(windows_sys::Win32::System::Threading::GetCurrentThread(), &mut c) };
    c
}

/// Diagnostic cycle counters and step traces (P6 profiles): off in the app — a measurement
/// mode turns them on (`set_diag`), so the app pays one relaxed load per hook and keeps no
/// growing trace.
pub static PROF: [std::sync::atomic::AtomicU64; 12] = [const { std::sync::atomic::AtomicU64::new(0) }; 12];
static DIAG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_diag(on: bool) {
    DIAG.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn diag() -> bool {
    DIAG.load(std::sync::atomic::Ordering::Relaxed)
}

/// Start of a profiled span (0 when diagnostics are off).
pub fn prof_start() -> u64 {
    if diag() {
        thread_cycles()
    } else {
        0
    }
}

pub fn prof_add(slot: usize, since: u64) {
    if diag() {
        PROF[slot].fetch_add(thread_cycles().wrapping_sub(since), std::sync::atomic::Ordering::Relaxed);
    }
}
