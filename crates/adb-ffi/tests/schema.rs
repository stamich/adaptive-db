//! Entity schemas over the C ABI (ABI 7): create, alter, drop, list, history, write validation
//! and status mapping.
use std::ptr;

use adb_ffi::{
    adb_alter_entity_json, adb_batch_data, adb_batch_len, adb_batch_release, adb_close,
    adb_create_entity_json, adb_current_schema_epoch, adb_drop_entity, adb_insert_row_json,
    adb_last_error_len, adb_last_error_ptr, adb_list_schemas_json, adb_open, adb_read_changes_json,
    adb_schema_history_json,
    handle::{AdbBatchHandle, AdbDatabaseHandle},
    AdbStatus,
};
use serde_json::Value;
use tempfile::{tempdir, TempDir};

/// The `account` table used by the tests: `id` (primary key) and `balance` with a default.
const ACCOUNT: &str = r#"{"name":"account","entity_id":3,"primary_key":"id","fields":[
    {"name":"id","value_type":"Int64","nullable":false},
    {"name":"balance","value_type":"Int64","nullable":false,"default":{"Int64":0}}]}"#;

/// Opens a fresh database over the ABI.
fn open() -> (TempDir, *mut AdbDatabaseHandle) {
    let dir = tempdir().unwrap();
    let path = dir.path().to_string_lossy().as_bytes().to_vec();
    let mut handle = ptr::null_mut();
    assert_eq!(
        adb_open(path.as_ptr(), path.len(), &mut handle),
        AdbStatus::Ok
    );
    (dir, handle)
}

/// Decodes and releases a JSON buffer.
fn take_json(buffer: *mut AdbBatchHandle) -> Value {
    let bytes =
        unsafe { std::slice::from_raw_parts(adb_batch_data(buffer), adb_batch_len(buffer)) }
            .to_vec();
    assert_eq!(adb_batch_release(buffer), AdbStatus::Ok);
    serde_json::from_slice(&bytes).unwrap()
}

/// The current thread's last error message.
fn last_error() -> String {
    let bytes = unsafe { std::slice::from_raw_parts(adb_last_error_ptr(), adb_last_error_len()) };
    String::from_utf8_lossy(bytes).into_owned()
}

/// Calls `adb_create_entity_json`; returns the status and the decoded schema on success.
fn create(db: *mut AdbDatabaseHandle, spec: &str) -> (AdbStatus, Option<Value>) {
    let mut out = ptr::null_mut();
    let status = adb_create_entity_json(db, spec.as_ptr(), spec.len(), &mut out);
    (status, (status == AdbStatus::Ok).then(|| take_json(out)))
}

/// Calls `adb_alter_entity_json`; returns the status and the decoded schema on success.
fn alter(db: *mut AdbDatabaseHandle, request: &str) -> (AdbStatus, Option<Value>) {
    let mut out = ptr::null_mut();
    let status = adb_alter_entity_json(db, request.as_ptr(), request.len(), &mut out);
    if status != AdbStatus::Ok {
        assert!(out.is_null(), "no buffer on failure");
    }
    (status, (status == AdbStatus::Ok).then(|| take_json(out)))
}

/// The catalog epoch.
fn epoch(db: *mut AdbDatabaseHandle) -> u64 {
    let mut epoch = u64::MAX;
    assert_eq!(adb_current_schema_epoch(db, &mut epoch), AdbStatus::Ok);
    epoch
}

