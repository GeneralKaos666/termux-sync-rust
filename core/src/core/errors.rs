//! Typed errors for the ported core.
//!
//! The Python source classed failures by exception name (`OSError`,
//! `UserError`, ...). Keep that name in the variant so the front door can pick
//! a title and hint without parsing a string, while `Display` preserves the
//! exact wire text the CLI, logs and tests already rely on.

use std::fmt::Display;

/// A failure raised by the core crate.
///
/// `Display` keeps the historical `"<Kind>: <message>"` shape, including the
/// `termux_sync.` module prefix on [`CoreError::User`]. Build values with the
/// helpers instead of writing the prefix by hand.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoreError {
    /// `termux_sync.UserError`: a problem the user can act on.
    #[error("termux_sync.UserError: {0}")]
    User(String),
    /// Bare `UserError` used when the message is already localised.
    #[error("UserError: {0}")]
    UserShort(String),
    /// `OSError`: a filesystem or process failure.
    #[error("OSError: {0}")]
    Os(String),
    /// `RuntimeError`: an operation failed while running.
    #[error("RuntimeError: {0}")]
    Runtime(String),
    /// `ValueError`: invalid input or configuration.
    #[error("ValueError: {0}")]
    Value(String),
    /// `FileNotFoundError`: a requested path or backup is missing.
    #[error("FileNotFoundError: {0}")]
    FileNotFound(String),
    /// `PermissionError`: the process may not touch the path.
    #[error("PermissionError: {0}")]
    Permission(String),
    /// `AttributeError`: a JSON document had the wrong shape.
    #[error("AttributeError: {0}")]
    Attribute(String),
    /// `SameFileError`: source and destination are the same inode.
    #[error("SameFileError: {0}")]
    SameFile(String),
    /// `ReadError`: an archive or stream could not be read.
    #[error("ReadError: {0}")]
    Read(String),
    /// `JSONDecodeError`: a document is not valid JSON.
    #[error("JSONDecodeError: {0}")]
    JsonDecode(String),
    /// Internal error with no Python class. `Display` prints the message alone.
    #[error("{0}")]
    Plain(String),
}

impl CoreError {
    pub fn user(message: impl Into<String>) -> Self {
        Self::User(message.into())
    }

    pub fn user_short(message: impl Into<String>) -> Self {
        Self::UserShort(message.into())
    }

    pub fn os(error: impl Display) -> Self {
        Self::Os(error.to_string())
    }

    pub fn runtime(message: impl Display) -> Self {
        Self::Runtime(message.to_string())
    }

    pub fn value(message: impl Display) -> Self {
        Self::Value(message.to_string())
    }

    pub fn file_not_found(message: impl Into<String>) -> Self {
        Self::FileNotFound(message.into())
    }

    pub fn permission(message: impl Into<String>) -> Self {
        Self::Permission(message.into())
    }

    pub fn attribute(message: impl Into<String>) -> Self {
        Self::Attribute(message.into())
    }

    pub fn same_file(message: impl Into<String>) -> Self {
        Self::SameFile(message.into())
    }

    pub fn read(message: impl Display) -> Self {
        Self::Read(message.to_string())
    }

    pub fn json_decode(message: impl Display) -> Self {
        Self::JsonDecode(message.to_string())
    }

    pub fn plain(message: impl Into<String>) -> Self {
        Self::Plain(message.into())
    }

    /// Substring search in the rendered message. Test-only convenience.
    #[cfg(test)]
    pub fn contains(&self, pattern: &str) -> bool {
        self.to_string().contains(pattern)
    }

    /// Rebuild an error from a Python exception name and message.
    pub fn from_kind(kind: &str, message: impl Into<String>) -> Self {
        let message = message.into();
        match kind {
            "UserError" => Self::UserShort(message),
            "OSError" => Self::Os(message),
            "RuntimeError" => Self::Runtime(message),
            "ValueError" => Self::Value(message),
            "FileNotFoundError" => Self::FileNotFound(message),
            "PermissionError" => Self::Permission(message),
            "AttributeError" => Self::Attribute(message),
            "SameFileError" => Self::SameFile(message),
            "ReadError" => Self::Read(message),
            "JSONDecodeError" => Self::JsonDecode(message),
            _ => Self::Plain(message),
        }
    }

