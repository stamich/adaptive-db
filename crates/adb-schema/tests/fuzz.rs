//! Deterministic fuzzing of the schema layer: mutated definition and change documents, long
//! random change sequences, random rows through the adapter, and damaged catalog events. Every
//! input is either refused with an error or produces a valid result; nothing panics.
use adb_core::{Row, RowId, SchemaEpoch, SchemaVersion, Value, ValueType};
use adb_schema::{
    apply, create, validate::validate_chain, validate::validate_schema, CatalogEvent, EntitySchema,
    EntitySpec, NewField, RowAdapter, SchemaChange, SchemaRegistry, SYSTEM_ENTITY_FIELD,
};

/// Deterministic pseudo-random words (xorshift64).
struct Words(u64);

impl Words {
    /// Next word.
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A number in `0..bound`.
    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

/// A valid definition document.
const SPEC: &str = r#"{"name":"account","entity_id":3,"primary_key":"id","fields":[
 {"name":"id","value_type":"Int64","nullable":false},
 {"name":"balance","value_type":"Int64","nullable":false,"default":{"Int64":0}},
 {"name":"note","value_type":"String","nullable":true,"references":null}]}"#;

/// A valid change document of each kind.
const CHANGES: [&str; 3] = [
    r#"{"op":"add_field","field":{"name":"c","value_type":"String","nullable":false,"default":{"String":"EUR"}}}"#,
    r#"{"op":"rename_field","from":"balance","to":"amount"}"#,
    r#"{"op":"drop_field","name":"note"}"#,
];

/// Bytes a mutation inserts: JSON punctuation, digits, letters and a non-ASCII byte.
const ALPHABET: &[u8] = b"{}[]:,\"0123456789-.eEanulltrueINTnoteid \\\xff";

/// Mutates one to four random bytes of `text`.
fn mutate(words: &mut Words, text: &str) -> Vec<u8> {
    let mut bytes = text.as_bytes().to_vec();
    for _ in 0..1 + words.below(4) {
        let at = words.below(bytes.len());
        match words.below(3) {
            0 => bytes[at] = ALPHABET[words.below(ALPHABET.len())],
            1 => {
                bytes.remove(at);
            }
            _ => bytes.insert(at, ALPHABET[words.below(ALPHABET.len())]),
        }
    }
    bytes
}

/// Version 1 of the fixture definition.
fn base() -> EntitySchema {
    create(&serde_json::from_str(SPEC).unwrap(), 3, SchemaEpoch(1)).unwrap()
}

/// Mutated definitions decode to an error or create a valid schema (or are refused by it).
#[test]
fn mutated_definitions_never_panic() {
    let mut words = Words(0x5eed_0001);
    let mut created = 0;
    for _ in 0..20_000 {
        let bytes = mutate(&mut words, SPEC);
        let Ok(spec) = serde_json::from_slice::<EntitySpec>(&bytes) else {
            continue;
        };
        if let Ok(schema) = create(&spec, 3, SchemaEpoch(1)) {
            validate_schema(&schema).expect("created schemas are valid");
            created += 1;
        }
    }
    assert!(created > 0, "some mutations stay valid");
}

/// Mutated change documents decode to an error or apply to a valid successor (or are refused).
#[test]
fn mutated_changes_never_panic() {
    let mut words = Words(0x5eed_0002);
    let base = base();
    for _ in 0..20_000 {
        let text = CHANGES[words.below(CHANGES.len())];
        let bytes = mutate(&mut words, text);
        let Ok(change) = serde_json::from_slice::<SchemaChange>(&bytes) else {
            continue;
        };
        if let Ok(next) = apply(&base, &change, SchemaEpoch(2)) {
            validate_chain(&[base.clone(), next]).expect("applied changes are valid successors");
        }
    }
}

