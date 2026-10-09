//! The Add Project dialog's data: the shapes of packages/core-ui/add-project-modal/types.ts (deleted 2026-10-01) and
//! the result readers of apps/desktop/views/modal-host/add-project-requests.ts (deleted 2026-10-01) (`readAddProject*`), which turn a
//! gxserver answer into those shapes and refuse anything unexpected with the same messages.
use serde_json::Value;

/// A clone source row: a plain Git URL or one of the four providers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AddProjectSourceId {
    Url,
    Github,
    Gitlab,
    Bitbucket,
    AzureDevops,
}

impl AddProjectSourceId {
    pub(crate) const PROVIDERS: [Self; 4] = [
        Self::Github,
        Self::Gitlab,
        Self::Bitbucket,
        Self::AzureDevops,
    ];

    pub(crate) fn wire(self) -> &'static str {
        match self {
            Self::Url => "url",
            Self::Github => "github",
            Self::Gitlab => "gitlab",
            Self::Bitbucket => "bitbucket",
            Self::AzureDevops => "azure-devops",
        }
    }

    pub(crate) fn from_wire(value: &str) -> Option<Self> {
        match value {
            "url" => Some(Self::Url),
            "github" => Some(Self::Github),
            "gitlab" => Some(Self::Gitlab),
            "bitbucket" => Some(Self::Bitbucket),
            "azure-devops" => Some(Self::AzureDevops),
            _ => None,
        }
    }

    pub(crate) fn index(self) -> usize {
        match self {
            Self::Url => 0,
            Self::Github => 1,
            Self::Gitlab => 2,
            Self::Bitbucket => 3,
            Self::AzureDevops => 4,
        }
    }
}

/// A machine the dialog can add a project on (`AddProjectMachineOption`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AddProjectMachineOption {
    pub(crate) add_project_base_directory: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) label: String,
    pub(crate) machine_id: String,
    pub(crate) platform: Option<String>,
    pub(crate) starts_at_drive_list: bool,
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn required_text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .map(str::to_string)
}

