//! Fixed-size database page representation and hardened common-header validation.

use adb_core::{Lsn, PageId};
use crc32fast::Hasher;

use crate::PageError;

/// Fixed database page size in bytes.
pub const PAGE_SIZE: usize = 16 * 1024;
/// Number of bytes reserved for the common database page header.
pub const PAGE_HEADER_SIZE: usize = 32;
/// Magic value identifying an Adaptive DB page.
pub const PAGE_MAGIC: u32 = 0x4144_4250;
/// Hardened page-header format version written by Milestone 1.5.1.
pub const PAGE_FORMAT_VERSION: u16 = 1;

/// Documents `OFF_MAGIC` and its role in this hardened milestone.
const OFF_MAGIC: usize = 0;
/// Documents `OFF_KIND` and its role in this hardened milestone.
const OFF_KIND: usize = 4;
/// Documents `OFF_SLOT_COUNT` and its role in this hardened milestone.
const OFF_SLOT_COUNT: usize = 6;
/// Documents `OFF_FREE_START` and its role in this hardened milestone.
const OFF_FREE_START: usize = 8;
/// Documents `OFF_FREE_END` and its role in this hardened milestone.
const OFF_FREE_END: usize = 10;
/// Documents `OFF_FORMAT_VERSION` and its role in this hardened milestone.
const OFF_FORMAT_VERSION: usize = 12;
/// Documents `OFF_PAGE_LSN` and its role in this hardened milestone.
const OFF_PAGE_LSN: usize = 16;
/// Documents `OFF_CHECKSUM` and its role in this hardened milestone.
const OFF_CHECKSUM: usize = 24;

/// Enumerates physical page kinds used by the milestone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum PageKind {
    /// Heap/slotted page storing serialized current records.
    Heap = 1,
    /// B+Tree leaf page.
    BTreeLeaf = 2,
    /// B+Tree internal page.
    BTreeInternal = 3,
}

/// Converts persisted numeric page-kind tags into typed values.
impl TryFrom<u16> for PageKind {
    /// Page decoding error.
    type Error = PageError;

    /// Validates and converts one persisted page kind.
    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Heap),
            2 => Ok(Self::BTreeLeaf),
            3 => Ok(Self::BTreeInternal),
            other => Err(PageError::Corrupt(format!("unknown page kind {other}"))),
        }
    }
}

/// Owns one fixed-size database page together with its identifier and binary header.
#[derive(Clone)]
pub struct Page {
    /// Stable physical page identifier.
    pub id: PageId,
    bytes: Box<[u8; PAGE_SIZE]>,
}

/// Implements page initialization, validation, metadata access and checksum sealing.
impl Page {
    /// Creates a new hardened page with format version 1.
    pub fn new(id: PageId, kind: PageKind) -> Self {
        let mut page = Self {
            id,
            bytes: Box::new([0; PAGE_SIZE]),
        };
        page.write_u32(OFF_MAGIC, PAGE_MAGIC);
        page.write_u16(OFF_KIND, kind as u16);
        page.write_u16(OFF_SLOT_COUNT, 0);
        page.write_u16(OFF_FREE_START, PAGE_HEADER_SIZE as u16);
        page.write_u16(OFF_FREE_END, PAGE_SIZE as u16);
        page.write_u16(OFF_FORMAT_VERSION, PAGE_FORMAT_VERSION);
        page.set_page_lsn(Lsn(0));
        page.write_u32(OFF_CHECKSUM, 0);
        page
    }

    /// Constructs a page from disk and validates magic, kind, header bounds and checksum.
    ///
    /// Legacy Milestone 1.5 pages have format version 0 and no checksum. They remain readable
    /// after structural validation and are upgraded to version 1 on their next physical write.
    pub fn from_bytes(id: PageId, bytes: [u8; PAGE_SIZE]) -> Result<Self, PageError> {
        let page = Self {
            id,
            bytes: Box::new(bytes),
        };
        if page.read_u32(OFF_MAGIC) != PAGE_MAGIC {
            return Err(PageError::Corrupt(format!("bad magic for page {}", id.0)));
        }
        let kind = PageKind::try_from(page.read_u16(OFF_KIND))?;
        let version = page.read_u16(OFF_FORMAT_VERSION);
        if version > PAGE_FORMAT_VERSION {
            return Err(PageError::Corrupt(format!(
                "unsupported page version {version}"
            )));
        }
        page.validate_common_header(kind)?;
        if version == PAGE_FORMAT_VERSION {
            let stored = page.read_u32(OFF_CHECKSUM);
            if stored == 0 || stored != page.compute_checksum() {
                return Err(PageError::Corrupt(format!(
                    "checksum mismatch for page {}",
                    id.0
                )));
            }
        }
        Ok(page)
    }

    /// Returns the validated physical page kind.
    pub fn kind(&self) -> Result<PageKind, PageError> {
        PageKind::try_from(self.read_u16(OFF_KIND))
    }

    /// Updates the physical page kind.
    pub fn set_kind(&mut self, kind: PageKind) {
        self.write_u16(OFF_KIND, kind as u16);
    }

    /// Returns the heap slot count.
    pub fn slot_count(&self) -> u16 {
        self.read_u16(OFF_SLOT_COUNT)
    }

    /// Updates the heap slot count.
    pub fn set_slot_count(&mut self, value: u16) {
        self.write_u16(OFF_SLOT_COUNT, value);
    }

