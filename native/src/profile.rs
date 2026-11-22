//! Opt-in phase timing (`--features profile`). Without the feature every call is a no-op
//! the compiler removes.

#[derive(Clone, Copy, Debug)]
pub enum Phase {
    Corners,
    BlockDensity,
    Aquifer,
    SurfaceScan,
    Veins,
}

pub const PHASES: [Phase; 5] = [Phase::Corners, Phase::BlockDensity, Phase::Aquifer, Phase::SurfaceScan, Phase::Veins];

#[cfg(feature = "profile")]
mod imp {
    use super::Phase;
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

    static NANOS: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];

    pub struct Guard(Phase, std::time::Instant);

    impl Drop for Guard {
        fn drop(&mut self) {
            NANOS[self.0 as usize].fetch_add(self.1.elapsed().as_nanos() as u64, Relaxed);
        }
    }

    #[inline]
    pub fn time(p: Phase) -> Guard {
        Guard(p, std::time::Instant::now())
    }

    /// Nanoseconds per phase since the last call (then reset).
    pub fn take() -> [u64; 5] {
        std::array::from_fn(|i| NANOS[i].swap(0, Relaxed))
    }
}

#[cfg(not(feature = "profile"))]
mod imp {
    use super::Phase;

    pub struct Guard;

    #[inline(always)]
    pub fn time(_: Phase) -> Guard {
        Guard
    }

    pub fn take() -> [u64; 5] {
        [0; 5]
    }
}

pub use imp::{take, time};
