//! The decode pool of the thumbnails: the bounds of the parallel jobs and the threads of the
//! cache reads.

use std::sync::OnceLock;

/// With 4 jobs, each with its share of the cores, the first thumbnail comes after one decode.
pub(crate) const MAX_THUMB_JOBS: usize = 4;

/// The sharp decodes of the preview formats wait until every preview is out.
pub(crate) const MAX_PREVIEW_JOBS: usize = 4;

/// A warm grid reads all its cells together, not 4 at a time behind the decode slots.
pub(crate) const MAX_CACHE_READS: usize = 64;

/// A QOI read takes one core. More threads than cores give nothing.
const MAX_CACHE_READ_THREADS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThumbPhase {
    CacheRead,
    Preview,
    Sharp,
}

/// A cache hit never waits for a cold HEIC or sips decode to free a thread.
pub(crate) fn spawn_job(phase: ThumbPhase, job: impl FnOnce() + Send + 'static) {
    match cache_read_pool() {
        Some(pool) if phase == ThumbPhase::CacheRead => pool.spawn(job),
        _ => rayon::spawn(job),
    }
}

/// `None` when the build failed: the reads then share the global pool.
fn cache_read_pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(rayon::current_num_threads().min(MAX_CACHE_READ_THREADS))
            .thread_name(|index| format!("phewr-cache-read-{index}"))
            .build()
            .ok()
    })
    .as_ref()
}
