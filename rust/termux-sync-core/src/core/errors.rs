//! Expected user-facing errors, mirroring `UserError` and `_explain`.

#[derive(Debug, Clone)]
pub struct UserError {
    pub message: String,
    pub hint: String,
    pub title: String,
}

impl UserError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hint: String::new(),
            title: "Cannot continue".to_string(),
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn to_error(&self) -> String {
        format!("termux_sync.UserError: {}", self.message)
    }
}

/// Best-effort equivalent of `_explain` for errors that reach Rust.
pub fn explain(kind: &str, message: &str) -> (String, String, String) {
    match kind {
        "UserError" => (message.to_string(), String::new(), "Cannot continue".to_string()),
        "EOFError" => (
            "Input was closed".to_string(),
            "termux-sync asked a question, but no answer could be read (the input stream ended, or you pressed Ctrl+D).".to_string(),
            "Run the command directly in the Termux terminal and answer the prompts.".to_string(),
        ),
        "FileNotFoundError" => (
            "File or folder not found".to_string(),
            message.to_string(),
            "Check that the path exists and is spelled correctly.".to_string(),
        ),
        "PermissionError" => (
            "Permission denied".to_string(),
            message.to_string(),
            "Pick a path inside your home directory, or run termux-setup-storage to grant access to shared storage.".to_string(),
        ),
        "OSError" if message.contains("No space left") => (
            "Device is out of space".to_string(),
            "There is not enough free storage to finish this.".to_string(),
            "Free some space with termux-sync clear-cache or check usage with termux-sync check.".to_string(),
        ),
        "RuntimeError" | "ValueError" => (
            "Operation failed".to_string(),
            message.to_string(),
            "See the message above. Details are saved in the log.".to_string(),
        ),
        other => (
            "Unexpected error".to_string(),
            format!("{other}: {message}"),
            "Run again with --debug for the full traceback.".to_string(),
        ),
    }
}
