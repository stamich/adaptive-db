//! `CREATE TABLE` and `ALTER TABLE` as values, and how they produce schema versions.
//!
//! Every change is checked twice: here, against the schema it applies to (names, types,
//! defaults, the primary key), and by the engine, against the rest of the catalog (foreign keys,
//! indexes that depend on a dropped field). The JVM checks the same rules earlier to give
//! messages in SQL terms, but only the engine's verdict counts.
use adb_core::{FieldId, SchemaEpoch, SchemaVersion, Value, ValueType};
use serde::{Deserialize, Serialize};

use crate::{
    limits::{MAX_FIELDS_PER_ENTITY, MAX_VERSIONS_PER_ENTITY},
    model::{same_name, EntitySchema, FieldDef, ForeignKey, SYSTEM_ENTITY_FIELD},
    validate::{validate_schema, validate_successor},
    SchemaError,
};

/// A field as `CREATE TABLE` or `ADD COLUMN` declares it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewField {
    /// Field name.
    pub name: String,
    /// Type of its values.
    pub value_type: ValueType,
    /// Whether NULL is allowed.
    pub nullable: bool,
    /// Constant default, if any.
    #[serde(default)]
    pub default: Option<Value>,
    /// Foreign-key hint, if any.
    #[serde(default)]
    pub references: Option<ForeignKey>,
    /// Explicit field id: only when a catalog written before 2.4.0 is imported (its ids must be
    /// kept, because its rows use them). New fields get the entity's next id.
    #[serde(default)]
    pub id: Option<FieldId>,
}

/// A table as `CREATE TABLE` declares it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntitySpec {
    /// Entity name.
    pub name: String,
    /// Fields in declaration order.
    pub fields: Vec<NewField>,
    /// Name of the primary-key field (a non-NULL `Int64`).
    pub primary_key: String,
    /// Explicit entity id: only for imported catalogs; new entities get the next id.
    #[serde(default)]
    pub entity_id: Option<u64>,
}

/// One `ALTER TABLE` change. The JSON form (`{"op":"add_field","field":{...}}`) is part of the
/// C ABI (`adb_alter_entity_json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum SchemaChange {
    /// `ADD COLUMN`: a nullable field, or a `NOT NULL` field with a default.
    AddField {
        /// The new field.
        field: NewField,
    },
    /// `RENAME COLUMN from TO to`: metadata only, the field keeps its id.
    RenameField {
        /// Current name.
        from: String,
        /// New name.
        to: String,
    },
    /// `DROP COLUMN name [RESTRICT]`: a logical drop, stored values are hidden.
    DropField {
        /// Field name.
        name: String,
    },
}

/// Version 1 of a new entity with id `entity_id`, published at `epoch`.
///
/// Field ids are 1, 2, 3, ... in declaration order, or the explicit ids of an imported
/// catalog (then every field must carry one).
pub fn create(
    spec: &EntitySpec,
    entity_id: u64,
    epoch: SchemaEpoch,
) -> Result<EntitySchema, SchemaError> {
    let explicit = spec
        .fields
        .iter()
        .filter(|field| field.id.is_some())
        .count();
    if explicit != 0 && explicit != spec.fields.len() {
        return Err(SchemaError::Invalid(
            "either every field or no field carries an explicit id".into(),
        ));
    }
    if spec.fields.len() > MAX_FIELDS_PER_ENTITY {
        return Err(SchemaError::Limit(format!(
            "an entity has at most {MAX_FIELDS_PER_ENTITY} fields"
        )));
    }
    let mut fields: Vec<FieldDef> = spec
        .fields
        .iter()
        .enumerate()
        .map(|(at, field)| FieldDef {
            id: field.id.unwrap_or(at as FieldId + 1),
            name: field.name.clone(),
            value_type: field.value_type,
            nullable: field.nullable,
            default: field.default.clone(),
            introduced_in: SchemaVersion::FIRST,
            dropped_in: None,
            references: field.references,
        })
        .collect();
    fields.sort_by_key(|field| field.id);
    let primary_key = fields
        .iter()
        .find(|field| same_name(&field.name, &spec.primary_key))
        .map(|field| field.id)
        .ok_or_else(|| SchemaError::Invalid(format!("unknown primary key {}", spec.primary_key)))?;
    let next_field_id = fields.iter().map(|field| field.id).max().unwrap_or(0) + 1;
    let schema = EntitySchema {
        entity_id,
        name: spec.name.clone(),
        version: SchemaVersion::FIRST,
        parent: None,
        epoch,
        primary_key,
        next_field_id,
        fields,
    };
    validate_schema(&schema)?;
    Ok(schema)
}

