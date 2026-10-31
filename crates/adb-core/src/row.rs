//! Logical row representation: a sparse map from field id to value, stamped with the schema
//! version it was written under.
//!
//! # Encodings
//!
//! The binary encoding (bincode: log records, current and version stores) of a row is the map of
//! its fields. Since data format 3 (Milestone 2.4.0) a row written under a schema version also
//! carries one extra map entry under the reserved key [`SCHEMA_VERSION_FIELD`] holding the version
//! as an `Int64`. A row without that entry (every row written before 2.4.0, and rows of entities
//! without a registered schema) decodes as version 0, "unversioned". Format 3 is therefore a
//! superset of format 2: a 2.3.0 directory is upgraded by stamping its marker, without rewriting
//! its log, and every log position stays valid.
//!
//! The JSON encoding (C ABI) is `{"fields": {"1": {"Int64": 5}}, "schema_version": 3}`;
//! `schema_version` is omitted when 0 and optional on input.
use std::{collections::BTreeMap, fmt};

use serde::{
    de::{self, MapAccess, SeqAccess, Visitor},
    ser::{SerializeMap, SerializeStruct},
    Deserialize, Deserializer, Serialize, Serializer,
};

use crate::{FieldId, SchemaVersion, Value};

/// Reserved field id that carries the schema version in the binary encoding of a row. It is
/// never a user field: schemas assign ids below it and writes that use it are refused.
pub const SCHEMA_VERSION_FIELD: FieldId = FieldId::MAX;

/// One logical row. Fields are kept ordered so encodings are deterministic.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    /// Field values by field id. An absent field and a field holding [`Value::Null`] are
    /// different: an absent field may read as a schema default, an explicit NULL never does.
    pub fields: BTreeMap<FieldId, Value>,
    /// Schema version of the entity the row was written under; 0 = unversioned (written before
    /// 2.4.0, or an entity without a registered schema).
    pub schema_version: SchemaVersion,
}

impl Row {
    /// Creates an empty, unversioned row.
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder-style setter used mostly by tests and examples.
    pub fn with_field(mut self, field_id: FieldId, value: Value) -> Self {
        self.fields.insert(field_id, value);
        self
    }

    /// Builder-style setter of the schema version.
    pub fn with_schema_version(mut self, version: SchemaVersion) -> Self {
        self.schema_version = version;
        self
    }

    /// Returns the value of `field_id`, if present.
    pub fn get(&self, field_id: FieldId) -> Option<&Value> {
        self.fields.get(&field_id)
    }
}

/// JSON shape of a row (the derived form of 2.3.0 plus the optional version).
#[derive(Serialize, Deserialize)]
struct RowJson {
    /// Field values.
    fields: BTreeMap<FieldId, Value>,
    /// Schema version; omitted when 0.
    #[serde(default, skip_serializing_if = "SchemaVersion::is_unversioned")]
    schema_version: SchemaVersion,
}

/// The binary field map of a row: its fields plus the version entry (see the module docs).
struct VersionedFields<'a>(&'a Row);

impl Serialize for VersionedFields<'_> {
    /// Writes the fields in key order, then the version entry when the row is versioned.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let row = self.0;
        let versioned = !row.schema_version.is_unversioned();
        let mut map = serializer.serialize_map(Some(row.fields.len() + usize::from(versioned)))?;
        for (field, value) in &row.fields {
            map.serialize_entry(field, value)?;
        }
        if versioned {
            map.serialize_entry(
                &SCHEMA_VERSION_FIELD,
                &Value::Int64(i64::from(row.schema_version.0)),
            )?;
        }
        map.end()
    }
}

impl Serialize for Row {
    /// JSON for human-readable formats, the versioned field map otherwise.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            RowJson {
                fields: self.fields.clone(),
                schema_version: self.schema_version,
            }
            .serialize(serializer)
        } else {
            let mut row = serializer.serialize_struct("Row", 1)?;
            row.serialize_field("fields", &VersionedFields(self))?;
            row.end()
        }
    }
}

