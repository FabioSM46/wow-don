//! Lavoro di rete in parallelo con i thread della libreria standard (issue #180): l'import
//! risolve e scarica più voci insieme, poi le installa una alla volta.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

/// Quante voci di un import si risolvono e si scaricano insieme. Abbastanza per coprire la
/// latenza di CurseForge e della sua CDN; poche per non sembrare un abuso a fonti che non hanno
/// un limite dichiarato (Tukui, GitHub senza token) e per tenere pochi zip in memoria a metà.
pub const DOWNLOADS_AT_ONCE: usize = 6;

/// `f` su ogni elemento, con al massimo [`DOWNLOADS_AT_ONCE`] chiamate insieme; i risultati
/// tornano nell'ordine di `items`. Con zero o un elemento non crea thread. Un panico in `f` si
/// propaga al chiamante, come in una chiamata normale.
pub fn map<T, R, F>(items: &[T], f: F) -> Vec<R>
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync,
{
    map_with(items, DOWNLOADS_AT_ONCE, f)
}

fn map_with<T, R, F>(items: &[T], workers: usize, f: F) -> Vec<R>
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
        for _ in 0..workers.clamp(1, items.len()) {
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
        // I primi finiscono per ultimi.
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
        // Due giri da 150 ms, non dodici.
        let elapsed = start.elapsed();
        assert!(elapsed < Duration::from_millis(150 * 5), "{elapsed:?}");
        assert_eq!(peak.load(Ordering::SeqCst), DOWNLOADS_AT_ONCE);
    }

    #[test]
    fn one_worker_is_sequential() {
        let order = Mutex::new(Vec::new());
        let items: Vec<u32> = (0..5).collect();
        map_with(&items, 1, |n| order.lock().unwrap().push(*n));
        assert_eq!(*order.lock().unwrap(), items);
    }

    #[test]
    fn no_items_no_threads() {
        let empty: [u8; 0] = [];
        assert!(map(&empty, |_| 1).is_empty());
        assert_eq!(map(&[7], |n| n + 1), [8]);
    }
}