/// A random change of `schema`: names come from a small pool so that collisions, renames of
/// dropped fields and drops of the primary key all occur.
fn random_change(words: &mut Words) -> SchemaChange {
    const NAMES: [&str; 7] = ["id", "balance", "note", "a", "B", "c_1", "Ü"];
    let name = |words: &mut Words| NAMES[words.below(NAMES.len())].to_string();
    match words.below(3) {
        0 => {
            let value_type = [ValueType::Int64, ValueType::String, ValueType::Bool][words.below(3)];
            let default = match words.below(4) {
                0 => None,
                1 => Some(Value::Int64(words.next() as i64)),
                2 => Some(Value::String("x".repeat(words.below(2000)))),
                _ => Some(Value::Bool(true)),
            };
            SchemaChange::AddField {
                field: NewField {
                    name: name(words),
                    value_type,
                    nullable: words.below(2) == 0,
                    default,
                    references: None,
                    id: None,
                },
            }
        }
        1 => SchemaChange::RenameField {
            from: name(words),
            to: name(words),
        },
        _ => SchemaChange::DropField { name: name(words) },
    }
}

/// A random row: fields among the ids the history knows plus a few it does not, random values,
/// a random stamp up to one version past the latest.
fn random_row(words: &mut Words, latest: &EntitySchema) -> Row {
    let mut row = Row::new().with_schema_version(SchemaVersion(
        words.below(latest.version.0 as usize + 2) as u32,
    ));
    for _ in 0..words.below(8) {
        let field = words.below(latest.next_field_id as usize + 3) as u32;
        let value = match words.below(4) {
            0 => Value::Null,
            1 => Value::Int64(words.next() as i64),
            2 => Value::String("v".into()),
            _ => Value::Bool(false),
        };
        row.fields.insert(field, value);
    }
    row
}

/// Long random change sequences keep a valid chain, and the adapter maps any row to any known
/// version without panicking, keeping only fields of the target (and the system field).
#[test]
fn random_evolutions_keep_valid_chains_and_adapt_any_row() {
    let mut words = Words(0x5eed_0003);
    for _round in 0..20 {
        let mut versions = vec![base()];
        for step in 0..300 {
            let change = random_change(&mut words);
            let latest = versions.last().unwrap();
            if let Ok(next) = apply(latest, &change, SchemaEpoch(step + 2)) {
                versions.push(next);
            }
        }
        validate_chain(&versions).expect("every accepted change keeps the chain valid");
        let latest = versions.last().unwrap();
        let mut registry = SchemaRegistry::new();
        for version in &versions {
            registry
                .apply(version.epoch, &CatalogEvent::Schema(version.clone()))
                .expect("the registry accepts the chain it was built from");
        }
        for _ in 0..500 {
            let row = random_row(&mut words, latest);
            let to = SchemaVersion(1 + words.below(latest.version.0 as usize) as u32);
            let adapted = RowAdapter::compile(latest, row.schema_version, to).adapt(row.clone());
            let target = &versions[to.0 as usize - 1];
            for field in adapted.fields.keys() {
                assert!(
                    *field == SYSTEM_ENTITY_FIELD
                        || target
                            .field_by_id(*field)
                            .is_some_and(|f| f.is_active_in(to))
                        || row.schema_version.effective() == to,
                    "field {field} leaked into version {}",
                    to.0
                );
            }
            // Writes of random rows are refused or produce rows that fit the latest version.
            if let Ok(written) = registry.prepare_write(RowId::from_primary_key(3, 1), row) {
                assert_eq!(written.schema_version, latest.version);
                adb_schema::validate::check_row(latest, RowId::from_primary_key(3, 1), &written)
                    .expect("prepared rows fit the latest version");
            }
        }
    }
}

/// Damaged catalog events (bincode, as in the log) decode to an error or are validated by the
/// registry before they change anything.
#[test]
fn damaged_catalog_events_are_refused_or_valid() {
    let mut words = Words(0x5eed_0004);
    let event = CatalogEvent::Schema(base());
    let bytes = bincode::serialize(&event).unwrap();
    for _ in 0..20_000 {
        let mut damaged = bytes.clone();
        for _ in 0..1 + words.below(3) {
            let at = words.below(damaged.len());
            damaged[at] = words.next() as u8;
        }
        if words.below(10) == 0 {
            damaged.truncate(words.below(damaged.len()));
        }
        let Ok(decoded) = bincode::deserialize::<CatalogEvent>(&damaged) else {
            continue;
        };
        let mut registry = SchemaRegistry::new();
        let before = registry.state().clone();
        match registry.apply(SchemaEpoch(1), &decoded) {
            Ok(()) => {
                for schema in registry.entities() {
                    validate_schema(schema).expect("accepted events hold valid schemas");
                }
            }
            Err(_) => assert_eq!(registry.state(), &before, "a refused event changes nothing"),
        }
    }
}
