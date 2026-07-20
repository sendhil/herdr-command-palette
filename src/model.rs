use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiCapabilities {
    pub typed_agent_start: bool,
    pub agent_prompt: bool,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CapabilitySchemaError {
    #[error("API schema is missing the request schema")]
    MissingRequestSchema,
    #[error("API request schema has no request variants")]
    MissingRequestVariants,
    #[error("API schema contains an invalid reference: {0}")]
    InvalidReference(String),
}

pub fn capabilities_from_schema(schema: &Value) -> Result<ApiCapabilities, CapabilitySchemaError> {
    let request = schema
        .pointer("/schemas/request")
        .ok_or(CapabilitySchemaError::MissingRequestSchema)?;
    let request = resolve_schema(schema, request)?;
    let variants = request
        .get("oneOf")
        .or_else(|| request.get("anyOf"))
        .and_then(Value::as_array)
        .ok_or(CapabilitySchemaError::MissingRequestVariants)?;

    let mut capabilities = ApiCapabilities {
        typed_agent_start: false,
        agent_prompt: false,
    };
    for variant in variants {
        let variant = resolve_schema(schema, variant)?;
        match method_constant(variant) {
            Some("agent.prompt") => capabilities.agent_prompt = true,
            Some("agent.start") => {
                if let Some(params) = variant.pointer("/properties/params") {
                    capabilities.typed_agent_start |=
                        typed_agent_start_schema(resolve_schema(schema, params)?);
                }
            }
            _ => {}
        }
    }
    Ok(capabilities)
}

fn method_constant(variant: &Value) -> Option<&str> {
    variant
        .pointer("/properties/method/const")
        .and_then(Value::as_str)
}

fn typed_agent_start_schema(params: &Value) -> bool {
    let Some(properties) = params.get("properties").and_then(Value::as_object) else {
        return false;
    };
    let Some(required) = params.get("required").and_then(Value::as_array) else {
        return false;
    };
    let has_property = |name: &str| properties.contains_key(name);
    let is_required = |name: &str| required.iter().any(|item| item.as_str() == Some(name));

    ["name", "kind", "pane_id", "timeout_ms"]
        .into_iter()
        .all(has_property)
        && ["name", "kind", "pane_id"].into_iter().all(is_required)
}

fn resolve_schema<'a>(
    root: &'a Value,
    schema: &'a Value,
) -> Result<&'a Value, CapabilitySchemaError> {
    let mut current = schema;
    for _ in 0..32 {
        let Some(reference) = current.get("$ref").and_then(Value::as_str) else {
            return Ok(current);
        };
        let Some(pointer) = reference.strip_prefix('#') else {
            return Err(CapabilitySchemaError::InvalidReference(reference.into()));
        };
        current = root
            .pointer(pointer)
            .ok_or_else(|| CapabilitySchemaError::InvalidReference(reference.into()))?;
    }
    let reference = current
        .get("$ref")
        .and_then(Value::as_str)
        .unwrap_or("reference chain is too deep");
    Err(CapabilitySchemaError::InvalidReference(reference.into()))
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct PluginInvocationContext {
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub workspace_label: Option<String>,
    #[serde(default)]
    pub workspace_cwd: Option<String>,
    #[serde(default)]
    pub worktree: Option<WorkspaceWorktreeInfo>,
    #[serde(default)]
    pub tab_id: Option<String>,
    #[serde(default)]
    pub tab_label: Option<String>,
    #[serde(default)]
    pub focused_pane_id: Option<String>,
    #[serde(default)]
    pub focused_pane_cwd: Option<String>,
    #[serde(default)]
    pub focused_pane_agent: Option<String>,
    #[serde(default)]
    pub focused_pane_status: Option<AgentStatus>,
    #[serde(default)]
    pub selected_text: Option<String>,
    #[serde(default)]
    pub invocation_source: Option<String>,
    #[serde(default)]
    pub correlation_id: Option<String>,
    #[serde(default)]
    pub clicked_url: Option<String>,
    #[serde(default)]
    pub link_handler_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SuccessEnvelope<T> {
    pub id: String,
    pub result: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ErrorEnvelope {
    pub id: String,
    pub error: ErrorBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SessionSnapshotResult {
    #[serde(rename = "type")]
    pub response_type: String,
    pub snapshot: SessionSnapshot,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PluginListResult {
    #[serde(rename = "type")]
    pub response_type: String,
    pub plugins: Vec<InstalledPluginInfo>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PluginActionListResult {
    #[serde(rename = "type")]
    pub response_type: String,
    pub actions: Vec<PluginActionInfo>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WorktreeListResult {
    #[serde(rename = "type")]
    pub response_type: String,
    pub source: WorktreeSourceInfo,
    pub worktrees: Vec<WorktreeInfo>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SessionSnapshot {
    pub version: String,
    pub protocol: u32,
    #[serde(default)]
    pub focused_workspace_id: Option<String>,
    #[serde(default)]
    pub focused_tab_id: Option<String>,
    #[serde(default)]
    pub focused_pane_id: Option<String>,
    pub workspaces: Vec<WorkspaceInfo>,
    pub tabs: Vec<TabInfo>,
    pub panes: Vec<PaneInfo>,
    pub layouts: Vec<PaneLayoutSnapshot>,
    pub agents: Vec<AgentInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorkspaceInfo {
    pub workspace_id: String,
    pub number: usize,
    pub label: String,
    pub focused: bool,
    pub pane_count: usize,
    pub tab_count: usize,
    pub active_tab_id: String,
    pub agent_status: AgentStatus,
    #[serde(default)]
    pub worktree: Option<WorkspaceWorktreeInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorkspaceWorktreeInfo {
    pub repo_key: String,
    pub repo_name: String,
    pub repo_root: String,
    pub checkout_path: String,
    pub is_linked_worktree: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TabInfo {
    pub tab_id: String,
    pub workspace_id: String,
    pub number: usize,
    pub label: String,
    pub focused: bool,
    pub pane_count: usize,
    pub agent_status: AgentStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PaneInfo {
    pub pane_id: String,
    pub terminal_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub focused: bool,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub foreground_cwd: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub terminal_title: Option<String>,
    #[serde(default)]
    pub terminal_title_stripped: Option<String>,
    #[serde(default)]
    pub display_agent: Option<String>,
    pub agent_status: AgentStatus,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PaneLayoutSnapshot {
    pub workspace_id: String,
    pub tab_id: String,
    pub zoomed: bool,
    pub area: PaneLayoutRect,
    pub focused_pane_id: String,
    pub panes: Vec<PaneLayoutPane>,
    pub splits: Vec<PaneLayoutSplit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct PaneLayoutRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PaneLayoutPane {
    pub pane_id: String,
    pub focused: bool,
    pub rect: PaneLayoutRect,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PaneLayoutSplit {
    pub id: String,
    pub direction: SplitDirection,
    pub ratio: f32,
    pub rect: PaneLayoutRect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitDirection {
    Right,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentInfo {
    pub terminal_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub terminal_title: Option<String>,
    #[serde(default)]
    pub terminal_title_stripped: Option<String>,
    #[serde(default)]
    pub display_agent: Option<String>,
    pub agent_status: AgentStatus,
    pub workspace_id: String,
    pub tab_id: String,
    pub pane_id: String,
    pub focused: bool,
    #[serde(default)]
    pub launch_pending: bool,
    #[serde(default)]
    pub interactive_ready: bool,
    #[serde(default)]
    pub state_change_seq: u64,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub foreground_cwd: Option<String>,
    pub revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Idle,
    Working,
    Blocked,
    Done,
    Unknown,
}

impl AgentStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "IDLE",
            Self::Working => "WORKING",
            Self::Blocked => "BLOCKED",
            Self::Done => "DONE",
            Self::Unknown => "UNKNOWN",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct InstalledPluginInfo {
    pub plugin_id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub min_herdr_version: String,
    #[serde(default)]
    pub description: Option<String>,
    pub manifest_path: String,
    pub plugin_root: String,
    pub enabled: bool,
    #[serde(default)]
    pub platforms: Option<Vec<PluginPlatform>>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PluginActionInfo {
    pub plugin_id: String,
    pub action_id: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub contexts: Vec<PluginActionContext>,
    pub command: Vec<String>,
    #[serde(default)]
    pub platforms: Option<Vec<PluginPlatform>>,
}

impl PluginActionInfo {
    pub fn qualified_id(&self) -> String {
        format!("{}.{}", self.plugin_id, self.action_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginPlatform {
    Linux,
    Macos,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginActionContext {
    Global,
    Workspace,
    Tab,
    Pane,
    Selection,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorktreeSourceInfo {
    pub repo_key: String,
    pub repo_name: String,
    pub repo_root: String,
    pub source_checkout_path: String,
    #[serde(default)]
    pub source_workspace_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorktreeInfo {
    pub path: String,
    #[serde(default)]
    pub branch: Option<String>,
    pub is_bare: bool,
    pub is_detached: bool,
    pub is_prunable: bool,
    pub is_linked_worktree: bool,
    #[serde(default)]
    pub open_workspace_id: Option<String>,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeSnapshot {
    pub session: SessionSnapshot,
    pub worktrees: Vec<WorktreeInfo>,
    pub invocation: PluginInvocationContext,
    pub capabilities: ApiCapabilities,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> serde_json::Value {
        let raw = match name {
            "api-schema-0.7.4.json" => include_str!("../tests/fixtures/api-schema-0.7.4.json"),
            "api-schema-capable.json" => {
                include_str!("../tests/fixtures/api-schema-capable.json")
            }
            _ => panic!("unknown fixture {name}"),
        };
        serde_json::from_str(raw).unwrap()
    }

    #[test]
    fn capabilities_are_discovered_by_method_name_and_shape() {
        let old = fixture("api-schema-0.7.4.json");
        let new = fixture("api-schema-capable.json");
        let old_has_agent_start = old
            .pointer("/schemas/request/oneOf")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|variants| {
                variants.iter().any(|variant| {
                    variant
                        .pointer("/properties/method/const")
                        .and_then(serde_json::Value::as_str)
                        == Some("agent.start")
                })
            });
        assert!(
            old_has_agent_start,
            "the v0.7.4 fixture advertises agent.start"
        );

        assert_eq!(
            capabilities_from_schema(&old).unwrap(),
            ApiCapabilities {
                typed_agent_start: false,
                agent_prompt: false,
            }
        );
        assert_eq!(
            capabilities_from_schema(&new).unwrap(),
            ApiCapabilities {
                typed_agent_start: true,
                agent_prompt: true,
            }
        );
    }

    #[test]
    fn agent_status_has_compact_stable_text() {
        assert_eq!(AgentStatus::Idle.as_str(), "IDLE");
        assert_eq!(AgentStatus::Working.as_str(), "WORKING");
        assert_eq!(AgentStatus::Blocked.as_str(), "BLOCKED");
        assert_eq!(AgentStatus::Done.as_str(), "DONE");
        assert_eq!(AgentStatus::Unknown.as_str(), "UNKNOWN");
    }

    #[test]
    fn public_response_fixtures_deserialize_into_typed_models() {
        let session: SuccessEnvelope<SessionSnapshotResult> =
            serde_json::from_str(include_str!("../tests/fixtures/session-snapshot.json")).unwrap();
        assert_eq!(session.result.snapshot.workspaces.len(), 2);
        assert_eq!(session.result.snapshot.layouts[0].panes.len(), 4);

        let plugins: SuccessEnvelope<PluginListResult> =
            serde_json::from_str(include_str!("../tests/fixtures/plugin-list.json")).unwrap();
        assert!(plugins.result.plugins.iter().any(|plugin| !plugin.enabled));

        let actions: SuccessEnvelope<PluginActionListResult> =
            serde_json::from_str(include_str!("../tests/fixtures/plugin-actions.json")).unwrap();
        assert!(actions
            .result
            .actions
            .iter()
            .any(|action| { action.platforms.as_deref() == Some(&[PluginPlatform::Linux][..]) }));

        let worktrees: SuccessEnvelope<WorktreeListResult> =
            serde_json::from_str(include_str!("../tests/fixtures/worktree-list.json")).unwrap();
        assert!(worktrees
            .result
            .worktrees
            .iter()
            .any(|worktree| worktree.open_workspace_id.is_none()));
    }

    #[test]
    fn error_envelope_and_missing_optional_fields_deserialize() {
        let error: ErrorEnvelope = serde_json::from_str(
            r#"{"id":"request-1","error":{"code":"pane_not_found","message":"gone"}}"#,
        )
        .unwrap();
        assert_eq!(error.error.code, "pane_not_found");

        let context: PluginInvocationContext = serde_json::from_str("{}").unwrap();
        assert_eq!(context.selected_text, None);

        let context: PluginInvocationContext = serde_json::from_str(include_str!(
            "../tests/fixtures/plugin-invocation-context.json"
        ))
        .unwrap();
        assert_eq!(context.selected_text.as_deref(), Some("chosen"));
    }
}
