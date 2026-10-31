//! The rules of one schema version, of a step between two versions, and of a whole chain.
//!
//! [`validate_schema`] and [`validate_successor`] run for every change before it is published;
//! [`validate_chain`] runs when a persisted history is loaded, so a damaged registry or log
//! record is reported as corruption instead of being trusted.
use std::collections::HashSet;

use adb_core::{encode_key, Direction, SchemaVersion, ValueType, SCHEMA_VERSION_FIELD};

use crate::{
    limits::{MAX_DEFAULT_BYTES, MAX_FIELDS_PER_ENTITY, MAX_NAME_CHARS},
    model::{EntitySchema, FieldDef, SYSTEM_ENTITY_FIELD},
    SchemaError,
};

/// Checks an entity or field name: 1..=256 characters, a letter or `_` first, then letters,
/// digits and `_` (the identifiers the SQL tokenizer accepts).
pub fn validate_name(kind: &str, name: &str) -> Result<(), SchemaError> {
    let mut chars = name.chars();
    let valid_start = chars.next().is_some_and(|c| c.is_alphabetic() || c == '_');
    if !valid_start || !chars.all(|c| c.is_alphanumeric() || c == '_') {
        return Err(SchemaError::Invalid(format!(
            "{kind} name {name:?} must start with a letter or _ and hold only letters, digits and _"
        )));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(SchemaError::Limit(format!(
            "{kind} name exceeds {MAX_NAME_CHARS} characters"
        )));
    }
    Ok(())
}

/// Checks one version on its own: names, ids, types, defaults, the primary key.
pub fn validate_schema(schema: &EntitySchema) -> Result<(), SchemaError> {
    let invalid = |message: String| Err(SchemaError::Invalid(message));
    validate_name("entity", &schema.name)?;
    if schema.version.is_unversioned() {
        return invalid("schema versions start at 1".into());
    }
    if schema.parent
        != schema
            .version
            .0
            .checked_sub(1)
            .filter(|v| *v > 0)
            .map(SchemaVersion)
    {
        return invalid(format!(
            "version {} must derive from the version before it",
            schema.version.0
        ));
    }
    if schema.fields.is_empty() {
        return invalid(format!("{} has no fields", schema.name));
    }
    if schema.fields.len() > MAX_FIELDS_PER_ENTITY {
        return Err(SchemaError::Limit(format!(
            "an entity has at most {MAX_FIELDS_PER_ENTITY} fields"
        )));
    }
    let mut names = HashSet::new();
    let mut previous = None;
    for field in &schema.fields {
        validate_name("field", &field.name)?;
        if field.id == SYSTEM_ENTITY_FIELD || field.id == SCHEMA_VERSION_FIELD {
            return invalid(format!("field id {} is reserved", field.id));
        }
        if previous.is_some_and(|id| id >= field.id) {
            return invalid("fields must be in ascending id order without duplicates".into());
        }
        previous = Some(field.id);
        if field.id >= schema.next_field_id {
            return invalid(format!(
                "field id {} is not below the next id {}",
                field.id, schema.next_field_id
            ));
        }
        if field.introduced_in.is_unversioned() || field.introduced_in > schema.version {
            return invalid(format!("{} has an invalid introducing version", field.name));
        }
        if let Some(dropped) = field.dropped_in {
            if dropped <= field.introduced_in || dropped > schema.version {
                return invalid(format!("{} has an invalid dropping version", field.name));
            }
        }
        if field.is_active_in(schema.version) && !names.insert(field.name.to_lowercase()) {
            return invalid(format!(
                "{} has two columns named {}",
                schema.name, field.name
            ));
        }
        validate_default(field)?;
        if !field.nullable && field.default.is_none() && field.introduced_in > SchemaVersion::FIRST
        {
            return invalid(format!(
                "{} was added NOT NULL without a DEFAULT",
                field.name
            ));
        }
    }
    let Some(pk) = schema.field_by_id(schema.primary_key) else {
        return invalid(format!("{} has no primary-key field", schema.name));
    };
    if pk.value_type != ValueType::Int64 || pk.nullable {
        return invalid(format!("primary key {} must be a non-NULL BIGINT", pk.name));
    }
    if !pk.is_active_in(schema.version) {
        return invalid(format!("primary key {} cannot be dropped", pk.name));
    }
    Ok(())
}

