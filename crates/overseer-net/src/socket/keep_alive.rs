//! Keep-alive of an open connection: natively a ping after `interval` without frames, and a
//! dead connection after two. Browsers answer the server's pings themselves but cannot send
//! any, so there the state is inert and a dead connection is noticed when the browser closes it.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use crate::rt::Instant;
use crate::rt::Sleep;

/// Keep-alive state of one connection.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub(super) struct KeepAlive {
    /// When the last frame arrived (or the connection opened).
    last_frame: Instant,
    /// A ping went out since `last_frame`.
    pinged: bool,
    /// A keep-alive ping waits for the connection to take it.
    pub(super) ping_due: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl KeepAlive {
    pub(super) fn new() -> Self {
        Self {
            last_frame: Instant::now(),
            pinged: false,
            ping_due: false,
        }
    }

    /// A frame arrived: the connection is alive.
    pub(super) fn received(&mut self) {
        self.last_frame = Instant::now();
        self.pinged = false;
    }

    /// Waits on `timer` until a ping is due (`Ready(true)`, [`Self::ping_due`] is set) or the
    /// connection is dead (`Ready(false)`); pending forever without an `interval`.
    pub(super) fn poll(
        &mut self,
        interval: Option<Duration>,
        timer: &mut Pin<Box<Sleep>>,
        cx: &mut Context<'_>,
    ) -> Poll<bool> {
        let Some(interval) = interval else {
            return Poll::Pending;
        };
        let deadline = self.last_frame + if self.pinged { interval * 2 } else { interval };
        if timer.deadline() != deadline {
            timer.as_mut().reset(deadline);
        }
        if timer.as_mut().poll(cx).is_pending() {
            return Poll::Pending;
        }
        if self.pinged {
            return Poll::Ready(false);
        }
        self.pinged = true;
        self.ping_due = true;
        Poll::Ready(true)
    }
}

/// Keep-alive state of one connection: inert in browsers.
#[cfg(target_arch = "wasm32")]
#[derive(Debug)]
pub(super) struct KeepAlive;

#[cfg(target_arch = "wasm32")]
impl KeepAlive {
    pub(super) const fn new() -> Self {
        Self
    }

    /// A frame arrived; nothing to track.
    #[expect(
        clippy::unused_self,
        reason = "the same interface as the native keep-alive"
    )]
    pub(super) const fn received(&mut self) {}

    /// Never due: the browser keeps the connection alive.
    #[expect(
        clippy::unused_self,
        reason = "the same interface as the native keep-alive"
    )]
    pub(super) const fn poll(
        &mut self,
        _interval: Option<Duration>,
        _timer: &mut Pin<Box<Sleep>>,
        _cx: &mut Context<'_>,
    ) -> Poll<bool> {
        Poll::Pending
    }
}
