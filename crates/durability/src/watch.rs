//! A portable single-value watch channel, the subset of `tokio::sync::watch` that
//! [`crate::DurableOffset`] and the durability implementations use.
//!
//! Built when the `local` feature (and so Tokio) is off, e.g. on `wasm32-unknown-unknown`.
//! Same semantics as Tokio's: receivers see the latest value, `has_changed` and `wait_for`
//! report an error once the sender is gone, and `wait_for` resolves immediately if the current
//! value already satisfies the predicate.

use std::{
    fmt,
    future::poll_fn,
    ops::Deref,
    sync::{Arc, Mutex, MutexGuard},
    task::{Poll, Waker},
};

pub mod error {
    /// The sender of a watch channel was dropped.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
    #[error("channel closed")]
    pub struct RecvError(pub(super) ());
}
use error::RecvError;

struct State<T> {
    value: T,
    version: u64,
    closed: bool,
    wakers: Vec<Waker>,
}

struct Shared<T> {
    state: Mutex<State<T>>,
}

impl<T> Shared<T> {
    fn lock(&self) -> MutexGuard<'_, State<T>> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Create a watch channel holding `init`.
pub fn channel<T>(init: T) -> (Sender<T>, Receiver<T>) {
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            value: init,
            version: 0,
            closed: false,
            wakers: Vec::new(),
        }),
    });
    (Sender { shared: shared.clone() }, Receiver { shared, seen: 0 })
}

/// The sending half: replaces the value and wakes the receivers waiting on it.
pub struct Sender<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Sender<T> {
    /// A new receiver that has seen the current value.
    pub fn subscribe(&self) -> Receiver<T> {
        let seen = self.shared.lock().version;
        Receiver {
            shared: self.shared.clone(),
            seen,
        }
    }

    /// The current value.
    pub fn borrow(&self) -> Ref<'_, T> {
        Ref(self.shared.lock())
    }

    /// Modify the value in place and notify the receivers.
    pub fn send_modify(&self, modify: impl FnOnce(&mut T)) {
        let wakers = {
            let mut state = self.shared.lock();
            modify(&mut state.value);
            state.version += 1;
            std::mem::take(&mut state.wakers)
        };
        wakers.into_iter().for_each(Waker::wake);
    }

    /// Replace the value, notify the receivers and return the old value.
    pub fn send_replace(&self, value: T) -> T {
        let mut old = Some(value);
        self.send_modify(|v| {
            let new = old.take().unwrap();
            old = Some(std::mem::replace(v, new));
        });
        old.unwrap()
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let wakers = {
            let mut state = self.shared.lock();
            state.closed = true;
            std::mem::take(&mut state.wakers)
        };
        wakers.into_iter().for_each(Waker::wake);
    }
}

/// The receiving half.
pub struct Receiver<T> {
    shared: Arc<Shared<T>>,
    seen: u64,
}

impl<T> Clone for Receiver<T> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
            seen: self.seen,
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for Receiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.shared.lock();
        f.debug_struct("Receiver")
            .field("value", &state.value)
            .field("version", &state.version)
            .field("closed", &state.closed)
            .finish()
    }
}

impl<T> Receiver<T> {
    /// The current value.
    pub fn borrow(&self) -> Ref<'_, T> {
        Ref(self.shared.lock())
    }

    /// Whether the value changed since this receiver last saw it; an error once the sender is
    /// gone.
    pub fn has_changed(&self) -> Result<bool, RecvError> {
        let state = self.shared.lock();
        if state.closed {
            return Err(RecvError(()));
        }
        Ok(state.version != self.seen)
    }

    /// Wait until the value satisfies `f`, checking the current value first. An error if the
    /// sender is dropped before it does.
    pub async fn wait_for(&mut self, mut f: impl FnMut(&T) -> bool) -> Result<Ref<'_, T>, RecvError> {
        let shared = self.shared.clone();
        let seen = poll_fn(|cx| {
            let mut state = shared.lock();
            if f(&state.value) {
                return Poll::Ready(Ok(state.version));
            }
            if state.closed {
                return Poll::Ready(Err(RecvError(())));
            }
            state.wakers.push(cx.waker().clone());
            Poll::Pending
        })
        .await?;
        self.seen = seen;
        Ok(self.borrow())
    }
}

/// A borrow of the current value. Holds the channel's lock: drop it promptly.
pub struct Ref<'a, T>(MutexGuard<'a, State<T>>);

impl<T> Deref for Ref<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0.value
    }
}

#[cfg(test)]
mod tests {
    use std::{future::Future, pin::pin, task::Context};

    use futures::task::noop_waker;

    use super::*;

    fn poll_once<F: Future>(fut: std::pin::Pin<&mut F>) -> Poll<F::Output> {
        let waker = noop_waker();
        fut.poll(&mut Context::from_waker(&waker))
    }

    #[test]
    fn receiver_sees_latest_value_and_change() {
        let (tx, rx) = channel(1);
        assert_eq!(*rx.borrow(), 1);
        assert_eq!(rx.has_changed(), Ok(false));

        assert_eq!(tx.send_replace(2), 1);
        assert_eq!(*rx.borrow(), 2);
        assert_eq!(rx.has_changed(), Ok(true));

        // A receiver subscribed after the send has already seen it.
        assert_eq!(tx.subscribe().has_changed(), Ok(false));
    }

    #[test]
    fn wait_for_resolves_at_once_if_already_satisfied() {
        let (_tx, mut rx) = channel(5);
        let fut = pin!(rx.wait_for(|v| *v >= 5));
        match poll_once(fut) {
            Poll::Ready(Ok(v)) => assert_eq!(*v, 5),
            _ => panic!("wait_for should be ready"),
        }
    }

    #[test]
    fn wait_for_waits_for_a_satisfying_send() {
        let (tx, mut rx) = channel(0);
        {
            let mut fut = pin!(rx.wait_for(|v| *v >= 2));
            assert!(poll_once(fut.as_mut()).is_pending());
            tx.send_replace(1);
            assert!(poll_once(fut.as_mut()).is_pending());
            tx.send_replace(2);
            match poll_once(fut.as_mut()) {
                Poll::Ready(Ok(v)) => assert_eq!(*v, 2),
                _ => panic!("wait_for should be ready"),
            }
        }
        // `wait_for` marks the value it returned as seen.
        assert_eq!(rx.has_changed(), Ok(false));
    }

    #[test]
    fn dropped_sender_closes_the_channel() {
        let (tx, mut rx) = channel(0);
        {
            let mut fut = pin!(rx.wait_for(|v| *v >= 1));
            assert!(poll_once(fut.as_mut()).is_pending());
            drop(tx);
            assert!(matches!(poll_once(fut.as_mut()), Poll::Ready(Err(_))));
        }
        assert!(rx.has_changed().is_err());
        // The last value stays readable.
        assert_eq!(*rx.borrow(), 0);
    }
}
