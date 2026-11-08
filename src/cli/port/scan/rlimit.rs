#[cfg(unix)]
use std::thread;

#[cfg(unix)]
use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};

/// Extra file descriptors on top of the scan concurrency, covering the
/// async runtime (epoll and eventfd per worker), stdio, and any other
/// handle the process may open.
#[cfg(unix)]
fn reserve() -> u64 {
    let workers: u64 =
        u64::try_from(thread::available_parallelism().map_or(1, usize::from)).unwrap_or(1);

    (workers * 2) + 16
}

/// Compute the concurrency sustainable by a `RLIMIT_NOFILE` budget and the
/// soft limit it requires, given the current soft and hard limits.
///
/// Returns `(soft_limit, concurrency)`, where `soft_limit` is the value the
/// caller should attempt to set (`>=` the current soft limit) and
/// `concurrency` never exceeds the requested one.
#[cfg(unix)]
fn plan(requested: u64, soft: u64, hard: u64, reserve: u64) -> (u64, u64) {
    let needed: u64 = requested.saturating_add(reserve);

    // The current soft limit already covers the request.
    if soft >= needed {
        return (soft, requested);
    }

    // The soft limit can be raised up to the hard limit without any
    // privilege: if even the hard limit falls short, the best that can be
    // done is to use it in full.
    let target: u64 = needed.min(hard).max(soft);
    let concurrency: u64 = target.saturating_sub(reserve).max(1).min(requested);

    (target, concurrency)
}

/// Ensure the process can sustain the requested scan concurrency.
///
/// Attempts to raise the `RLIMIT_NOFILE` soft limit up to the hard limit
/// when the current one falls short (allowed for unprivileged processes);
/// if that is not possible, the concurrency is clamped to the available
/// budget, trading scan speed for correctness.
#[cfg(unix)]
pub(super) fn ensure_budget(concurrency: usize) -> usize {
    let requested: u64 = u64::try_from(concurrency).unwrap_or(u64::MAX);

    let limits: Rlimit = getrlimit(Resource::Nofile);
    let soft: u64 = limits.current.unwrap_or(u64::MAX);
    let hard: u64 = limits.maximum.unwrap_or(u64::MAX);

    let reserve: u64 = reserve();
    let (target, mut budget) = plan(requested, soft, hard, reserve);

    // The soft limit may be raised up to the hard limit without any
    // privilege: do it silently, for it is what makes the scan run at full
    // speed in the common case.
    if target > soft {
        match setrlimit(
            Resource::Nofile,
            Rlimit {
                current: Some(target),
                maximum: limits.maximum,
            },
        ) {
            Ok(()) => {
                tracing::debug!("Raised RLIMIT_NOFILE soft limit from {soft} to {target}");
            }
            Err(error) => {
                // Without the raise, fall back to the untouched soft limit.
                budget = soft.saturating_sub(reserve).max(1).min(requested);
                tracing::debug!("Could not raise RLIMIT_NOFILE soft limit ({error})");
            }
        }
    }

    // Warn whenever the scan cannot run at the requested speed, whatever
    // the reason (hard limit too low, or raise refused by the system).
    if budget < requested {
        tracing::warn!(
            "RLIMIT_NOFILE cannot sustain {requested} concurrent probes: scanning with {budget}; raise `ulimit -n` for a full-speed scan"
        );
    }

    budget as usize
}

/// Ensure the process can sustain the requested scan concurrency.
///
/// Platforms without rlimit semantics use the requested concurrency as is.
#[cfg(not(unix))]
pub(super) fn ensure_budget(concurrency: usize) -> usize {
    concurrency
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn keeps_concurrency_within_budget() {
        assert_eq!(plan(4_500, 100_000, 1_000_000, 32), (100_000, 4_500));
    }

    #[test]
    fn raises_soft_limit_up_to_the_request() {
        assert_eq!(plan(4_500, 1_024, 1_000_000, 32), (4_532, 4_500));
    }

    #[test]
    fn clamps_to_hard_limit_when_it_falls_short() {
        assert_eq!(plan(4_500, 1_024, 1_024, 32), (1_024, 992));
    }

    #[test]
    fn clamps_to_soft_limit_when_it_cannot_be_raised() {
        assert_eq!(plan(4_500, 512, 512, 32), (512, 480));
    }

    #[test]
    fn keeps_at_least_one_probe() {
        assert_eq!(plan(4_500, 16, 16, 32), (16, 1));
    }
}
