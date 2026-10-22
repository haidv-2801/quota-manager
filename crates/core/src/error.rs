#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("vault error: {0}")]
    Vault(String),
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("account not found")]
    NotFound,
}

pub type Result<T> = std::result::Result<T, Error>;
