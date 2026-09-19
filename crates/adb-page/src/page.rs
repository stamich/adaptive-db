//! Fixed-size database page representation with structural validation and checksums.
use crate::PageError;
use adb_core::{Lsn, PageId};
use crc32fast::Hasher;
/// Fixed database page size in bytes.
pub const PAGE_SIZE: usize = 16 * 1024;
/// Bytes reserved for the common page header.
pub const PAGE_HEADER_SIZE: usize = 32;
/// Magic value identifying an Adaptive DB page.
pub const PAGE_MAGIC: u32 = 0x4144_4250;
/// Hardened page format written by Milestone 1.6.1/1.7.1.
pub const PAGE_FORMAT_VERSION: u16 = 1;
/// Defines the `OFF_MAGIC` constant used by this subsystem.
const OFF_MAGIC: usize = 0;
const OFF_KIND: usize = 4;
const OFF_SLOT_COUNT: usize = 6;
const OFF_FREE_START: usize = 8;
const OFF_FREE_END: usize = 10;
const OFF_FORMAT_VERSION: usize = 12;
const OFF_PAGE_LSN: usize = 16;
const OFF_CHECKSUM: usize = 24;
/// Enumerates physical page types persisted by the storage engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum PageKind {
    /// Heap/slotted page.
    Heap = 1,
    /// B+Tree leaf page.
    BTreeLeaf = 2,
    /// B+Tree internal page.
    BTreeInternal = 3,
}
/// Converts persisted page-kind tags into typed values.
impl TryFrom<u16> for PageKind {
    /// Error returned when a persisted kind tag is unknown.
    type Error = PageError;

