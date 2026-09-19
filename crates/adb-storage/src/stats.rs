//! Module `stats` for crate `adb-storage`.
/// Represents `StorageStats` state used by this subsystem.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StorageStats {
    pub current_rows: u64,
    pub historical_versions: u64,
    pub current_heap_pages: u64,
    pub current_index_pages: u64,
    pub version_heap_pages: u64,
    pub version_index_pages: u64,
}