    /// Returns the beginning of free space.
    pub fn free_start(&self) -> u16 {
        self.read_u16(OFF_FREE_START)
    }

    /// Updates the beginning of free space.
    pub fn set_free_start(&mut self, value: u16) {
        self.write_u16(OFF_FREE_START, value);
    }

    /// Returns the end of free space.
    pub fn free_end(&self) -> u16 {
        self.read_u16(OFF_FREE_END)
    }

    /// Updates the end of free space.
    pub fn set_free_end(&mut self, value: u16) {
        self.write_u16(OFF_FREE_END, value);
    }

    /// Returns the page LSN stamped by the storage/index writer.
    pub fn page_lsn(&self) -> Lsn {
        Lsn(self.read_u64(OFF_PAGE_LSN))
    }

    /// Updates the page LSN.
    pub fn set_page_lsn(&mut self, lsn: Lsn) {
        self.write_u64(OFF_PAGE_LSN, lsn.0);
    }

    /// Returns the data payload after the common header.
    pub fn payload(&self) -> &[u8] {
        &self.bytes[PAGE_HEADER_SIZE..]
    }

    /// Returns the mutable data payload after the common header.
    pub fn payload_mut(&mut self) -> &mut [u8] {
        &mut self.bytes[PAGE_HEADER_SIZE..]
    }

    /// Returns the complete encoded page bytes.
    pub fn bytes(&self) -> &[u8; PAGE_SIZE] {
        &self.bytes
    }

    /// Returns mutable complete page bytes.
    pub fn bytes_mut(&mut self) -> &mut [u8; PAGE_SIZE] {
        &mut self.bytes
    }

    /// Marks this page as hardened format v1 and stores a CRC32 over the complete page.
    pub fn seal_for_write(&mut self) {
        self.write_u16(OFF_FORMAT_VERSION, PAGE_FORMAT_VERSION);
        self.write_u32(OFF_CHECKSUM, 0);
        let checksum = self.compute_checksum();
        self.write_u32(OFF_CHECKSUM, checksum);
    }

    /// Validates common free-space and slot-directory invariants before higher-level decoding.
    fn validate_common_header(&self, kind: PageKind) -> Result<(), PageError> {
        let start = self.free_start() as usize;
        let end = self.free_end() as usize;
        if start < PAGE_HEADER_SIZE || start > end || end > PAGE_SIZE {
            return Err(PageError::Corrupt(format!(
                "invalid free-space bounds start={start} end={end}"
            )));
        }
        if kind == PageKind::Heap {
            let slots = self.slot_count() as usize;
            let directory_end = PAGE_HEADER_SIZE
                .checked_add(
                    slots
                        .checked_mul(4)
                        .ok_or_else(|| PageError::Corrupt("slot directory overflow".into()))?,
                )
                .ok_or_else(|| PageError::Corrupt("slot directory overflow".into()))?;
            if directory_end != start || directory_end > PAGE_SIZE {
                return Err(PageError::Corrupt("invalid heap slot directory".into()));
            }
            for slot in 0..slots {
                let off = PAGE_HEADER_SIZE + slot * 4;
                let tuple_off = self.read_u16(off) as usize;
                let len = self.read_u16(off + 2) as usize;
                let tuple_end = tuple_off
                    .checked_add(len)
                    .ok_or_else(|| PageError::Corrupt("slot range overflow".into()))?;
                if tuple_off < end || tuple_end > PAGE_SIZE {
                    return Err(PageError::Corrupt(format!(
                        "slot {slot} points outside tuple area"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Computes page CRC with the checksum field logically zeroed.
    fn compute_checksum(&self) -> u32 {
        let mut hasher = Hasher::new();
        hasher.update(&self.bytes[..OFF_CHECKSUM]);
        hasher.update(&[0u8; 4]);
        hasher.update(&self.bytes[OFF_CHECKSUM + 4..]);
        hasher.finalize()
    }

    /// Reads a little-endian u16 from a compile-time-safe header offset.
    fn read_u16(&self, off: usize) -> u16 {
        u16::from_le_bytes([self.bytes[off], self.bytes[off + 1]])
    }

    /// Reads a little-endian u32 from a compile-time-safe header offset.
    fn read_u32(&self, off: usize) -> u32 {
        u32::from_le_bytes([
            self.bytes[off],
            self.bytes[off + 1],
            self.bytes[off + 2],
            self.bytes[off + 3],
        ])
    }

    /// Reads a little-endian u64 from a compile-time-safe header offset.
    fn read_u64(&self, off: usize) -> u64 {
        u64::from_le_bytes([
            self.bytes[off],
            self.bytes[off + 1],
            self.bytes[off + 2],
            self.bytes[off + 3],
            self.bytes[off + 4],
            self.bytes[off + 5],
            self.bytes[off + 6],
            self.bytes[off + 7],
        ])
    }

    /// Writes a little-endian u16 to a compile-time-safe header offset.
    fn write_u16(&mut self, off: usize, value: u16) {
        self.bytes[off..off + 2].copy_from_slice(&value.to_le_bytes());
    }

    /// Writes a little-endian u32 to a compile-time-safe header offset.
    fn write_u32(&mut self, off: usize, value: u32) {
        self.bytes[off..off + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// Writes a little-endian u64 to a compile-time-safe header offset.
    fn write_u64(&mut self, off: usize, value: u64) {
        self.bytes[off..off + 8].copy_from_slice(&value.to_le_bytes());
    }
}
