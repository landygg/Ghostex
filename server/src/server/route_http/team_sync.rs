//! Team sync routes (crate::team_sync): a workspace's connection to its team's Convex project
//! (join, connect, leave, invite, identity, status, ping, the team's and this member's Linear
//! keys) and the reads the Work page uses
//! (`listTicketSlackThreads`). Every one is full-local: they hold or use the member token.

use serde_json::{Map, Value};

use crate::domain::DomainStateError;
use crate::paths::GxserverPaths;
use crate::team_sync;

use super::*;

type TeamSyncOperation = fn(&GxserverPaths, &Map<String, Value>) -> Result<Value, String>;

fn team_sync_operation(path: &str) -> Option<TeamSyncOperation> {
    Some(match path {
        "/api/joinTeamSync" => team_sync::join_team,
        "/api/connectTeamSync" => team_sync::connect_team,
        "/api/leaveTeamSync" => team_sync::leave_team,
        "/api/readTeamSyncStatus" => team_sync::team_status,
        "/api/createTeamSyncInvite" => team_sync::create_invite,
        "/api/setTeamSyncIdentity" => team_sync::set_identity,
        "/api/pingTeamSync" => team_sync::ping_team,
        "/api/listTeamSyncCommands" => team_sync::recent_commands,
        "/api/listTicketSlackThreads" => team_sync::list_ticket_slack_threads,
        "/api/setTeamLinearKey" => team_sync::set_team_linear_key,
        "/api/setOwnLinearKey" => team_sync::set_own_linear_key,
        _ => return None,
    })
}

pub(super) async fn route_team_sync_http(
    request: RouteHttpRequest,
) -> Result<RoutedResponse, RouteHttpRequest> {
    let Some(operation) = team_sync_operation(request.endpoint.path.as_str()) else {
        return Err(request);
    };
    let RouteHttpRequest {
        state,
        endpoint,
        request_id,
        body_json,
        ..
    } = request;
    // Off the async runtime: every operation is a network call to the team's Convex project.
    let worker_state = state.clone();
    let worker_endpoint = endpoint.path.clone();
    let worker_request_id = request_id.clone();
    let response = tokio::task::spawn_blocking(move || {
        handle_domain_http(
            &worker_state,
            worker_endpoint,
            worker_request_id,
            &body_json,
            |_, _, params, _| {
                operation(&worker_state.paths, params).map_err(DomainStateError::bad_request)
            },
        )
    })
    .await;
    Ok(match response {
        Ok(response) => response,
        Err(error) => domain_error_response(
            endpoint.path,
            request_id,
            DomainStateError::corrupt_state(format!("The team sync request failed: {error}")),
        ),
    })
}