/// The machine list the app builds (`gpui_add_project_dialog_machine_options`): entries without
/// a label or a machine id are dropped.
pub(crate) fn parse_add_project_machine_options(value: &Value) -> Vec<AddProjectMachineOption> {
    value
        .as_array()
        .map(|machines| {
            machines
                .iter()
                .filter_map(|machine| {
                    Some(AddProjectMachineOption {
                        add_project_base_directory: text(machine, "addProjectBaseDirectory"),
                        description: text(machine, "description"),
                        label: required_text(machine, "label")?,
                        machine_id: required_text(machine, "machineId")?,
                        platform: text(machine, "platform"),
                        starts_at_drive_list: machine
                            .get("startsAtDriveList")
                            .and_then(Value::as_bool)
                            == Some(true),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AddProjectBrowseEntry {
    pub(crate) full_path: String,
    pub(crate) name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AddProjectInspectionKind {
    Directory,
    File,
    Missing,
}

/// What gxserver found at the full typed path (`AddProjectPathInspection`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AddProjectPathInspection {
    pub(crate) kind: Option<AddProjectInspectionKind>,
    pub(crate) project_path: Option<String>,
    pub(crate) project_id: Option<String>,
    pub(crate) git_root: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AddProjectBrowseResult {
    pub(crate) entries: Vec<AddProjectBrowseEntry>,
    pub(crate) is_drive_list: bool,
    pub(crate) parent_path: String,
    pub(crate) inspection: Option<AddProjectPathInspection>,
}

pub(crate) const UNEXPECTED_ANSWER: &str = "The machine returned an unexpected answer.";

fn result_object<'a>(value: &'a Value, key: &str) -> Result<&'a Value, String> {
    value
        .get(key)
        .filter(|entry| entry.is_object())
        .ok_or_else(|| UNEXPECTED_ANSWER.to_string())
}

fn required(value: &Value, key: &str) -> Result<String, String> {
    required_text(value, key).ok_or_else(|| UNEXPECTED_ANSWER.to_string())
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.is_empty())
}

/// `readAddProjectBrowseResult`.
pub(crate) fn read_add_project_browse_result(
    value: &Value,
) -> Result<AddProjectBrowseResult, String> {
    let parent_path = value
        .get("parentPath")
        .and_then(Value::as_str)
        .ok_or_else(|| UNEXPECTED_ANSWER.to_string())?;
    let entries = value
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| UNEXPECTED_ANSWER.to_string())?
        .iter()
        .map(|entry| {
            Some(AddProjectBrowseEntry {
                full_path: entry.get("fullPath")?.as_str()?.to_string(),
                name: entry.get("name")?.as_str()?.to_string(),
            })
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| UNEXPECTED_ANSWER.to_string())?;
    let inspection = value
        .get("inspection")
        .filter(|inspection| inspection.is_object())
        .map(|inspection| AddProjectPathInspection {
            kind: match inspection.get("kind").and_then(Value::as_str) {
                Some("directory") => Some(AddProjectInspectionKind::Directory),
                Some("file") => Some(AddProjectInspectionKind::File),
                Some("missing") => Some(AddProjectInspectionKind::Missing),
                _ => None,
            },
            project_path: non_empty(text(inspection, "projectPath")),
            project_id: non_empty(text(inspection, "projectId")),
            git_root: non_empty(text(inspection, "gitRoot")),
        });
    Ok(AddProjectBrowseResult {
        entries,
        is_drive_list: value.get("isDriveList").and_then(Value::as_bool) == Some(true),
        parent_path: parent_path.to_string(),
        inspection,
    })
}

/// `readAddProjectCreateDirectoryResult`: the server-normalized path of the new folder.
pub(crate) fn read_add_project_created_directory(value: &Value) -> Result<String, String> {
    required(value, "path")
}

/// `readAddProjectAddResult`: only the answer's shape is checked; the app activates the project.
pub(crate) fn read_add_project_add_result(value: &Value) -> Result<(), String> {
    result_object(value, "project").map(|_| ())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AddProjectProviderAuthStatus {
    Authenticated,
    Unauthenticated,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AddProjectProviderDiscovery {
    pub(crate) auth: Option<(AddProjectProviderAuthStatus, Option<String>)>,
    pub(crate) install_hint: Option<String>,
    /// The `/api/managedTools` tool that installs this provider's missing CLI (`gh`, `glab`).
    pub(crate) install_tool: Option<String>,
    pub(crate) provider: Option<AddProjectSourceId>,
    pub(crate) available: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AddProjectSourceControlDiscovery {
    pub(crate) providers: Vec<AddProjectProviderDiscovery>,
}

/// `readAddProjectDiscovery`.
pub(crate) fn read_add_project_discovery(
    value: &Value,
) -> Result<AddProjectSourceControlDiscovery, String> {
    let discovery = result_object(value, "discovery")?;
    let providers = discovery
        .get("providers")
        .and_then(Value::as_array)
        .ok_or_else(|| UNEXPECTED_ANSWER.to_string())?;
    Ok(AddProjectSourceControlDiscovery {
        providers: providers
            .iter()
            .map(|provider| AddProjectProviderDiscovery {
                auth: provider
                    .get("auth")
                    .filter(|auth| auth.is_object())
                    .map(|auth| {
                        let status = match auth.get("status").and_then(Value::as_str) {
                            Some("authenticated") => AddProjectProviderAuthStatus::Authenticated,
                            Some("unauthenticated") => {
                                AddProjectProviderAuthStatus::Unauthenticated
                            }
                            _ => AddProjectProviderAuthStatus::Unknown,
                        };
                        (status, text(auth, "detail"))
                    }),
                install_hint: text(provider, "installHint"),
                install_tool: text(provider, "installTool")
                    .filter(|tool| matches!(tool.as_str(), "gh" | "glab")),
                provider: provider
                    .get("provider")
                    .and_then(Value::as_str)
                    .and_then(AddProjectSourceId::from_wire),
                available: provider.get("status").and_then(Value::as_str) == Some("available"),
            })
            .collect(),
    })
}

/// `AddProjectRepositoryInfo`, the provider lookup's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AddProjectRepositoryInfo {
    pub(crate) name_with_owner: String,
    pub(crate) url: String,
}

/// `readAddProjectRepositoryInfo`.
pub(crate) fn read_add_project_repository(
    value: &Value,
) -> Result<AddProjectRepositoryInfo, String> {
    let repository = result_object(value, "repository")?;
    let name_with_owner = required(repository, "nameWithOwner")?;
    required(repository, "provider")?;
    required(repository, "sshUrl")?;
    Ok(AddProjectRepositoryInfo {
        name_with_owner,
        url: required(repository, "url")?,
    })
}

/// `AddProjectClonePreview`: what the review step shows about the destination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AddProjectClonePreview {
    pub(crate) destination_blocked: bool,
    pub(crate) destination_exists: bool,
    pub(crate) destination_is_empty: Option<bool>,
    pub(crate) destination_path: String,
    pub(crate) warning: Option<String>,
}

/// `readAddProjectClonePreview`.
pub(crate) fn read_add_project_clone_preview(
    value: &Value,
) -> Result<AddProjectClonePreview, String> {
    let preview = result_object(value, "preview")?;
    if let Some(kind) = preview.get("destinationExistsKind") {
        if !matches!(kind.as_str(), Some("directory" | "file" | "other")) {
            return Err("The machine returned an unexpected clone destination.".to_string());
        }
    }
    required(preview, "cloneUrl")?;
    required(preview, "destinationFolderName")?;
    let destination_path = required(preview, "destinationPath")?;
    required(preview, "parentPath")?;
    required(preview, "repositoryName")?;
    let flag = |key: &str| preview.get(key).and_then(Value::as_bool) == Some(true);
    Ok(AddProjectClonePreview {
        destination_blocked: flag("destinationBlocked"),
        destination_exists: flag("destinationExists"),
        destination_is_empty: preview.get("destinationIsEmpty").and_then(Value::as_bool),
        destination_path,
        warning: text(preview, "warning"),
    })
}

/// `readAddProjectCloneHandle`: the started job's id.
pub(crate) fn read_add_project_clone_handle(value: &Value) -> Result<String, String> {
    required(result_object(value, "job")?, "jobId")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AddProjectCloneJobState {
    Canceled,
    Completed,
    Failed,
    Running,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AddProjectCloneJob {
    pub(crate) error: Option<String>,
    pub(crate) message: Option<String>,
    /// git's latest progress line while the job runs.
    pub(crate) progress: Option<String>,
    pub(crate) project_path: Option<String>,
    pub(crate) state: AddProjectCloneJobState,
}

/// `readAddProjectCloneJob`.
pub(crate) fn read_add_project_clone_job(value: &Value) -> Result<AddProjectCloneJob, String> {
    let job = result_object(value, "job")?;
    let state = match required(job, "state")?.as_str() {
        "canceled" => AddProjectCloneJobState::Canceled,
        "completed" => AddProjectCloneJobState::Completed,
        "failed" => AddProjectCloneJobState::Failed,
        "running" => AddProjectCloneJobState::Running,
        _ => return Err("The machine returned an unexpected clone state.".to_string()),
    };
    required(job, "jobId")?;
    Ok(AddProjectCloneJob {
        error: text(job, "error"),
        message: text(job, "message"),
        progress: text(job, "progress"),
        project_path: text(job, "projectPath"),
        state,
    })
}

/// `describeError`: the host's own text, or the step's fallback when it is blank.
pub(crate) fn describe_add_project_error(error: &str, fallback: &str) -> String {
    let trimmed = error.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}
