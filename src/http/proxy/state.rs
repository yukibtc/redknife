use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub(crate) struct ProxyState {
    // The next ID to assign to a request.
    next_id: AtomicU64,
}

impl ProxyState {
    #[inline]
    pub(super) fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }
}
