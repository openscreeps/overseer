//! Milliseconds of a monotonic clock: `performance.now()` in browsers, [`std::time::Instant`]
//! elsewhere.

/// Current time in milliseconds.
#[cfg(target_arch = "wasm32")]
pub fn now() -> f64 {
    web_sys::window()
        .and_then(|window| window.performance())
        .map_or(0.0, |performance| performance.now())
}

/// Current time in milliseconds since the first call.
#[cfg(not(target_arch = "wasm32"))]
pub fn now() -> f64 {
    use std::sync::LazyLock;
    use std::time::Instant;

    static START: LazyLock<Instant> = LazyLock::new(Instant::now);
    START.elapsed().as_secs_f64() * 1000.0
}