    /// Python exception name, for log lines and legacy callers.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::User(_) | Self::UserShort(_) => "UserError",
            Self::Os(_) => "OSError",
            Self::Runtime(_) => "RuntimeError",
            Self::Value(_) => "ValueError",
            Self::FileNotFound(_) => "FileNotFoundError",
            Self::Permission(_) => "PermissionError",
            Self::Attribute(_) => "AttributeError",
            Self::SameFile(_) => "SameFileError",
            Self::Read(_) => "ReadError",
            Self::JsonDecode(_) => "JSONDecodeError",
            Self::Plain(_) => "Exception",
        }
    }

    /// Message without the `"<Kind>: "` prefix.
    pub fn message(&self) -> &str {
        match self {
            Self::User(message)
            | Self::UserShort(message)
            | Self::Os(message)
            | Self::Runtime(message)
            | Self::Value(message)
            | Self::FileNotFound(message)
            | Self::Permission(message)
            | Self::Attribute(message)
            | Self::SameFile(message)
            | Self::Read(message)
            | Self::JsonDecode(message)
            | Self::Plain(message) => message,
        }
    }
}

/// Title, message and hint for a terminal error card.
pub fn explain(error: &CoreError) -> (String, String, String) {
    let message = error.message().to_string();
    match error {
        CoreError::User(_) | CoreError::UserShort(_) => {
            ("Cannot continue".to_string(), message, String::new())
        }
        CoreError::Os(_) if message.contains("No space left") => (
            "Device is out of space".to_string(),
            "There is not enough free storage to finish this.".to_string(),
            "Free some space with termux-sync clear-cache or check usage with termux-sync check."
                .to_string(),
        ),
        CoreError::Permission(_) => (
            "Permission denied".to_string(),
            message,
            "Pick a path inside your home directory, or run termux-setup-storage to grant access to shared storage.".to_string(),
        ),
        CoreError::FileNotFound(_) => (
            "File or folder not found".to_string(),
            message,
            "Check that the path exists and is spelled correctly.".to_string(),
        ),
        CoreError::Runtime(_)
        | CoreError::Value(_)
        | CoreError::Os(_)
        | CoreError::Attribute(_)
        | CoreError::SameFile(_)
        | CoreError::Read(_)
        | CoreError::JsonDecode(_)
        | CoreError::Plain(_) => (
            "Operation failed".to_string(),
            message,
            "See the message above. Details are saved in the log.".to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_keeps_the_python_prefix() {
        assert_eq!(
            CoreError::user("bad name").to_string(),
            "termux_sync.UserError: bad name"
        );
        assert_eq!(CoreError::os("denied").to_string(), "OSError: denied");
        assert_eq!(CoreError::plain("boom").to_string(), "boom");
    }

    #[test]
    fn kind_matches_the_exception_name() {
        assert_eq!(CoreError::user("x").kind(), "UserError");
        assert_eq!(CoreError::os("x").kind(), "OSError");
        assert_eq!(CoreError::file_not_found("x").kind(), "FileNotFoundError");
        assert_eq!(CoreError::json_decode("x").kind(), "JSONDecodeError");
        assert_eq!(CoreError::plain("x").kind(), "Exception");
    }

    #[test]
    fn explain_uses_the_variant_not_a_prefix() {
        let (title, message, hint) = explain(&CoreError::file_not_found("gone"));
        assert_eq!(title, "File or folder not found");
        assert_eq!(message, "gone");
        assert!(hint.contains("Check that the path exists"));
    }

    #[test]
    fn out_of_space_os_errors_get_their_own_title() {
        let (title, _, hint) = explain(&CoreError::os("No space left on device"));
        assert_eq!(title, "Device is out of space");
        assert!(hint.contains("clear-cache"));
    }
}
