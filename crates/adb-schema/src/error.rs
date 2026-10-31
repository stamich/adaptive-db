//! Errors of schema validation and evolution.
use thiserror::Error;

/// Why a schema, a change or a row was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SchemaError {
    /// The definition or change breaks a schema rule (the message names it).
    #[error("invalid schema: {0}")]
    Invalid(String),
    /// A limit of [`crate::limits`] was exceeded.
    #[error("schema limit exceeded: {0}")]
    Limit(String),
    /// A named field does not exist (or is dropped) in the schema.
    #[error("unknown column {column} in {entity}")]
    UnknownField {
        /// Entity name.
        entity: String,
        /// Column name.
        column: String,
    },
    /// A row does not fit its schema (unknown, dropped or mistyped field, missing value).
    #[error("row does not fit {entity}: {reason}")]
    Row {
        /// Entity name.
        entity: String,
        /// What is wrong.
        reason: String,
    },
    /// A persisted history failed validation (a damaged registry or log record).
    #[error("corrupt schema history: {0}")]
    Corrupt(String),
}
