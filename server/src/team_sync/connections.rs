//! Each workspace's connection to its team's Convex project: the deployment URL and this member's
//! token, kept under `teamSync` in the private work-mode credentials file.
//!
//! CDXC:TeamSync 2026-10-09 WHY:
//! The member token is the only thing that proves who this Ghostex is to the team's Convex project,
//! so it lives next to the Linear keys in the private credentials file (0600 on macOS and Linux),
//! is never published, and no endpoint returns it.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::paths::GxserverPaths;
use crate::work_mode::{read_credentials, write_credentials};

const CREDENTIALS_KEY: &str = "teamSync";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TeamConnection {
    #[serde(skip)]
    pub(crate) workspace_id: String,
    /// `https://<name>.convex.cloud`, without a trailing slash.
    pub(crate) deployment_url: String,
    pub(crate) member_token: String,
    #[serde(default)]
    pub(crate) team_name: Option<String>,
    #[serde(default)]
    pub(crate) member_id: Option<String>,
    #[serde(default)]
    pub(crate) member_name: Option<String>,
    #[serde(default)]
    pub(crate) connected_at: Option<String>,
    /// "Create my Slack tickets with my own Linear key": this workspace's Linear key is kept in the
    /// team's Convex project for this member (crate::team_sync::linear_keys).
    #[serde(default)]
    pub(crate) own_linear_key: bool,
}

impl TeamConnection {
    /// What endpoints and the CLI may show: everything but the token.
    pub(crate) fn summary(&self) -> Value {
        serde_json::json!({
            "workspaceId": self.workspace_id,
            "deploymentUrl": self.deployment_url,
            "siteUrl": super::invite_link::site_url(&self.deployment_url),
            "teamName": self.team_name,
            "memberId": self.member_id,
            "memberName": self.member_name,
            "connectedAt": self.connected_at,
            "ownLinearKey": self.own_linear_key,
        })
    }
}

fn connections_map(credentials: &Map<String, Value>) -> Map<String, Value> {
    credentials
        .get(CREDENTIALS_KEY)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// Every workspace's connection, ordered by workspace id.
pub(crate) fn read_team_connections(paths: &GxserverPaths) -> Vec<TeamConnection> {
    let mut connections: Vec<TeamConnection> = connections_map(&read_credentials(paths))
        .into_iter()
        .filter_map(|(workspace_id, value)| {
            let mut connection = serde_json::from_value::<TeamConnection>(value).ok()?;
            if connection.deployment_url.trim().is_empty()
                || connection.member_token.trim().is_empty()
            {
                return None;
            }
            connection.workspace_id = workspace_id;
            Some(connection)
        })
        .collect();
    connections.sort_by(|left, right| left.workspace_id.cmp(&right.workspace_id));
    connections
}

pub(crate) fn read_team_connection(
    paths: &GxserverPaths,
    workspace_id: &str,
) -> Option<TeamConnection> {
    read_team_connections(paths)
        .into_iter()
        .find(|connection| connection.workspace_id == workspace_id)
}

/// Saves (replacing) one workspace's connection.
pub(crate) fn store_team_connection(
    paths: &GxserverPaths,
    connection: &TeamConnection,
) -> std::io::Result<()> {
    let mut credentials = read_credentials(paths);
    let mut connections = connections_map(&credentials);
    connections.insert(
        connection.workspace_id.clone(),
        serde_json::to_value(connection).unwrap_or(Value::Null),
    );
    credentials.insert(CREDENTIALS_KEY.to_string(), Value::Object(connections));
    write_credentials(paths, &credentials)
}

/// Forgets one workspace's connection. Returns whether there was one.
pub(crate) fn remove_team_connection(
    paths: &GxserverPaths,
    workspace_id: &str,
) -> std::io::Result<bool> {
    let mut credentials = read_credentials(paths);
    let mut connections = connections_map(&credentials);
    let removed = connections.remove(workspace_id).is_some();
    if removed {
        credentials.insert(CREDENTIALS_KEY.to_string(), Value::Object(connections));
        write_credentials(paths, &credentials)?;
    }
    Ok(removed)
}
