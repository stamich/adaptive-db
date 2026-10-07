//! ADB Batch Format v2 encoder with checked lengths and bounded output size.
//!
//! v2 (Milestone 2.1) differs from v1 only in the header: the former reserved `u16` is now a
//! flags word, and row ids are present only when [`FLAG_ROW_IDS`] is set. The `u32` column id of
//! each column header is the output [`crate::SlotId`] (in v1 it was a storage field id).

use crate::{ColumnVector, ExecutionError, RecordBatch};

/// Magic word beginning each encoded batch.
pub const BATCH_MAGIC: u32 = 0x4144_4242;
/// Batch wire format version.
pub const BATCH_FORMAT_VERSION: u16 = 2;
/// Header flag: one 16-byte little-endian row id per row follows the header.
pub const FLAG_ROW_IDS: u16 = 0x0001;
/// Maximum encoded batch accepted by the hardened FFI boundary.
pub const MAX_BATCH_WIRE_BYTES: usize = 64 * 1024 * 1024;

/// Encodes a record batch after validating row/column lengths and all `u32` wire lengths.
pub fn encode_batch(batch: &RecordBatch) -> Result<Vec<u8>, ExecutionError> {
    let rows = u32::try_from(batch.len())
        .map_err(|_| ExecutionError::Wire("row count exceeds u32".into()))?;
    let columns = u32::try_from(batch.columns.len())
        .map_err(|_| ExecutionError::Wire("column count exceeds u32".into()))?;

    for column in &batch.columns {
        if column.len() != batch.len() {
            return Err(ExecutionError::Wire(
                "column length differs from row count".into(),
            ));
        }
    }

    if batch
        .row_ids
        .as_ref()
        .is_some_and(|row_ids| row_ids.len() != batch.len())
    {
        return Err(ExecutionError::Wire(
            "row id count differs from row count".into(),
        ));
    }

    let mut out = Vec::new();
    put_u32(&mut out, BATCH_MAGIC);
    put_u16(&mut out, BATCH_FORMAT_VERSION);
    put_u16(
        &mut out,
        if batch.row_ids.is_some() {
            FLAG_ROW_IDS
        } else {
            0
        },
    );
    put_u32(&mut out, rows);
    put_u32(&mut out, columns);

    for row_id in batch.row_ids.iter().flatten() {
        extend(&mut out, &row_id.0.to_le_bytes())?;
    }

    for column in &batch.columns {
        put_u32(&mut out, column.slot().0);
        out.push(column.physical_type() as u8);
        extend(&mut out, &[0, 0, 0])?;

        let (bitmap, payload) = encode_column(column)?;
        put_u32(
            &mut out,
            u32::try_from(bitmap.len())
                .map_err(|_| ExecutionError::Wire("null bitmap exceeds u32".into()))?,
        );
        put_u32(
            &mut out,
            u32::try_from(payload.len())
                .map_err(|_| ExecutionError::Wire("column payload exceeds u32".into()))?,
        );
        extend(&mut out, &bitmap)?;
        extend(&mut out, &payload)?;
    }

    if out.len() > MAX_BATCH_WIRE_BYTES {
        return Err(ExecutionError::Wire(format!(
            "encoded batch exceeds {MAX_BATCH_WIRE_BYTES} bytes"
        )));
    }
    Ok(out)
}

/// Encodes one typed column into null-bitmap and payload bytes.
fn encode_column(column: &ColumnVector) -> Result<(Vec<u8>, Vec<u8>), ExecutionError> {
    match column {
        ColumnVector::Bool { values, .. } => Ok((
            null_bitmap(values.iter().map(Option::is_none))?,
            values
                .iter()
                .map(|value| value.unwrap_or(false) as u8)
                .collect(),
        )),
        ColumnVector::Int64 { values, .. } => {
            let bitmap = null_bitmap(values.iter().map(Option::is_none))?;
            let mut payload = Vec::new();
            for value in values {
                extend(&mut payload, &value.unwrap_or_default().to_le_bytes())?;
            }
            Ok((bitmap, payload))
        }
        ColumnVector::Float64 { values, .. } => {
            let bitmap = null_bitmap(values.iter().map(Option::is_none))?;
            let mut payload = Vec::new();
            for value in values {
                extend(&mut payload, &value.unwrap_or_default().to_le_bytes())?;
            }
            Ok((bitmap, payload))
        }
        ColumnVector::String { values, .. } => {
            let bitmap = null_bitmap(values.iter().map(Option::is_none))?;
            let refs = values
                .iter()
                .map(|value| value.as_deref().map(str::as_bytes))
                .collect::<Vec<_>>();
            Ok((bitmap, encode_variable(&refs)?))
        }
        ColumnVector::Bytes { values, .. } => {
            let bitmap = null_bitmap(values.iter().map(Option::is_none))?;
            let refs = values
                .iter()
                .map(|value| value.as_deref())
                .collect::<Vec<_>>();
            Ok((bitmap, encode_variable(&refs)?))
        }
    }
}

/// Encodes offsets plus concatenated bytes for String/Bytes columns.
fn encode_variable(values: &[Option<&[u8]>]) -> Result<Vec<u8>, ExecutionError> {
    let mut data = Vec::new();
    let mut offsets = Vec::with_capacity(values.len().saturating_add(1));
    offsets.push(0u32);
    for value in values {
        if let Some(value) = value {
            extend(&mut data, value)?;
        }
        offsets.push(
            u32::try_from(data.len())
                .map_err(|_| ExecutionError::Wire("variable column exceeds 4GiB".into()))?,
        );
    }

    let mut payload = Vec::new();
    for offset in offsets {
        put_u32(&mut payload, offset);
    }
    extend(&mut payload, &data)?;
    Ok(payload)
}

/// Builds a compact null bitmap without overflowing length arithmetic.
fn null_bitmap(flags: impl Iterator<Item = bool>) -> Result<Vec<u8>, ExecutionError> {
    let flags = flags.collect::<Vec<_>>();
    let len = flags
        .len()
        .checked_add(7)
        .ok_or_else(|| ExecutionError::Wire("null bitmap length overflow".into()))?
        / 8;
    let mut bitmap = vec![0u8; len];
    for (index, is_null) in flags.into_iter().enumerate() {
        if is_null {
            bitmap[index / 8] |= 1 << (index % 8);
        }
    }
    Ok(bitmap)
}

/// Extends an output vector while enforcing the hardened batch-size ceiling.
fn extend(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), ExecutionError> {
    let next = out
        .len()
        .checked_add(bytes.len())
        .ok_or_else(|| ExecutionError::Wire("batch length overflow".into()))?;
    if next > MAX_BATCH_WIRE_BYTES {
        return Err(ExecutionError::Wire(format!(
            "encoded batch exceeds {MAX_BATCH_WIRE_BYTES} bytes"
        )));
    }
    out.extend_from_slice(bytes);
    Ok(())
}

/// Appends a little-endian `u16` to the wire buffer.
fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// Appends a little-endian `u32` to the wire buffer.
fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}
