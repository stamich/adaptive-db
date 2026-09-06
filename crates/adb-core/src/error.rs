use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid data: {0}")]
    InvalidData(String),
}
