//! The error type shared by the engine and the CLI.

use std::fmt;

/// An error whose message is shown to the user as is. Every message names
/// the problem and the next step to take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Bad command-line usage. The CLI adds a pointer to `--help`.
    Usage(String),
    /// A problem the user can fix: bad config, missing folder, failing git.
    User(String),
    /// A bug in firescale or in a rule.
    Internal(String),
}

impl Error {
    pub fn usage(message: impl Into<String>) -> Self {
        Self::Usage(message.into())
    }

    pub fn user(message: impl Into<String>) -> Self {
        Self::User(message.into())
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }

    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::Usage(message) | Self::User(message) | Self::Internal(message) => message,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
