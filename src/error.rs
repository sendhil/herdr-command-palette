use crate::client::RequestClass;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Stdout,
    Stderr,
}

impl std::fmt::Display for StreamKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stdout => formatter.write_str("stdout"),
            Self::Stderr => formatter.write_str("stderr"),
        }
    }
}

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("HERDR_BIN_PATH is missing or empty")]
    MissingBinPath,
    #[error("failed to start Herdr: {message}")]
    Spawn { message: String },
    #[error("failed while running Herdr: {message}")]
    Process { message: String },
    #[error("Herdr {class} request timed out")]
    Timeout { class: RequestClass },
    #[error("Herdr {stream} exceeded the {limit}-byte capture limit")]
    OutputLimit { stream: StreamKind, limit: usize },
    #[error("Herdr returned {code}: {message}")]
    Api { code: String, message: String },
    #[error("Herdr exited unsuccessfully: {message}")]
    Exit { message: String },
    #[error("Herdr returned invalid JSON for {context}: {message}")]
    InvalidJson {
        context: &'static str,
        message: String,
    },
    #[error("Herdr returned an unexpected response for {context}: {message}")]
    UnexpectedResponse {
        context: &'static str,
        message: String,
    },
}

impl ClientError {
    pub const NOT_GIT_WORKTREE_CODE: &str = "not_git_worktree";

    pub fn is_not_git_worktree(&self) -> bool {
        matches!(self, Self::Api { code, .. } if code == Self::NOT_GIT_WORKTREE_CODE)
    }

    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Api { code, .. } => Some(code),
            Self::MissingBinPath
            | Self::Spawn { .. }
            | Self::Process { .. }
            | Self::Timeout { .. }
            | Self::OutputLimit { .. }
            | Self::Exit { .. }
            | Self::InvalidJson { .. }
            | Self::UnexpectedResponse { .. } => None,
        }
    }

    pub fn request_class(&self) -> Option<RequestClass> {
        match self {
            Self::Timeout { class } => Some(*class),
            Self::MissingBinPath
            | Self::Spawn { .. }
            | Self::Process { .. }
            | Self::OutputLimit { .. }
            | Self::Api { .. }
            | Self::Exit { .. }
            | Self::InvalidJson { .. }
            | Self::UnexpectedResponse { .. } => None,
        }
    }

    pub fn stream_kind(&self) -> Option<StreamKind> {
        match self {
            Self::OutputLimit { stream, .. } => Some(*stream),
            Self::MissingBinPath
            | Self::Spawn { .. }
            | Self::Process { .. }
            | Self::Timeout { .. }
            | Self::Api { .. }
            | Self::Exit { .. }
            | Self::InvalidJson { .. }
            | Self::UnexpectedResponse { .. } => None,
        }
    }
}
