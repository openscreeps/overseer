//! Clocks, timers and tasks on every target: tokio natively, the browser in WebAssembly.
//!
//! Natively these are [`tokio::time`], [`tokio::spawn`] and the [`std::time`] clocks: a
//! [`Client`](crate::Client) and everything built on it run inside a tokio runtime with the time
//! driver enabled. In browsers (`wasm32-unknown-unknown`) there is no tokio runtime and the
//! [`std::time`] clocks panic: timers run on the global `setTimeout`, [`Instant`] reads
//! `performance.now()`, [`SystemTime`] reads `Date.now()` (both from the
//! [`wasmtimer`](https://docs.rs/wasmtimer) crate), and [`spawn`] runs tasks on the browser's
//! event loop (`wasm_bindgen_futures::spawn_local`). Both work in windows and web workers.
//!
//! Natively the client's futures and handles are `Send`, so they can move into spawned tasks of
//! a multi-threaded runtime. Browser futures (`fetch`, `WebSocket`) are not `Send`, and neither
//! are the client's futures there: [`MaybeSend`] and [`BoxFuture`] name the bound that holds on
//! the current target.

use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};

#[cfg(not(target_arch = "wasm32"))]
pub use std::time::{SystemTime, UNIX_EPOCH};
#[cfg(not(target_arch = "wasm32"))]
pub use tokio::time::{Instant, Sleep, Timeout, error::Elapsed, sleep, sleep_until, timeout};
#[cfg(target_arch = "wasm32")]
pub use wasmtimer::std::{Instant, SystemTime, UNIX_EPOCH};
#[cfg(target_arch = "wasm32")]
pub use wasmtimer::tokio::{Sleep, Timeout, error::Elapsed, sleep, sleep_until, timeout};

/// `Send` natively; no bound in browsers, whose futures and handles are thread-bound.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}

#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + ?Sized> MaybeSend for T {}

/// `Send` natively; no bound in browsers, whose futures and handles are thread-bound.
#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}

#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSend for T {}

/// A boxed future: `Send` natively, not in browsers ([`MaybeSend`]).
#[cfg(not(target_arch = "wasm32"))]
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A boxed future: `Send` natively, not in browsers ([`MaybeSend`]).
#[cfg(target_arch = "wasm32")]
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// Runs `future` in the background: a tokio task natively (inside a tokio runtime), a task on
/// the browser's event loop in WebAssembly.
///
/// # Panics
///
/// Natively, when called outside a tokio runtime.
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + MaybeSend + 'static,
    F::Output: MaybeSend + 'static,
{
    #[cfg(not(target_arch = "wasm32"))]
    {
        JoinHandle(tokio::spawn(future))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        wasm_bindgen_futures::spawn_local(async move {
            // The handle may be gone: the task was detached.
            let _ = sender.send(future.await);
        });
        JoinHandle(receiver)
    }
}

/// A task started by [`spawn`]. Awaiting it waits for the task to end: its output, or `None`
/// when it panicked or the runtime shut down first. Dropping it detaches the task, which keeps
/// running.
pub struct JoinHandle<T>(Task<T>);

#[cfg(not(target_arch = "wasm32"))]
type Task<T> = tokio::task::JoinHandle<T>;

#[cfg(target_arch = "wasm32")]
type Task<T> = tokio::sync::oneshot::Receiver<T>;

impl<T> Future for JoinHandle<T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.get_mut().0).poll(cx).map(Result::ok)
    }
}

impl<T> fmt::Debug for JoinHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JoinHandle").finish_non_exhaustive()
    }
}
