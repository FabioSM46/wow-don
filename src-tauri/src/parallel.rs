//! Parallel network work with standard library threads (issue #180): the import resolves and
//! downloads several entries at once, then installs them one at a time.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

/// How many entries of an import are resolved and downloaded at once. Enough to cover the
/// latency of CurseForge and its CDN; few enough not to look like abuse to sources that have no
/// declared limit (Tukui, GitHub without a token) and to keep few half-done zips in memory.
pub const DOWNLOADS_AT_ONCE: usize = 6;

/// `f` on every item, with at most [`DOWNLOADS_AT_ONCE`] calls at once; the results come back
/// in the order of `items`. With zero or one item it creates no threads. A panic in `f`
/// propagates to the caller, as in a normal call.
pub fn map<T, R, F>(items: &[T], f: F) -> Vec<R>
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync,
{
    if items.len() <= 1 {
        return items.iter().map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<R>>> = Mutex::new(items.iter().map(|_| None).collect());
    thread::scope(|scope| {
        for _ in 0..DOWNLOADS_AT_ONCE.min(items.len()) {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(index) else {
                        break;
                    };
                    let result = f(item);
                    results.lock().unwrap_or_else(|e| e.into_inner())[index] = Some(result);
                }
            });
        }
    });
    results
        .into_inner()
        .unwrap_or_else(|e| e.into_inner())
        .into_iter()
        .map(|result| result.expect("every item has a result once the scope has ended"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn results_keep_the_order_of_the_items() {
        let items: Vec<u64> = (0..20).collect();
        // The first ones finish last.
        let doubled = map(&items, |n| {
            thread::sleep(Duration::from_millis(20 - n));
            n * 2
        });
        assert_eq!(doubled, items.iter().map(|n| n * 2).collect::<Vec<_>>());
    }

    #[test]
    fn items_run_together_but_never_more_than_the_limit() {
        let running = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let items = vec![(); DOWNLOADS_AT_ONCE * 2];
        let start = Instant::now();
        map(&items, |_| {
            let now = running.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(150));
            running.fetch_sub(1, Ordering::SeqCst);
        });
        // Two rounds of 150 ms, not twelve.
        let elapsed = start.elapsed();
        assert!(elapsed < Duration::from_millis(150 * 5), "{elapsed:?}");
        assert_eq!(peak.load(Ordering::SeqCst), DOWNLOADS_AT_ONCE);
    }

    #[test]
    fn no_items_no_threads() {
        let empty: [u8; 0] = [];
        assert!(map(&empty, |_| 1).is_empty());
        assert_eq!(map(&[7], |n| n + 1), [8]);
    }
}