/// Inserts `row` (fields JSON) as primary key `pk` of entity 3.
fn insert(db: *mut AdbDatabaseHandle, pk: u64, fields: &str) -> AdbStatus {
    let row = format!(r#"{{"fields":{fields}}}"#);
    let mut ts = 0;
    adb_insert_row_json(db, 3, pk, row.as_ptr(), row.len(), &mut ts)
}

/// Field names of a schema, active fields only.
fn active_names(schema: &Value) -> Vec<String> {
    schema["fields"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|field| field["dropped_in"].is_null())
        .map(|field| field["name"].as_str().unwrap().to_string())
        .collect()
}

/// The whole DDL cycle: create, alter (add, rename, drop), list, history and drop; every step
/// advances the catalog epoch.
#[test]
fn ddl_round_trip() {
    let (_dir, db) = open();
    let start = epoch(db);

    let (status, created) = create(db, ACCOUNT);
    assert_eq!(status, AdbStatus::Ok, "{}", last_error());
    let created = created.unwrap();
    assert_eq!(created["entity_id"], 3);
    assert_eq!(created["version"], 1);
    assert!(created["parent"].is_null());
    assert_eq!(created["primary_key"], 1);
    assert_eq!(
        created["fields"][1]["default"],
        serde_json::json!({"Int64":0})
    );
    assert_eq!(epoch(db), start + 1);

    let (status, added) = alter(
        db,
        r#"{"entity_id":3,"change":{"op":"add_field","field":
            {"name":"currency","value_type":"String","nullable":false,"default":{"String":"EUR"}}}}"#,
    );
    assert_eq!(status, AdbStatus::Ok, "{}", last_error());
    let added = added.unwrap();
    assert_eq!(added["version"], 2);
    assert_eq!(added["parent"], 1);
    assert_eq!(added["fields"][2]["introduced_in"], 2);

    let (status, renamed) = alter(
        db,
        r#"{"entity_id":3,"change":{"op":"rename_field","from":"balance","to":"amount"}}"#,
    );
    assert_eq!(status, AdbStatus::Ok, "{}", last_error());
    assert_eq!(
        active_names(&renamed.unwrap()),
        ["id", "amount", "currency"]
    );

    let (status, dropped) = alter(
        db,
        r#"{"entity_id":3,"change":{"op":"drop_field","name":"currency"}}"#,
    );
    assert_eq!(status, AdbStatus::Ok, "{}", last_error());
    let dropped = dropped.unwrap();
    assert_eq!(dropped["version"], 4);
    assert_eq!(dropped["fields"][2]["dropped_in"], 4);
    assert_eq!(active_names(&dropped), ["id", "amount"]);
    assert_eq!(epoch(db), start + 4);

    let mut out = ptr::null_mut();
    assert_eq!(adb_list_schemas_json(db, &mut out), AdbStatus::Ok);
    let catalog = take_json(out);
    assert_eq!(catalog["schema_epoch"], start + 4);
    assert_eq!(catalog["entities"].as_array().unwrap().len(), 1);
    assert_eq!(catalog["entities"][0]["version"], 4);

    let mut out = ptr::null_mut();
    assert_eq!(adb_schema_history_json(db, 3, &mut out), AdbStatus::Ok);
    let history = take_json(out);
    let versions: Vec<u64> = history["versions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|schema| schema["version"].as_u64().unwrap())
        .collect();
    assert_eq!(versions, [1, 2, 3, 4]);

    assert_eq!(adb_drop_entity(db, 3), AdbStatus::Ok);
    assert_eq!(epoch(db), start + 5);
    let mut out = ptr::null_mut();
    assert_eq!(adb_list_schemas_json(db, &mut out), AdbStatus::Ok);
    assert!(take_json(out)["entities"].as_array().unwrap().is_empty());
    let mut out = ptr::null_mut();
    assert_eq!(
        adb_schema_history_json(db, 3, &mut out),
        AdbStatus::Ok,
        "the history of a dropped entity stays readable"
    );
    assert_eq!(take_json(out)["versions"].as_array().unwrap().len(), 4);
    assert_eq!(adb_close(db), AdbStatus::Ok);
}

