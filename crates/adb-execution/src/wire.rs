//! ADB Batch Format v1 encoder with checked lengths and bounded output size.
use crate::{ColumnVector, ExecutionError, RecordBatch};
/// Magic word beginning each encoded batch.
pub const BATCH_MAGIC: u32 = 0x4144_4242;
/// Batch wire format version.
pub const BATCH_FORMAT_VERSION: u16 = 1;
/// Maximum encoded batch accepted by the Milestone 1.7.1 FFI boundary.
pub const MAX_BATCH_WIRE_BYTES: usize = 64 * 1024 * 1024;
/// Encodes a record batch after validating row/column lengths and all u32 wire lengths.
pub fn encode_batch_v1(batch: &RecordBatch) -> Result<Vec<u8>, ExecutionError> {
    let rows = u32::try_from(batch.len())
        .map_err(|_| ExecutionError::Wire("row count exceeds u32".into()))?;
    let columns = u32::try_from(batch.columns.len())
        .map_err(|_| ExecutionError::Wire("column count exceeds u32".into()))?;
    for c in &batch.columns {
        if c.len() != batch.len() {
            return Err(ExecutionError::Wire(
                "column length differs from row count".into(),
            ));
        }
    }
    let mut out = Vec::new();
    put_u32(&mut out, BATCH_MAGIC);
    put_u16(&mut out, BATCH_FORMAT_VERSION);
    put_u16(&mut out, 0);
    put_u32(&mut out, rows);
    put_u32(&mut out, columns);
    for id in &batch.row_ids {
        extend(&mut out, &id.0.to_le_bytes())?;
    }
    for c in &batch.columns {
        put_u32(&mut out, c.field_id());
        out.push(c.physical_type() as u8);
        extend(&mut out, &[0, 0, 0])?;
        let (bitmap, payload) = encode_column(c)?;
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
/// Encodes one typed column into null bitmap and payload bytes.
fn encode_column(c: &ColumnVector) -> Result<(Vec<u8>, Vec<u8>), ExecutionError> {
    match c {
        ColumnVector::Bool { values, .. } => Ok((
            null_bitmap(values.iter().map(Option::is_none))?,
            values.iter().map(|v| v.unwrap_or(false) as u8).collect(),
        )),
        ColumnVector::Int64 { values, .. } => {
            let b = null_bitmap(values.iter().map(Option::is_none))?;
            let mut p = Vec::new();
            for v in values {
                extend(&mut p, &v.unwrap_or_default().to_le_bytes())?;
            }
            Ok((b, p))
        }
        ColumnVector::Float64 { values, .. } => {
            let b = null_bitmap(values.iter().map(Option::is_none))?;
            let mut p = Vec::new();
            for v in values {
                extend(&mut p, &v.unwrap_or_default().to_le_bytes())?;
            }
            Ok((b, p))
        }
        ColumnVector::String { values, .. } => {
            let b = null_bitmap(values.iter().map(Option::is_none))?;
            let refs = values
                .iter()
                .map(|v| v.as_deref().map(str::as_bytes))
                .collect::<Vec<_>>();
            Ok((b, encode_variable(&refs)?))
        }
        ColumnVector::Bytes { values, .. } => {
            let b = null_bitmap(values.iter().map(Option::is_none))?;
            let refs = values.iter().map(|v| v.as_deref()).collect::<Vec<_>>();
            Ok((b, encode_variable(&refs)?))
        }
    }
}
/// Encodes offsets plus concatenated bytes for String/Bytes columns.
fn encode_variable(values: &[Option<&[u8]>]) -> Result<Vec<u8>, ExecutionError> {
    let mut data = Vec::new();
    let mut offsets = Vec::with_capacity(values.len().saturating_add(1));
    offsets.push(0u32);
    for v in values {
        if let Some(v) = v {
            extend(&mut data, v)?;
        }
        offsets.push(
            u32::try_from(data.len())
                .map_err(|_| ExecutionError::Wire("variable column exceeds 4GiB".into()))?,
        );
    }
    let mut p = Vec::new();
    for o in offsets {
        put_u32(&mut p, o);
    }
    extend(&mut p, &data)?;
    Ok(p)
}
/// Builds a compact null bitmap without overflowing length arithmetic.
fn null_bitmap(flags: impl Iterator<Item = bool>) -> Result<Vec<u8>, ExecutionError> {
    let f = flags.collect::<Vec<_>>();
    let len = f
        .len()
        .checked_add(7)
        .ok_or_else(|| ExecutionError::Wire("null bitmap length overflow".into()))?
        / 8;
    let mut b = vec![0u8; len];
    for (i, n) in f.into_iter().enumerate() {
        if n {
            b[i / 8] |= 1 << (i % 8);
        }
    }
    Ok(b)
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
/// Appends a little-endian u16 to the wire buffer.
fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes())
}
/// Appends a little-endian u32 to the wire buffer.
fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes())
}
