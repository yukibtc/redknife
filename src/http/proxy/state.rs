use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::mpsc::{Permit, Sender};

use super::{PendingRequest, RequestRecord};

pub(super) enum DispatchOutcome {
    Continue(Box<PendingRequest>),
    WaitForInterceptorDecision,
    ChannelClosed,
}

struct State {
    // Whether the interceptor is enabled.
    intercepting: bool,
    // The next ID to assign to a request.
    next_id: u64,
    // IDs are assigned on receipt, before reading the body. Resolution order can differ.
    history: BTreeMap<u64, Arc<RequestRecord>>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            intercepting: false,
            next_id: 1,
            history: BTreeMap::new(),
        }
    }
}

#[derive(Default)]
pub(crate) struct ProxyState {
    inner: Mutex<State>,
}

impl ProxyState {
    #[inline]
    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }

    #[inline]
    pub(crate) fn is_intercepting(&self) -> bool {
        self.lock().intercepting
    }

    /// Toggle the intercepting status and returns `true` if now it's enabled, or `false` if it's disabled.
    #[inline]
    pub(crate) fn toggle_intercepting(&self) -> bool {
        let mut state: MutexGuard<'_, State> = self.lock();
        state.intercepting = !state.intercepting;
        state.intercepting
    }

    #[inline]
    pub(super) fn next_id(&self) -> u64 {
        let mut state: MutexGuard<'_, State> = self.lock();
        let id: u64 = state.next_id;
        state.next_id += 1;
        id
    }

    pub(super) fn dispatch(
        &self,
        pending: PendingRequest,
        sender: &Sender<PendingRequest>,
        permit: Option<Permit<'_, PendingRequest>>,
    ) -> DispatchOutcome {
        let state: MutexGuard<'_, State> = self.lock();

        if sender.is_closed() {
            return DispatchOutcome::ChannelClosed;
        }

        // The interceptor is disabled, dispatch immediately.
        if !state.intercepting {
            return DispatchOutcome::Continue(Box::new(pending));
        }

        let permit: Option<Permit<'_, PendingRequest>> =
            permit.or_else(|| sender.try_reserve().ok());

        if let Some(permit) = permit {
            permit.send(pending);
        }

        // A full or closed queue cancels the reply, preserving fail-closed behavior.
        DispatchOutcome::WaitForInterceptorDecision
    }

    #[inline]
    pub(crate) fn record(&self, request: RequestRecord) {
        let mut state = self.lock();
        state.history.insert(request.id, Arc::new(request));
    }

    /// Clone only entry handles, and only when new resolutions changed the archive.
    pub(crate) fn history_since(&self, count: usize) -> Option<Vec<Arc<RequestRecord>>> {
        let state: MutexGuard<'_, State> = self.lock();

        if state.history.len() == count {
            return None;
        }

        Some(state.history.values().cloned().collect())
    }
}
