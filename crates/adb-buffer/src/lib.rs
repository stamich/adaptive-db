//! Page cache and page files.
//!
//! The buffer pool follows a **no-steal** policy: a dirty page is never written back on
//! eviction. Dirty pages leave memory only through a checkpoint (see
//! [`BufferPool::journal_writes`]), which publishes them atomically together with all other
//! files via `adb-journal`. Between checkpoints the on-disk files therefore always describe the
//! last checkpointed state exactly, and the canonical log replays everything after it.
pub mod buffer_pool;
pub mod error;
pub mod file_store;
pub mod page_store;

pub use buffer_pool::BufferPool;
pub use error::BufferError;
pub use file_store::{page_offset, FilePageStore};
pub use page_store::PageStore;