/// Checks a field's default: the field's type (NULL is not a default; it is what no default
/// means) and at most [`MAX_DEFAULT_BYTES`] encoded.
fn validate_default(field: &FieldDef) -> Result<(), SchemaError> {
    let Some(default) = &field.default else {
        return Ok(());
    };
    if ValueType::of(default) != Some(field.value_type) {
        return Err(SchemaError::Invalid(format!(
            "the DEFAULT of {} must be a non-NULL {:?}",
            field.name, field.value_type
        )));
    }
    if let adb_core::Value::Float64(v) = default {
        if !v.is_finite() {
            return Err(SchemaError::Invalid(format!(
                "the DEFAULT of {} must be finite",
                field.name
            )));
        }
    }
    let bytes = encode_key([(default, Direction::Asc)]).len();
    if bytes > MAX_DEFAULT_BYTES {
        return Err(SchemaError::Limit(format!(
            "the DEFAULT of {} takes {bytes} bytes, at most {MAX_DEFAULT_BYTES}",
            field.name
        )));
    }
    Ok(())
}

/// Checks that `next` is a valid successor of `previous`: the next version of the same entity,
/// published later, with every existing field unchanged except its name and a new drop, the
/// primary key and the id counter kept, and new fields introduced in `next` only.
pub fn validate_successor(previous: &EntitySchema, next: &EntitySchema) -> Result<(), SchemaError> {
    let invalid = |message: String| Err(SchemaError::Invalid(message));
    if next.entity_id != previous.entity_id {
        return invalid("a successor belongs to the same entity".into());
    }
    if next.version.0 != previous.version.0 + 1 || next.parent != Some(previous.version) {
        return invalid(format!(
            "version {} cannot follow version {}",
            next.version.0, previous.version.0
        ));
    }
    if next.epoch <= previous.epoch {
        return invalid("a successor is published at a later epoch".into());
    }
    if next.primary_key != previous.primary_key {
        return invalid("the primary key never changes".into());
    }
    if next.next_field_id < previous.next_field_id {
        return invalid("field ids are never reused".into());
    }
    for old in &previous.fields {
        let Some(new) = next.field_by_id(old.id) else {
            return invalid(format!("field {} disappeared from the history", old.id));
        };
        let same_definition = new.value_type == old.value_type
            && new.nullable == old.nullable
            && new.default == old.default
            && new.introduced_in == old.introduced_in
            && new.references == old.references;
        let drop_ok = match (old.dropped_in, new.dropped_in) {
            (Some(a), Some(b)) => a == b,
            (None, None) => true,
            (None, Some(dropped)) => dropped == next.version,
            (Some(_), None) => false,
        };
        if !same_definition || !drop_ok {
            return invalid(format!(
                "field {} changed more than its name or drop",
                old.id
            ));
        }
        if old.dropped_in.is_some() && new.name != old.name {
            return invalid(format!("dropped field {} cannot be renamed", old.id));
        }
    }
    for new in &next.fields {
        if previous.field_by_id(new.id).is_none()
            && (new.introduced_in != next.version || new.id < previous.next_field_id)
        {
            return invalid(format!(
                "new field {} must use a fresh id and the new version",
                new.id
            ));
        }
    }
    Ok(())
}

/// Checks a whole history of one entity, oldest first.
pub fn validate_chain(versions: &[EntitySchema]) -> Result<(), SchemaError> {
    let corrupt = |error: SchemaError| SchemaError::Corrupt(error.to_string());
    let Some(first) = versions.first() else {
        return Err(SchemaError::Corrupt("an empty schema history".into()));
    };
    if first.version != SchemaVersion::FIRST {
        return Err(SchemaError::Corrupt(format!(
            "the history of {} does not start at version 1",
            first.name
        )));
    }
    validate_schema(first).map_err(corrupt)?;
    for pair in versions.windows(2) {
        validate_schema(&pair[1]).map_err(corrupt)?;
        validate_successor(&pair[0], &pair[1]).map_err(corrupt)?;
    }
    Ok(())
}