/// The version after `schema` that `change` produces, published at `epoch`.
pub fn apply(
    schema: &EntitySchema,
    change: &SchemaChange,
    epoch: SchemaEpoch,
) -> Result<EntitySchema, SchemaError> {
    if schema.version.0 >= MAX_VERSIONS_PER_ENTITY {
        return Err(SchemaError::Limit(format!(
            "{} has reached {MAX_VERSIONS_PER_ENTITY} schema versions",
            schema.name
        )));
    }
    let version = SchemaVersion(schema.version.0 + 1);
    let mut next = schema.clone();
    next.version = version;
    next.parent = Some(schema.version);
    next.epoch = epoch;
    let unknown = |column: &str| SchemaError::UnknownField {
        entity: schema.name.clone(),
        column: column.to_string(),
    };
    match change {
        SchemaChange::AddField { field } => {
            if field.id.is_some() {
                return Err(SchemaError::Invalid(
                    "ADD COLUMN assigns the field id itself".into(),
                ));
            }
            if schema.field(&field.name).is_some() {
                return Err(SchemaError::Invalid(format!(
                    "{} already has a column {}",
                    schema.name, field.name
                )));
            }
            if !field.nullable && field.default.is_none() {
                return Err(SchemaError::Invalid(format!(
                    "ADD COLUMN {} NOT NULL needs a DEFAULT: existing rows have no value",
                    field.name
                )));
            }
            if schema.fields.len() >= MAX_FIELDS_PER_ENTITY {
                return Err(SchemaError::Limit(format!(
                    "an entity has at most {MAX_FIELDS_PER_ENTITY} fields, dropped ones included"
                )));
            }
            let id = schema.next_field_id;
            if id == SYSTEM_ENTITY_FIELD || id == adb_core::SCHEMA_VERSION_FIELD {
                return Err(SchemaError::Limit("field ids are exhausted".into()));
            }
            next.fields.push(FieldDef {
                id,
                name: field.name.clone(),
                value_type: field.value_type,
                nullable: field.nullable,
                default: field.default.clone(),
                introduced_in: version,
                dropped_in: None,
                references: field.references,
            });
            next.next_field_id = id + 1;
        }
        SchemaChange::RenameField { from, to } => {
            let id = schema.field(from).ok_or_else(|| unknown(from))?.id;
            if let Some(other) = schema.field(to) {
                if other.id != id || other.name == *to {
                    return Err(SchemaError::Invalid(format!(
                        "{} already has a column {to}",
                        schema.name
                    )));
                }
            }
            field_mut(&mut next, id).name = to.clone();
        }
        SchemaChange::DropField { name } => {
            let id = schema.field(name).ok_or_else(|| unknown(name))?.id;
            if id == schema.primary_key {
                return Err(SchemaError::Invalid(format!(
                    "{name} is the primary key of {} and cannot be dropped",
                    schema.name
                )));
            }
            field_mut(&mut next, id).dropped_in = Some(version);
        }
    }
    validate_schema(&next)?;
    validate_successor(schema, &next)?;
    Ok(next)
}

/// The field `id` of `schema`, for an in-place change of a copy.
fn field_mut(schema: &mut EntitySchema, id: FieldId) -> &mut FieldDef {
    schema
        .fields
        .iter_mut()
        .find(|field| field.id == id)
        .expect("looked up above")
}
