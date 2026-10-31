//! Size bounds of schemas. They match the bounds of the 2.3.0 JVM catalog, so every catalog it
//! wrote can be imported.

/// Most fields of one entity, dropped ones included (their ids are never reused).
pub const MAX_FIELDS_PER_ENTITY: usize = 4096;
/// Most versions of one entity.
pub const MAX_VERSIONS_PER_ENTITY: u32 = 65_535;
/// Most entities of one database (dropped ones included; their ids are never reused).
pub const MAX_ENTITIES: usize = 10_000;
/// Longest entity or field name, in characters.
pub const MAX_NAME_CHARS: usize = 256;
/// Largest default value, in bytes of its order-preserving key encoding.
pub const MAX_DEFAULT_BYTES: usize = 1024;