/// Writes through the row functions are validated against the schema and defaults are filled.
#[test]
fn row_functions_validate_against_the_schema() {
    let (_dir, db) = open();
    assert_eq!(create(db, ACCOUNT).0, AdbStatus::Ok);
    assert_eq!(insert(db, 1, r#"{"1":{"Int64":1}}"#), AdbStatus::Ok);
    assert_eq!(
        insert(db, 2, r#"{"1":{"Int64":2},"2":{"String":"x"}}"#),
        AdbStatus::InvalidArgument,
        "a mistyped value is refused"
    );
    assert!(last_error().contains("account"), "{}", last_error());
    assert_eq!(
        insert(db, 3, r#"{"1":{"Int64":3},"9":{"Int64":1}}"#),
        AdbStatus::InvalidArgument,
        "an unknown field is refused"
    );
    assert_eq!(
        insert(db, 4, r#"{"1":{"Int64":5}}"#),
        AdbStatus::InvalidArgument,
        "the primary-key field must match the row key"
    );
    assert_eq!(adb_close(db), AdbStatus::Ok);
}

/// Refusals map onto stable statuses and leave the catalog unchanged.
#[test]
fn refusals_map_onto_statuses() {
    let (_dir, db) = open();
    assert_eq!(create(db, ACCOUNT).0, AdbStatus::Ok);
    let before = epoch(db);

    assert_eq!(create(db, "{").0, AdbStatus::InvalidArgument);
    assert!(
        last_error().contains("invalid entity spec"),
        "{}",
        last_error()
    );
    assert_eq!(
        create(db, &ACCOUNT.replace("\"entity_id\":3,", "")).0,
        AdbStatus::InvalidArgument,
        "the name is taken"
    );
    assert_eq!(
        alter(db, r#"{"entity_id":3,"change":{"op":"truncate"}}"#).0,
        AdbStatus::InvalidArgument
    );
    assert_eq!(
        alter(
            db,
            r#"{"entity_id":3,"change":{"op":"drop_field","name":"id"}}"#
        )
        .0,
        AdbStatus::InvalidArgument,
        "the primary key cannot be dropped"
    );
    assert_eq!(
        alter(
            db,
            r#"{"entity_id":3,"change":{"op":"add_field","field":
            {"name":"required","value_type":"Int64","nullable":false}}}"#
        )
        .0,
        AdbStatus::InvalidArgument,
        "a NOT NULL field needs a default"
    );
    assert_eq!(
        alter(
            db,
            r#"{"entity_id":9,"change":{"op":"drop_field","name":"x"}}"#
        )
        .0,
        AdbStatus::NotFound
    );
    assert_eq!(adb_drop_entity(db, 9), AdbStatus::NotFound);
    let mut out = ptr::null_mut();
    assert_eq!(
        adb_schema_history_json(db, 9, &mut out),
        AdbStatus::NotFound
    );
    assert!(out.is_null());
    assert_eq!(epoch(db), before, "refused DDL commits nothing");

    let too_many: Vec<String> = (0..5000)
        .map(|i| format!(r#"{{"name":"f{i}","value_type":"Int64","nullable":true}}"#))
        .collect();
    let wide = format!(
        r#"{{"name":"wide","primary_key":"f0","fields":[{}]}}"#,
        too_many.join(",")
    );
    assert_eq!(create(db, &wide).0, AdbStatus::ResourceLimit);
    assert_eq!(adb_close(db), AdbStatus::Ok);
}

/// Null handles and pointers are refused without touching the output slot's contents.
#[test]
fn invalid_arguments_are_refused() {
    let mut out = ptr::null_mut();
    assert_eq!(
        adb_create_entity_json(ptr::null_mut(), ACCOUNT.as_ptr(), ACCOUNT.len(), &mut out),
        AdbStatus::InvalidArgument
    );
    assert!(out.is_null());
    let (_dir, db) = open();
    assert_eq!(
        adb_create_entity_json(db, ptr::null(), 4, &mut out),
        AdbStatus::InvalidArgument
    );
    assert_eq!(
        adb_list_schemas_json(db, ptr::null_mut()),
        AdbStatus::InvalidArgument
    );
    assert_eq!(
        adb_drop_entity(ptr::null_mut(), 3),
        AdbStatus::InvalidArgument
    );
    assert_eq!(adb_close(db), AdbStatus::Ok);
}

/// The catalog survives a reopen (it is a projection of the log).
#[test]
fn schemas_survive_reopen() {
    let (dir, db) = open();
    assert_eq!(create(db, ACCOUNT).0, AdbStatus::Ok);
    assert_eq!(
        alter(
            db,
            r#"{"entity_id":3,"change":{"op":"rename_field","from":"balance","to":"b"}}"#
        )
        .0,
        AdbStatus::Ok
    );
    let before = epoch(db);
    assert_eq!(adb_close(db), AdbStatus::Ok);

    let path = dir.path().to_string_lossy().as_bytes().to_vec();
    let mut db = ptr::null_mut();
    assert_eq!(adb_open(path.as_ptr(), path.len(), &mut db), AdbStatus::Ok);
    assert_eq!(epoch(db), before);
    let mut out = ptr::null_mut();
    assert_eq!(adb_list_schemas_json(db, &mut out), AdbStatus::Ok);
    assert_eq!(active_names(&take_json(out)["entities"][0]), ["id", "b"]);
    assert_eq!(adb_close(db), AdbStatus::Ok);
}

/// The change feed carries DDL as `schema` and `drop_table` entries and every row change names
/// its schema version.
#[test]
fn change_feed_carries_schema_events() {
    let (_dir, db) = open();
    assert_eq!(create(db, ACCOUNT).0, AdbStatus::Ok);
    assert_eq!(insert(db, 1, r#"{"1":{"Int64":1}}"#), AdbStatus::Ok);
    assert_eq!(adb_drop_entity(db, 3), AdbStatus::Ok);
    let mut out = ptr::null_mut();
    assert_eq!(
        adb_read_changes_json(db, 0, 100, ptr::null(), 0, &mut out),
        AdbStatus::Ok
    );
    let feed = take_json(out);
    let changes: Vec<&Value> = feed["events"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|event| event["changes"].as_array().unwrap())
        .collect();
    assert_eq!(changes.len(), 3, "{feed}");
    assert_eq!(changes[0]["kind"], "schema");
    assert_eq!(changes[0]["entity_id"], 3);
    assert_eq!(changes[0]["schema"]["version"], 1);
    assert_eq!(changes[1]["kind"], "insert");
    assert_eq!(changes[1]["schema_version"], 1);
    assert_eq!(
        changes[1]["after"]["fields"]["2"],
        serde_json::json!({"Int64":0}),
        "the stored row holds the filled-in default"
    );
    assert_eq!(changes[2]["kind"], "drop_table");
    assert_eq!(changes[2]["epoch"], 2);
    assert_eq!(adb_close(db), AdbStatus::Ok);
}
