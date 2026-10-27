use std::time::{Duration, Instant};

/// Best-effort cycle counter. Used only to time a short probe, not as a wall clock.
pub fn ticks() -> u64 {
    #[cfg(target_arch = "aarch64")]
    {
        let t: u64;
        unsafe {
            core::arch::asm!(
                "mrs {t}, cntvct_el0",
                t = out(reg) t,
                options(nomem, nostack, preserves_flags)
            );
        }
        t
    }
    #[cfg(target_arch = "x86_64")]
    {
        unsafe { core::arch::x86_64::_rdtsc() }
    }
    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        static ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        ORIGIN.get_or_init(Instant::now).elapsed().as_nanos() as u64
    }
}

pub fn measure(mut body: impl FnMut()) -> Duration {
    let start = Instant::now();
    body();
    start.elapsed()
}

pub struct Timer {
    origin: Instant,
}

impl Timer {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.origin.elapsed()
    }
}

impl Default for Timer {
    fn default() -> Self {
        Self::new()
    }
}
