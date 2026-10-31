//! One immutable version of an entity's schema.
use adb_core::{FieldId, SchemaEpoch, SchemaVersion, Value, ValueType};
use serde::{Deserialize, Serialize};

/// Field 0 is reserved for the system: the JVM gateway stores the entity id in it. Schemas never
/// declare it, row validation always accepts it and adaptation always keeps it.
pub const SYSTEM_ENTITY_FIELD: FieldId = 0;

/// A declared, not enforced foreign key: the field holds primary keys of another entity (an
/// optimizer hint since 2.2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForeignKey {
    /// Referenced entity.
    pub entity_id: u64,
    /// Referenced field (that entity's primary key).
    pub field_id: FieldId,
}

/// One field of an entity across its whole life: the version that introduced it and, once
/// dropped, the version that dropped it. Only `name` and `dropped_in` ever change between
/// versions; everything else is fixed when the field is added.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldDef {
    /// Stable identifier; never reused within the entity.
    pub id: FieldId,
    /// Current name (as declared; compared case-insensitively).
    pub name: String,
    /// Type of its non-NULL values.
    pub value_type: ValueType,
    /// Whether NULL is allowed.
    pub nullable: bool,
    /// Constant default. A row written before `introduced_in` reads it (a *virtual* default:
    /// nothing is rewritten); an insert that omits the field stores it.
    pub default: Option<Value>,
    /// Version that added the field.
    pub introduced_in: SchemaVersion,
    /// Version that dropped it (logical drop: stored values are hidden, not removed).
    pub dropped_in: Option<SchemaVersion>,
    /// Foreign-key hint.
    pub references: Option<ForeignKey>,
}

impl FieldDef {
    /// Whether the field exists in `version`: added at or before it and not dropped by then.
    pub fn is_active_in(&self, version: SchemaVersion) -> bool {
        self.introduced_in <= version && self.dropped_in.is_none_or(|dropped| dropped > version)
    }
}

/// One version of one entity's schema. Published versions are immutable; a change produces a
/// new value with the next version ([`crate::apply`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntitySchema {
    /// Entity id (the high 64 bits of its row keys).
    pub entity_id: u64,
    /// Entity (table) name in this version.
    pub name: String,
    /// This version.
    pub version: SchemaVersion,
    /// The version it was derived from (`version - 1`), `None` for version 1.
    pub parent: Option<SchemaVersion>,
    /// Catalog epoch at which this version was published.
    pub epoch: SchemaEpoch,
    /// Primary-key field (a non-NULL `Int64`; never dropped).
    pub primary_key: FieldId,
    /// Id the next added field gets; greater than every id ever used.
    pub next_field_id: FieldId,
    /// Every field ever added, dropped ones included, in id order.
    pub fields: Vec<FieldDef>,
}

impl EntitySchema {
    /// The fields active in this version, in id order.
    pub fn active_fields(&self) -> impl Iterator<Item = &FieldDef> {
        self.fields
            .iter()
            .filter(move |field| field.is_active_in(self.version))
    }

    /// The field with id `id`, active or dropped.
    pub fn field_by_id(&self, id: FieldId) -> Option<&FieldDef> {
        self.fields
            .binary_search_by_key(&id, |field| field.id)
            .ok()
            .map(|at| &self.fields[at])
    }

    /// The active field named `name` (case-insensitive).
    pub fn field(&self, name: &str) -> Option<&FieldDef> {
        self.active_fields()
            .find(|field| same_name(&field.name, name))
    }

    /// Whether `id` names a field active in this version.
    pub fn is_active(&self, id: FieldId) -> bool {
        self.field_by_id(id)
            .is_some_and(|field| field.is_active_in(self.version))
    }

    /// The primary-key field.
    pub fn primary_key_field(&self) -> &FieldDef {
        self.field_by_id(self.primary_key)
            .expect("validated schemas name an existing primary key")
    }
}

/// Whether two entity or field names are the same name: names compare case-insensitively (as
/// SQL identifiers do), Unicode letters included.
pub fn same_name(a: &str, b: &str) -> bool {
    a == b || a.to_lowercase() == b.to_lowercase()
}
