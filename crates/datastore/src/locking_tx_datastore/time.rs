#[cfg(target_arch = "wasm32")]
use std::sync::OnceLock;
#[cfg(target_arch = "wasm32")]
use std::time::Duration;

use spacetimedb_lib::Timestamp;

#[cfg(not(target_arch = "wasm32"))]
pub use std::time::Instant;

/// Wall-clock and monotonic time supplied by the embedder on `wasm32`, where `std` has neither.
///
/// Without an installed clock the datastore falls back to the module-unit-test stubs:
/// epoch-zero timestamps and zero-duration timings.
#[cfg(target_arch = "wasm32")]
pub trait HostClock: Send + Sync {
    /// The current wall-clock time.
    fn now(&self) -> Timestamp;
    /// Time elapsed since an arbitrary fixed origin; never decreases.
    fn monotonic(&self) -> Duration;
}

#[cfg(target_arch = "wasm32")]
static HOST_CLOCK: OnceLock<Box<dyn HostClock>> = OnceLock::new();

/// Install the process-wide host clock. Returns `false` if one was already installed.
#[cfg(target_arch = "wasm32")]
pub fn install_host_clock(clock: Box<dyn HostClock>) -> bool {
    HOST_CLOCK.set(clock).is_ok()
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
#[allow(deprecated)]
pub fn now_timestamp() -> Timestamp {
    Timestamp::now()
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub fn now_timestamp() -> Timestamp {
    HOST_CLOCK.get().map_or(Timestamp::UNIX_EPOCH, |clock| clock.now())
}

// `std::time::Instant::now` panics on `wasm32-unknown-unknown`, so on wasm32 an instant is
// a reading of the host clock's monotonic time (zero when no host clock is installed).
#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy, Debug)]
pub struct Instant(Duration);

#[cfg(target_arch = "wasm32")]
impl Instant {
    pub fn now() -> Self {
        Self(HOST_CLOCK.get().map_or(Duration::ZERO, |clock| clock.monotonic()))
    }

    pub fn elapsed(&self) -> Duration {
        Self::now().0.saturating_sub(self.0)
    }
}
