use thiserror::Error;

pub type MyError = anyhow::Error;

#[derive(Error, Debug)]
pub enum BotError {
    #[error("User not found")]
    UserNotFound,

    #[error("Module '{0}' not found")]
    ModuleNotFound(String),

    #[error("Query is too old")]
    QueryIsTooOld,

    #[error("Application Error: {0}")]
    Other(String),
}
