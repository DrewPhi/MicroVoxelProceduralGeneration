//! Shared helpers for the terrain developer tools.

pub mod color;
pub mod iso;
pub mod source;

/// Evaluate `f(0..n)` across all CPU cores.
pub fn par_map<T: Send + Clone + Default>(n: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let mut out = vec![T::default(); n];
    let threads = std::thread::available_parallelism().map_or(4, |t| t.get());
    let chunk = n.div_ceil(threads).max(1);
    let f = &f;
    std::thread::scope(|s| {
        for (c, slice) in out.chunks_mut(chunk).enumerate() {
            s.spawn(move || {
                for (i, v) in slice.iter_mut().enumerate() {
                    *v = f(c * chunk + i);
                }
            });
        }
    });
    out
}
