use adb_core::Row;

#[derive(Debug, Clone)]
pub enum Mutation {
    Put(Row),
    Delete,
}