    /// Converts a validated numeric kind tag into `PageKind`.
    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Heap),
            2 => Ok(Self::BTreeLeaf),
            3 => Ok(Self::BTreeInternal),
            other => Err(PageError::Corrupt(format!("unknown page kind {other}"))),
        }
    }
}
/// Owns one fixed-size page and its stable physical identifier.
#[derive(Clone)]
pub struct Page {
    /// Stable physical page identifier.
    pub id: PageId,
    bytes: Box<[u8; PAGE_SIZE]>,
}
/// Implements page initialization, decoding, structural validation, and checksum sealing.
impl Page {
    /// Creates a fresh page in hardened format; checksum is sealed by the PageStore before persistence.
    pub fn new(id: PageId, kind: PageKind) -> Self {
        let mut p = Self {
            id,
            bytes: Box::new([0; PAGE_SIZE]),
        };
        p.write_u32(OFF_MAGIC, PAGE_MAGIC);
        p.write_u16(OFF_KIND, kind as u16);
        p.write_u16(OFF_SLOT_COUNT, 0);
        p.write_u16(OFF_FREE_START, PAGE_HEADER_SIZE as u16);
        p.write_u16(OFF_FREE_END, PAGE_SIZE as u16);
        p.write_u16(OFF_FORMAT_VERSION, PAGE_FORMAT_VERSION);
        p.set_page_lsn(Lsn(0));
        p.write_u32(OFF_CHECKSUM, 0);
        p
    }
    /// Decodes a persisted page and rejects bad magic, kind, bounds, slots, version, or checksum.
    pub fn from_bytes(id: PageId, bytes: [u8; PAGE_SIZE]) -> Result<Self, PageError> {
        let p = Self {
            id,
            bytes: Box::new(bytes),
        };
        if p.read_u32(OFF_MAGIC) != PAGE_MAGIC {
            return Err(PageError::Corrupt(format!("bad magic for page {}", id.0)));
        }
        let kind = PageKind::try_from(p.read_u16(OFF_KIND))?;
        let version = p.read_u16(OFF_FORMAT_VERSION);
        if version > PAGE_FORMAT_VERSION {
            return Err(PageError::Corrupt(format!(
                "unsupported page version {version}"
            )));
        }
        p.validate_structure(kind)?;
        let stored = p.read_u32(OFF_CHECKSUM);
        if version == PAGE_FORMAT_VERSION && stored == 0 {
            return Err(PageError::Corrupt(format!(
                "missing checksum for hardened page {}",
                id.0
            )));
        }
        if stored != 0 {
            p.verify_checksum()?;
        }
        Ok(p)
    }
    /// Returns the physical page kind.
    pub fn kind(&self) -> Result<PageKind, PageError> {
        PageKind::try_from(self.read_u16(OFF_KIND))
    }
    /// Replaces the physical page kind.
    pub fn set_kind(&mut self, kind: PageKind) {
        self.write_u16(OFF_KIND, kind as u16)
    }
    /// Returns the number of heap slots in the page header.
    pub fn slot_count(&self) -> u16 {
        self.read_u16(OFF_SLOT_COUNT)
    }
    /// Updates the heap slot count.
    pub fn set_slot_count(&mut self, v: u16) {
        self.write_u16(OFF_SLOT_COUNT, v)
    }
    /// Returns the first free byte after the slot directory.
    pub fn free_start(&self) -> u16 {
        self.read_u16(OFF_FREE_START)
    }
    /// Updates the first free byte after the slot directory.
    pub fn set_free_start(&mut self, v: u16) {
        self.write_u16(OFF_FREE_START, v)
    }
    /// Returns the first byte of the tuple area at the end of the page.
    pub fn free_end(&self) -> u16 {
        self.read_u16(OFF_FREE_END)
    }
    /// Updates the first byte of the tuple area.
    pub fn set_free_end(&mut self, v: u16) {
        self.write_u16(OFF_FREE_END, v)
    }
    /// Returns the WAL LSN associated with the page image.
    pub fn page_lsn(&self) -> Lsn {
        Lsn(self.read_u64(OFF_PAGE_LSN))
    }
    /// Updates the page LSN.
    pub fn set_page_lsn(&mut self, lsn: Lsn) {
        self.write_u64(OFF_PAGE_LSN, lsn.0)
    }
    /// Returns the payload after the common header.
    pub fn payload(&self) -> &[u8] {
        &self.bytes[PAGE_HEADER_SIZE..]
    }
    /// Returns mutable payload bytes after the common header.
    pub fn payload_mut(&mut self) -> &mut [u8] {
        &mut self.bytes[PAGE_HEADER_SIZE..]
    }
    /// Returns the complete encoded page.
    pub fn bytes(&self) -> &[u8; PAGE_SIZE] {
        &self.bytes
    }
    /// Returns mutable complete page bytes.
    pub fn bytes_mut(&mut self) -> &mut [u8; PAGE_SIZE] {
        &mut self.bytes
    }
    /// Upgrades the page to hardened format and writes its CRC32 checksum.
    pub fn seal_checksum(&mut self) {
        self.write_u16(OFF_FORMAT_VERSION, PAGE_FORMAT_VERSION);
        self.write_u32(OFF_CHECKSUM, 0);
        let c = checksum(&self.bytes);
        self.write_u32(OFF_CHECKSUM, c)
    }
    /// Verifies a non-zero checksum while logically zeroing the checksum field.
    pub fn verify_checksum(&self) -> Result<(), PageError> {
        let expected = self.read_u32(OFF_CHECKSUM);
        if expected == 0 {
            return Err(PageError::Corrupt(format!(
                "zero checksum for persisted page {}",
                self.id.0
            )));
        }
        let mut bytes = *self.bytes.clone();
        bytes[OFF_CHECKSUM..OFF_CHECKSUM + 4].fill(0);
        let actual = checksum(&bytes);
        if actual != expected {
            return Err(PageError::Corrupt(format!(
                "checksum mismatch for page {}: expected {expected:#x}, got {actual:#x}",
                self.id.0
            )));
        }
        Ok(())
    }
    /// Validates common free-space and heap slot-directory invariants.
    fn validate_structure(&self, kind: PageKind) -> Result<(), PageError> {
        let start = self.free_start() as usize;
        let end = self.free_end() as usize;
        if start < PAGE_HEADER_SIZE || start > end || end > PAGE_SIZE {
            return Err(PageError::Corrupt(format!(
                "invalid free-space bounds start={start} end={end}"
            )));
        }
        if kind == PageKind::Heap {
            let slots = self.slot_count() as usize;
            let dir_end = PAGE_HEADER_SIZE
                .checked_add(
                    slots
                        .checked_mul(4)
                        .ok_or_else(|| PageError::Corrupt("slot directory overflow".into()))?,
                )
                .ok_or_else(|| PageError::Corrupt("slot directory overflow".into()))?;
            if dir_end != start || dir_end > PAGE_SIZE {
                return Err(PageError::Corrupt("invalid heap slot directory".into()));
            }
            for slot in 0..slots {
                let o = PAGE_HEADER_SIZE + slot * 4;
                let tuple = self.read_u16(o) as usize;
                let len = self.read_u16(o + 2) as usize;
                let tuple_end = tuple
                    .checked_add(len)
                    .ok_or_else(|| PageError::Corrupt("slot range overflow".into()))?;
                if tuple < end || tuple_end > PAGE_SIZE {
                    return Err(PageError::Corrupt(format!(
                        "slot {slot} points outside tuple area"
                    )));
                }
            }
        }
        Ok(())
    }
    /// Reads a little-endian u16 from a fixed header range.
    fn read_u16(&self, o: usize) -> u16 {
        u16::from_le_bytes([self.bytes[o], self.bytes[o + 1]])
    }
    /// Reads a little-endian u32 from a fixed header range.
    fn read_u32(&self, o: usize) -> u32 {
        u32::from_le_bytes([
            self.bytes[o],
            self.bytes[o + 1],
            self.bytes[o + 2],
            self.bytes[o + 3],
        ])
    }
    /// Reads a little-endian u64 from a fixed header range.
    fn read_u64(&self, o: usize) -> u64 {
        let mut a = [0u8; 8];
        a.copy_from_slice(&self.bytes[o..o + 8]);
        u64::from_le_bytes(a)
    }
    /// Writes a little-endian u16 into a fixed header range.
    fn write_u16(&mut self, o: usize, v: u16) {
        self.bytes[o..o + 2].copy_from_slice(&v.to_le_bytes())
    }
    /// Writes a little-endian u32 into a fixed header range.
    fn write_u32(&mut self, o: usize, v: u32) {
        self.bytes[o..o + 4].copy_from_slice(&v.to_le_bytes())
    }
    /// Writes a little-endian u64 into a fixed header range.
    fn write_u64(&mut self, o: usize, v: u64) {
        self.bytes[o..o + 8].copy_from_slice(&v.to_le_bytes())
    }
}
/// Computes CRC32 over a page whose checksum field has already been zeroed.
fn checksum(bytes: &[u8; PAGE_SIZE]) -> u32 {
    let mut h = Hasher::new();
    h.update(bytes);
    h.finalize()
}
