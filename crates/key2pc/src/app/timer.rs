use windows_sys::Win32::Media::{timeBeginPeriod, timeEndPeriod};

pub struct TimerResolutionGuard(u32);

impl TimerResolutionGuard {
    pub fn new(period_ms: u32) -> Self {
        unsafe {
            timeBeginPeriod(period_ms);
        }
        Self(period_ms)
    }
}

impl Drop for TimerResolutionGuard {
    fn drop(&mut self) {
        unsafe {
            timeEndPeriod(self.0);
        }
    }
}