impl<'de> Deserialize<'de> for Row {
    /// Reads either encoding (see the module docs).
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if deserializer.is_human_readable() {
            let json = RowJson::deserialize(deserializer)?;
            Ok(Row {
                fields: json.fields,
                schema_version: json.schema_version,
            })
        } else {
            deserializer.deserialize_struct("Row", &["fields"], RowVisitor)
        }
    }
}

/// Visitor of the binary struct (one element: the versioned field map).
struct RowVisitor;

impl<'de> Visitor<'de> for RowVisitor {
    type Value = Row;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a row")
    }

    /// The struct's single element.
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Row, A::Error> {
        seq.next_element_seed(FieldsSeed)?
            .ok_or_else(|| de::Error::invalid_length(0, &self))
    }
}

/// Reads the versioned field map into a [`Row`].
struct FieldsSeed;

impl<'de> de::DeserializeSeed<'de> for FieldsSeed {
    type Value = Row;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Row, D::Error> {
        deserializer.deserialize_map(FieldsVisitor)
    }
}

/// Visitor of the versioned field map.
struct FieldsVisitor;

impl<'de> Visitor<'de> for FieldsVisitor {
    type Value = Row;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a map of field values")
    }

    /// Collects the fields; the reserved key becomes the schema version.
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Row, A::Error> {
        let mut row = Row::new();
        while let Some((field, value)) = map.next_entry::<FieldId, Value>()? {
            if field == SCHEMA_VERSION_FIELD {
                row.schema_version = match value {
                    Value::Int64(v) if (1..=i64::from(u32::MAX)).contains(&v) => {
                        SchemaVersion(v as u32)
                    }
                    other => {
                        return Err(de::Error::custom(format!(
                            "invalid row schema version {other:?}"
                        )))
                    }
                };
            } else {
                row.fields.insert(field, value);
            }
        }
        Ok(row)
    }
}

/// Unit tests of the row encodings.
#[cfg(test)]
mod tests {
    use super::*;

    /// The 2.3.0 encoding of a row: the bare field map.
    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct LegacyRow {
        fields: BTreeMap<FieldId, Value>,
    }

    /// An unversioned row encodes exactly as in 2.3.0, and a 2.3.0 row decodes as unversioned.
    #[test]
    fn unversioned_rows_keep_the_legacy_encoding() {
        let row = Row::new()
            .with_field(1, Value::Int64(5))
            .with_field(2, Value::String("x".into()));
        let legacy = LegacyRow {
            fields: row.fields.clone(),
        };
        let bytes = bincode::serialize(&row).unwrap();
        assert_eq!(bytes, bincode::serialize(&legacy).unwrap());
        assert_eq!(bincode::deserialize::<Row>(&bytes).unwrap(), row);
    }

    /// A versioned row round-trips in both encodings; the reserved key never shows as a field.
    #[test]
    fn versioned_rows_round_trip() {
        let row = Row::new()
            .with_field(3, Value::Null)
            .with_schema_version(SchemaVersion(7));
        let decoded: Row = bincode::deserialize(&bincode::serialize(&row).unwrap()).unwrap();
        assert_eq!(decoded, row);
        assert!(!decoded.fields.contains_key(&SCHEMA_VERSION_FIELD));
        let json = serde_json::to_string(&row).unwrap();
        assert_eq!(json, r#"{"fields":{"3":"Null"},"schema_version":7}"#);
        assert_eq!(serde_json::from_str::<Row>(&json).unwrap(), row);
        let plain: Row = serde_json::from_str(r#"{"fields":{"1":{"Int64":2}}}"#).unwrap();
        assert!(plain.schema_version.is_unversioned());
    }

    /// A damaged version entry is an error, not a silently wrong version.
    #[test]
    fn invalid_versions_are_rejected() {
        let mut fields = BTreeMap::new();
        fields.insert(SCHEMA_VERSION_FIELD, Value::String("seven".into()));
        let bytes = bincode::serialize(&LegacyRow { fields }).unwrap();
        assert!(bincode::deserialize::<Row>(&bytes).is_err());
    }
}
