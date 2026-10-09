use std::{collections::HashMap, fmt, sync::Arc};

use serde_json::Value;
use tokio::sync::Mutex;

use crate::{events::GxserverEventHub, logging::GxserverLogger, paths::GxserverPaths};

pub(super) const DEFAULT_REPOSITORY_HOST: &str = "github.com";
/// A clone is stopped after this long without new git progress output.
pub(super) const REPOSITORY_CLONE_TIMEOUT_MS: u64 = 10 * 60_000;
pub(super) const REPOSITORY_CLONE_OUTPUT_LIMIT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct RepositoryCloneError {
    pub code: &'static str,
    pub message: String,
}

impl RepositoryCloneError {
    pub(super) fn bad_request(message: impl Into<String>) -> Self {
        Self {
            code: "badRequest",
            message: message.into(),
        }
    }

    pub(super) fn dependency_unavailable(message: impl Into<String>) -> Self {
        Self {
            code: "dependencyUnavailable",
            message: message.into(),
        }
    }

    pub(super) fn not_found(message: impl Into<String>) -> Self {
        Self {
            code: "notFound",
            message: message.into(),
        }
    }
}

impl fmt::Display for RepositoryCloneError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for RepositoryCloneError {}

#[derive(Clone, Default)]
pub struct RepositoryCloneJobManager {
    pub(super) jobs: Arc<Mutex<HashMap<String, Value>>>,
}

#[derive(Clone)]
pub struct RepositoryCloneRuntime {
    pub event_hub: GxserverEventHub,
    pub logger: Arc<GxserverLogger>,
    pub paths: GxserverPaths,
    pub presentation_event_sequence: Arc<std::sync::Mutex<()>>,
    pub server_id: String,
}

#[derive(Debug)]
pub(super) struct ParsedRepositoryCloneInput {
    pub(super) clone_url: String,
    pub(super) repository_name: String,
}

#[derive(Debug)]
pub(super) struct CloneRunOutput {
    pub(super) exit_code: i32,
    pub(super) stderr: String,
    pub(super) stdout: String,
}
