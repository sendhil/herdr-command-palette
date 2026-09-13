use std::path::PathBuf;

use thiserror::Error;

use crate::client::{HerdrClient, HerdrOperation, OperationResponse};
use crate::command::{ArgumentKey, CommandId, CoreCommand, SplitDirection};
use crate::error::ClientError;
use crate::model::RuntimeSnapshot;
use crate::registry::PaletteItemId;
use crate::state::{ArgumentValue, CompletedArguments, Confirmation};

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum ExecutionOutcome {
    Succeeded,
    Failed {
        message: String,
        refreshed: Option<RuntimeSnapshot>,
    },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("{0} is required")]
    Required(&'static str),
    #[error("{0} must be text")]
    ExpectedText(&'static str),
    #[error("{0} must be a choice")]
    ExpectedChoice(&'static str),
    #[error("{0} is invalid")]
    InvalidChoice(&'static str),
    #[error("{0} must be explicitly confirmed")]
    NotConfirmed(&'static str),
    #[error("the current {0} is unavailable")]
    MissingCurrent(&'static str),
}

pub struct CommandExecutor<C> {
    client: C,
}

impl<C> CommandExecutor<C> {
    pub fn new(client: C) -> Self {
        Self { client }
    }

    pub fn client(&self) -> &C {
        &self.client
    }

    pub fn client_mut(&mut self) -> &mut C {
        &mut self.client
    }

    pub fn into_client(self) -> C {
        self.client
    }
}

impl<C: HerdrClient> CommandExecutor<C> {
    pub fn execute(
        &mut self,
        command: CommandId,
        values: &CompletedArguments,
        snapshot: &RuntimeSnapshot,
    ) -> ExecutionOutcome {
        match command {
            CommandId::Plugin(action_id) => self.execute_plugin(action_id),
            CommandId::Core(command) => self.execute_core(command, values, snapshot),
        }
    }

    pub fn focus_entity(
        &mut self,
        id: PaletteItemId,
        snapshot: &RuntimeSnapshot,
    ) -> ExecutionOutcome {
        let operation = match &id {
            PaletteItemId::Workspace(workspace_id) => HerdrOperation::WorkspaceFocus {
                workspace_id: workspace_id.clone(),
            },
            PaletteItemId::Tab(tab_id) => HerdrOperation::TabFocus {
                tab_id: tab_id.clone(),
            },
            PaletteItemId::Agent(pane_id) => HerdrOperation::AgentFocus {
                pane_id: pane_id.clone(),
            },
            PaletteItemId::Command(_) => {
                return ExecutionOutcome::Failed {
                    message: "command item cannot be focused".into(),
                    refreshed: None,
                }
            }
        };
        self.execute_entity_operation(id, operation, snapshot)
    }

    fn execute_entity_operation(
        &mut self,
        id: PaletteItemId,
        operation: HerdrOperation,
        snapshot: &RuntimeSnapshot,
    ) -> ExecutionOutcome {
        match self.client.dispatch(&operation) {
            Ok(_) => ExecutionOutcome::Succeeded,
            Err(error) if is_entity_refreshable_stale(&id, &error) => {
                let refreshed = match self.refresh_entity(snapshot) {
                    Ok(refreshed) => refreshed,
                    Err(refresh_error) => {
                        return ExecutionOutcome::Failed {
                            message: format!("{error}; failed to refresh runtime: {refresh_error}"),
                            refreshed: None,
                        }
                    }
                };
                if !entity_target_exists(&id, &refreshed) {
                    return failed(error, Some(refreshed));
                }
                match self.client.dispatch(&operation) {
                    Ok(_) => ExecutionOutcome::Succeeded,
                    Err(retry_error) => failed(retry_error, Some(refreshed)),
                }
            }
            Err(error) => failed(error, None),
        }
    }

    fn execute_plugin(&mut self, action_id: String) -> ExecutionOutcome {
        match self
            .client
            .dispatch(&HerdrOperation::PluginActionInvoke { action_id })
        {
            Ok(_) => ExecutionOutcome::Succeeded,
            Err(error) => failed(error, None),
        }
    }

    fn execute_core(
        &mut self,
        command: CoreCommand,
        values: &CompletedArguments,
        snapshot: &RuntimeSnapshot,
    ) -> ExecutionOutcome {
        let operations = match core_operations(command, values, snapshot) {
            Ok(operations) => operations,
            Err(error) => {
                return ExecutionOutcome::Failed {
                    message: error.to_string(),
                    refreshed: None,
                }
            }
        };
        match command {
            CoreCommand::RunCommandInNewPane => self.execute_run(operations, values),
            CoreCommand::SwitchWorkspace
            | CoreCommand::CreateWorkspace
            | CoreCommand::RenameWorkspace
            | CoreCommand::CloseWorkspace
            | CoreCommand::OpenWorktree
            | CoreCommand::CreateWorktree
            | CoreCommand::SwitchTab
            | CoreCommand::CreateTab
            | CoreCommand::RenameTab
            | CoreCommand::CloseTab
            | CoreCommand::FocusPane(_)
            | CoreCommand::SplitPane(_)
            | CoreCommand::TogglePaneZoom
            | CoreCommand::RenameFocusedPane
            | CoreCommand::ClosePane
            | CoreCommand::SwitchAgent
            | CoreCommand::RenameFocusedAgent
            | CoreCommand::PromptFocusedAgent
            | CoreCommand::StartAgent => {
                let Some(operation) = operations.into_iter().next() else {
                    return ExecutionOutcome::Failed {
                        message: "command produced no operation".into(),
                        refreshed: None,
                    };
                };
                self.execute_single(command, operation, snapshot)
            }
        }
    }

    fn execute_run(
        &mut self,
        operations: Vec<HerdrOperation>,
        values: &CompletedArguments,
    ) -> ExecutionOutcome {
        let Some(split) = operations.into_iter().next() else {
            return ExecutionOutcome::Failed {
                message: "run command produced no pane split".into(),
                refreshed: None,
            };
        };
        let command = match required_visible(values, ArgumentKey::Command, "command") {
            Ok(command) => command,
            Err(error) => return validation_failed(error),
        };
        let pane_id = match self.client.dispatch(&split) {
            Ok(OperationResponse::PaneCreated { pane_id }) => pane_id,
            Ok(OperationResponse::Empty) => {
                return ExecutionOutcome::Failed {
                    message: "Herdr did not return the newly created pane".into(),
                    refreshed: None,
                }
            }
            Err(error) => return failed(error, None),
        };
        match self.client.dispatch(&HerdrOperation::PaneRun {
            pane_id: pane_id.clone(),
            command,
        }) {
            Ok(_) => ExecutionOutcome::Succeeded,
            Err(error) => ExecutionOutcome::Failed {
                message: format!(
                    "failed to run command in newly created pane {pane_id}; the new pane remains: {error}"
                ),
                refreshed: None,
            },
        }
    }

    fn execute_single(
        &mut self,
        command: CoreCommand,
        operation: HerdrOperation,
        snapshot: &RuntimeSnapshot,
    ) -> ExecutionOutcome {
        match self.client.dispatch(&operation) {
            Ok(_) => ExecutionOutcome::Succeeded,
            Err(error) => {
                let Some(target) = stable_target(command, &operation) else {
                    return failed(error, None);
                };
                if !is_refreshable_stale(command, &error) {
                    return failed(error, None);
                }
                let refreshed = match self.refresh(snapshot) {
                    Ok(refreshed) => refreshed,
                    Err(refresh_error) => {
                        return ExecutionOutcome::Failed {
                            message: format!("{error}; failed to refresh runtime: {refresh_error}"),
                            refreshed: None,
                        }
                    }
                };
                if !retryable(command) || !retry_target_still_valid(command, &target, &refreshed) {
                    return failed(error, Some(refreshed));
                }
                match self.client.dispatch(&operation) {
                    Ok(_) => ExecutionOutcome::Succeeded,
                    Err(retry_error) => failed(retry_error, Some(refreshed)),
                }
            }
        }
    }

    fn refresh(&mut self, previous: &RuntimeSnapshot) -> Result<RuntimeSnapshot, ClientError> {
        let capabilities = self.client.api_capabilities()?;
        let session = self.client.session_snapshot()?;
        let worktrees = match session.focused_workspace_id.as_deref() {
            Some(workspace_id) => match self.client.worktree_list(workspace_id) {
                Ok(worktrees) => worktrees,
                Err(error) if error.is_not_git_worktree() => Vec::new(),
                Err(error) => return Err(error),
            },
            None => Vec::new(),
        };
        Ok(RuntimeSnapshot {
            session,
            worktrees,
            invocation: previous.invocation.clone(),
            capabilities,
        })
    }

    fn refresh_entity(
        &mut self,
        previous: &RuntimeSnapshot,
    ) -> Result<RuntimeSnapshot, ClientError> {
        let session = self.client.session_snapshot()?;
        let worktrees = match session.focused_workspace_id.as_deref() {
            Some(workspace_id) => match self.client.worktree_list(workspace_id) {
                Ok(worktrees) => worktrees,
                Err(error) if error.is_not_git_worktree() => Vec::new(),
                Err(error) => return Err(error),
            },
            None => Vec::new(),
        };
        Ok(RuntimeSnapshot {
            session,
            worktrees,
            invocation: previous.invocation.clone(),
            capabilities: previous.capabilities,
        })
    }
}

fn is_entity_refreshable_stale(id: &PaletteItemId, error: &ClientError) -> bool {
    let Some(code) = error.code() else {
        return false;
    };
    matches!(
        (id, code),
        (PaletteItemId::Workspace(_), "workspace_not_found")
            | (PaletteItemId::Tab(_), "tab_not_found")
            | (PaletteItemId::Agent(_), "agent_not_found")
    )
}

fn entity_target_exists(id: &PaletteItemId, snapshot: &RuntimeSnapshot) -> bool {
    match id {
        PaletteItemId::Workspace(id) => snapshot
            .session
            .workspaces
            .iter()
            .any(|workspace| !id.is_empty() && workspace.workspace_id == *id),
        PaletteItemId::Tab(id) => snapshot
            .session
            .tabs
            .iter()
            .find(|tab| tab.tab_id == *id)
            .is_some_and(|tab| {
                snapshot
                    .session
                    .workspaces
                    .iter()
                    .any(|workspace| workspace.workspace_id == tab.workspace_id)
            }),
        PaletteItemId::Agent(id) => snapshot
            .session
            .agents
            .iter()
            .find(|agent| agent.pane_id == *id)
            .is_some_and(|agent| {
                let workspace_exists = snapshot
                    .session
                    .workspaces
                    .iter()
                    .any(|workspace| workspace.workspace_id == agent.workspace_id);
                let tab_exists = snapshot.session.tabs.iter().any(|tab| {
                    tab.tab_id == agent.tab_id && tab.workspace_id == agent.workspace_id
                });
                let pane_exists = snapshot.session.panes.iter().any(|pane| {
                    pane.pane_id == agent.pane_id
                        && pane.workspace_id == agent.workspace_id
                        && pane.tab_id == agent.tab_id
                });
                workspace_exists && tab_exists && pane_exists
            }),
        PaletteItemId::Command(_) => false,
    }
}

fn failed(error: ClientError, refreshed: Option<RuntimeSnapshot>) -> ExecutionOutcome {
    ExecutionOutcome::Failed {
        message: error.to_string(),
        refreshed,
    }
}

fn validation_failed(error: ValidationError) -> ExecutionOutcome {
    ExecutionOutcome::Failed {
        message: error.to_string(),
        refreshed: None,
    }
}

fn core_operations(
    command: CoreCommand,
    values: &CompletedArguments,
    snapshot: &RuntimeSnapshot,
) -> Result<Vec<HerdrOperation>, ValidationError> {
    let workspace_id = || current_workspace_id(snapshot);
    let tab_id = || current_tab_id(snapshot);
    let pane_id = || current_pane_id(snapshot);
    match command {
        CoreCommand::SwitchWorkspace => Ok(vec![HerdrOperation::WorkspaceFocus {
            workspace_id: required_choice(values, ArgumentKey::Workspace, "workspace")?,
        }]),
        CoreCommand::CreateWorkspace => Ok(vec![HerdrOperation::WorkspaceCreate {
            label: required_trimmed(values, ArgumentKey::Label, "workspace label")?,
            cwd: optional_trimmed(values, ArgumentKey::Cwd, "workspace cwd")?.map(PathBuf::from),
        }]),
        CoreCommand::RenameWorkspace => Ok(vec![HerdrOperation::WorkspaceRename {
            workspace_id: workspace_id()?,
            label: required_trimmed(values, ArgumentKey::Label, "workspace label")?,
        }]),
        CoreCommand::CloseWorkspace => {
            confirmed(values, "workspace close")?;
            Ok(vec![HerdrOperation::WorkspaceClose {
                workspace_id: workspace_id()?,
            }])
        }
        CoreCommand::OpenWorktree => Ok(vec![HerdrOperation::WorktreeOpen {
            workspace_id: workspace_id()?,
            path: PathBuf::from(required_choice(values, ArgumentKey::Worktree, "worktree")?),
        }]),
        CoreCommand::CreateWorktree => Ok(vec![HerdrOperation::WorktreeCreate {
            workspace_id: workspace_id()?,
            branch: required_trimmed(values, ArgumentKey::Branch, "branch")?,
            base: optional_trimmed(values, ArgumentKey::Base, "base")?
                .unwrap_or_else(|| "HEAD".into()),
            label: optional_trimmed(values, ArgumentKey::Label, "worktree label")?,
        }]),
        CoreCommand::SwitchTab => Ok(vec![HerdrOperation::TabFocus {
            tab_id: required_choice(values, ArgumentKey::Tab, "tab")?,
        }]),
        CoreCommand::CreateTab => Ok(vec![HerdrOperation::TabCreate {
            workspace_id: workspace_id()?,
            label: required_trimmed(values, ArgumentKey::Label, "tab label")?,
            cwd: optional_trimmed(values, ArgumentKey::Cwd, "tab cwd")?.map(PathBuf::from),
        }]),
        CoreCommand::RenameTab => Ok(vec![HerdrOperation::TabRename {
            tab_id: tab_id()?,
            label: required_trimmed(values, ArgumentKey::Label, "tab label")?,
        }]),
        CoreCommand::CloseTab => {
            confirmed(values, "tab close")?;
            Ok(vec![HerdrOperation::TabClose { tab_id: tab_id()? }])
        }
        CoreCommand::FocusPane(direction) => Ok(vec![HerdrOperation::PaneFocus {
            pane_id: pane_id()?,
            direction,
        }]),
        CoreCommand::SplitPane(direction) => Ok(vec![HerdrOperation::PaneSplit {
            pane_id: pane_id()?,
            direction,
            cwd: optional_trimmed(values, ArgumentKey::Cwd, "pane cwd")?.map(PathBuf::from),
        }]),
        CoreCommand::TogglePaneZoom => Ok(vec![HerdrOperation::PaneZoomToggle {
            pane_id: pane_id()?,
        }]),
        CoreCommand::RenameFocusedPane => Ok(vec![HerdrOperation::PaneRename {
            pane_id: pane_id()?,
            label: optional_trimmed(values, ArgumentKey::Label, "pane label")?,
        }]),
        CoreCommand::ClosePane => {
            confirmed(values, "pane close")?;
            Ok(vec![HerdrOperation::PaneClose {
                pane_id: pane_id()?,
            }])
        }
        CoreCommand::RunCommandInNewPane => {
            required_visible(values, ArgumentKey::Command, "command")?;
            let direction =
                match required_choice(values, ArgumentKey::Direction, "direction")?.as_str() {
                    "right" => SplitDirection::Right,
                    "down" => SplitDirection::Down,
                    _ => return Err(ValidationError::InvalidChoice("direction")),
                };
            Ok(vec![HerdrOperation::PaneSplit {
                pane_id: pane_id()?,
                direction,
                cwd: None,
            }])
        }
        CoreCommand::SwitchAgent => Ok(vec![HerdrOperation::AgentFocus {
            pane_id: required_choice(values, ArgumentKey::Agent, "agent")?,
        }]),
        CoreCommand::RenameFocusedAgent => Ok(vec![HerdrOperation::AgentRename {
            pane_id: current_agent_pane_id(snapshot)?,
            name: optional_trimmed(values, ArgumentKey::Name, "agent name")?,
        }]),
        CoreCommand::PromptFocusedAgent => Ok(vec![HerdrOperation::AgentPrompt {
            pane_id: current_agent_pane_id(snapshot)?,
            text: required_visible(values, ArgumentKey::Prompt, "prompt")?,
        }]),
        CoreCommand::StartAgent => {
            let kind = required_trimmed(values, ArgumentKey::Kind, "agent kind")?;
            let name = optional_trimmed(values, ArgumentKey::Name, "agent name")?
                .unwrap_or_else(|| kind.clone());
            Ok(vec![HerdrOperation::AgentStart {
                name,
                kind,
                pane_id: pane_id()?,
            }])
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StableTarget {
    Workspace(String),
    Worktree(String),
    Tab(String),
    Pane(String),
    Agent(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StableTargetKind {
    Workspace,
    Worktree,
    Tab,
    Pane,
    Agent,
}

impl StableTarget {
    fn kind(&self) -> StableTargetKind {
        match self {
            Self::Workspace(_) => StableTargetKind::Workspace,
            Self::Worktree(_) => StableTargetKind::Worktree,
            Self::Tab(_) => StableTargetKind::Tab,
            Self::Pane(_) => StableTargetKind::Pane,
            Self::Agent(_) => StableTargetKind::Agent,
        }
    }
}

fn stable_target(command: CoreCommand, operation: &HerdrOperation) -> Option<StableTarget> {
    let target = match operation {
        HerdrOperation::WorkspaceFocus { workspace_id } => {
            Some(StableTarget::Workspace(workspace_id.clone()))
        }
        HerdrOperation::WorktreeOpen { path, .. } => {
            Some(StableTarget::Worktree(path.to_string_lossy().into_owned()))
        }
        HerdrOperation::TabFocus { tab_id } => Some(StableTarget::Tab(tab_id.clone())),
        HerdrOperation::PaneFocus { pane_id, .. }
        | HerdrOperation::PaneZoomToggle { pane_id }
        | HerdrOperation::PaneRename { pane_id, .. } => Some(StableTarget::Pane(pane_id.clone())),
        HerdrOperation::AgentFocus { pane_id }
        | HerdrOperation::AgentRename { pane_id, .. }
        | HerdrOperation::AgentPrompt { pane_id, .. } => Some(StableTarget::Agent(pane_id.clone())),
        HerdrOperation::WorkspaceCreate { .. }
        | HerdrOperation::WorkspaceRename { .. }
        | HerdrOperation::WorkspaceClose { .. }
        | HerdrOperation::WorktreeCreate { .. }
        | HerdrOperation::TabCreate { .. }
        | HerdrOperation::TabRename { .. }
        | HerdrOperation::TabClose { .. }
        | HerdrOperation::PaneSplit { .. }
        | HerdrOperation::PaneClose { .. }
        | HerdrOperation::PaneRun { .. }
        | HerdrOperation::AgentStart { .. }
        | HerdrOperation::PluginActionInvoke { .. } => None,
    }?;
    let required_kind = match command {
        CoreCommand::SwitchWorkspace => Some(StableTargetKind::Workspace),
        CoreCommand::OpenWorktree => Some(StableTargetKind::Worktree),
        CoreCommand::SwitchTab => Some(StableTargetKind::Tab),
        CoreCommand::FocusPane(_)
        | CoreCommand::TogglePaneZoom
        | CoreCommand::RenameFocusedPane => Some(StableTargetKind::Pane),
        CoreCommand::SwitchAgent
        | CoreCommand::RenameFocusedAgent
        | CoreCommand::PromptFocusedAgent => Some(StableTargetKind::Agent),
        CoreCommand::CreateWorkspace
        | CoreCommand::RenameWorkspace
        | CoreCommand::CloseWorkspace
        | CoreCommand::CreateWorktree
        | CoreCommand::CreateTab
        | CoreCommand::RenameTab
        | CoreCommand::CloseTab
        | CoreCommand::SplitPane(_)
        | CoreCommand::ClosePane
        | CoreCommand::RunCommandInNewPane
        | CoreCommand::StartAgent => None,
    };
    (required_kind == Some(target.kind())).then_some(target)
}

fn is_refreshable_stale(command: CoreCommand, error: &ClientError) -> bool {
    let Some(code) = error.code() else {
        return false;
    };
    match command {
        CoreCommand::SwitchWorkspace => code == "workspace_not_found",
        CoreCommand::OpenWorktree => code == "worktree_not_found",
        CoreCommand::SwitchTab => code == "tab_not_found",
        CoreCommand::FocusPane(_) => code == "pane_not_found" || code == "target_pane_not_found",
        CoreCommand::TogglePaneZoom | CoreCommand::RenameFocusedPane => code == "pane_not_found",
        CoreCommand::SwitchAgent
        | CoreCommand::RenameFocusedAgent
        | CoreCommand::PromptFocusedAgent => code == "agent_not_found",
        CoreCommand::CreateWorkspace
        | CoreCommand::RenameWorkspace
        | CoreCommand::CloseWorkspace
        | CoreCommand::CreateWorktree
        | CoreCommand::CreateTab
        | CoreCommand::RenameTab
        | CoreCommand::CloseTab
        | CoreCommand::SplitPane(_)
        | CoreCommand::ClosePane
        | CoreCommand::RunCommandInNewPane
        | CoreCommand::StartAgent => false,
    }
}

fn retryable(command: CoreCommand) -> bool {
    match command {
        CoreCommand::SwitchWorkspace
        | CoreCommand::SwitchTab
        | CoreCommand::FocusPane(_)
        | CoreCommand::TogglePaneZoom
        | CoreCommand::RenameFocusedPane
        | CoreCommand::SwitchAgent
        | CoreCommand::RenameFocusedAgent
        | CoreCommand::PromptFocusedAgent => true,
        CoreCommand::CreateWorkspace
        | CoreCommand::RenameWorkspace
        | CoreCommand::CloseWorkspace
        | CoreCommand::OpenWorktree
        | CoreCommand::CreateWorktree
        | CoreCommand::CreateTab
        | CoreCommand::RenameTab
        | CoreCommand::CloseTab
        | CoreCommand::SplitPane(_)
        | CoreCommand::ClosePane
        | CoreCommand::RunCommandInNewPane
        | CoreCommand::StartAgent => false,
    }
}

fn retry_target_still_valid(
    command: CoreCommand,
    original: &StableTarget,
    refreshed: &RuntimeSnapshot,
) -> bool {
    match (command, original) {
        (CoreCommand::SwitchWorkspace, StableTarget::Workspace(id)) => refreshed
            .session
            .workspaces
            .iter()
            .any(|workspace| workspace.workspace_id == *id),
        (CoreCommand::OpenWorktree, StableTarget::Worktree(path)) => refreshed
            .worktrees
            .iter()
            .any(|worktree| worktree.path == *path),
        (CoreCommand::SwitchTab, StableTarget::Tab(id)) => {
            refreshed.session.tabs.iter().any(|tab| tab.tab_id == *id)
        }
        (CoreCommand::FocusPane(_), StableTarget::Pane(id))
        | (CoreCommand::TogglePaneZoom, StableTarget::Pane(id))
        | (CoreCommand::RenameFocusedPane, StableTarget::Pane(id)) => refreshed
            .session
            .panes
            .iter()
            .any(|pane| pane.pane_id == *id),
        (CoreCommand::SwitchAgent, StableTarget::Agent(pane_id))
        | (CoreCommand::RenameFocusedAgent, StableTarget::Agent(pane_id))
        | (CoreCommand::PromptFocusedAgent, StableTarget::Agent(pane_id)) => refreshed
            .session
            .agents
            .iter()
            .any(|agent| agent.pane_id == *pane_id),
        (CoreCommand::CreateWorkspace, _)
        | (CoreCommand::RenameWorkspace, _)
        | (CoreCommand::CloseWorkspace, _)
        | (CoreCommand::CreateWorktree, _)
        | (CoreCommand::CreateTab, _)
        | (CoreCommand::RenameTab, _)
        | (CoreCommand::CloseTab, _)
        | (CoreCommand::SplitPane(_), _)
        | (CoreCommand::ClosePane, _)
        | (CoreCommand::RunCommandInNewPane, _)
        | (CoreCommand::StartAgent, _) => false,
        (CoreCommand::SwitchWorkspace, _)
        | (CoreCommand::OpenWorktree, _)
        | (CoreCommand::SwitchTab, _)
        | (CoreCommand::FocusPane(_), _)
        | (CoreCommand::TogglePaneZoom, _)
        | (CoreCommand::RenameFocusedPane, _)
        | (CoreCommand::SwitchAgent, _)
        | (CoreCommand::RenameFocusedAgent, _)
        | (CoreCommand::PromptFocusedAgent, _) => false,
    }
}

fn required_trimmed(
    values: &CompletedArguments,
    key: ArgumentKey,
    name: &'static str,
) -> Result<String, ValidationError> {
    let value = required_text(values, key, name)?;
    let value = value.trim();
    if value.is_empty() {
        return Err(ValidationError::Required(name));
    }
    Ok(value.into())
}

fn required_visible(
    values: &CompletedArguments,
    key: ArgumentKey,
    name: &'static str,
) -> Result<String, ValidationError> {
    let value = required_text(values, key, name)?;
    if value.trim().is_empty() {
        return Err(ValidationError::Required(name));
    }
    Ok(value.into())
}

fn required_text<'a>(
    values: &'a CompletedArguments,
    key: ArgumentKey,
    name: &'static str,
) -> Result<&'a str, ValidationError> {
    match values.get(key) {
        Some(ArgumentValue::Text(value)) => Ok(value),
        Some(ArgumentValue::Choice(_)) | Some(ArgumentValue::Confirmation(_)) => {
            Err(ValidationError::ExpectedText(name))
        }
        None => Err(ValidationError::Required(name)),
    }
}

fn optional_trimmed(
    values: &CompletedArguments,
    key: ArgumentKey,
    name: &'static str,
) -> Result<Option<String>, ValidationError> {
    match values.get(key) {
        Some(ArgumentValue::Text(value)) => {
            let value = value.trim();
            Ok((!value.is_empty()).then(|| value.into()))
        }
        Some(ArgumentValue::Choice(_)) | Some(ArgumentValue::Confirmation(_)) => {
            Err(ValidationError::ExpectedText(name))
        }
        None => Ok(None),
    }
}

fn required_choice(
    values: &CompletedArguments,
    key: ArgumentKey,
    name: &'static str,
) -> Result<String, ValidationError> {
    match values.get(key) {
        Some(ArgumentValue::Choice(value)) if !value.trim().is_empty() => Ok(value.clone()),
        Some(ArgumentValue::Choice(_)) => Err(ValidationError::Required(name)),
        Some(ArgumentValue::Text(_)) | Some(ArgumentValue::Confirmation(_)) => {
            Err(ValidationError::ExpectedChoice(name))
        }
        None => Err(ValidationError::Required(name)),
    }
}

fn confirmed(values: &CompletedArguments, name: &'static str) -> Result<(), ValidationError> {
    match values.get(ArgumentKey::Confirm) {
        Some(ArgumentValue::Confirmation(Confirmation::Yes)) => Ok(()),
        Some(ArgumentValue::Confirmation(Confirmation::No))
        | Some(ArgumentValue::Text(_))
        | Some(ArgumentValue::Choice(_))
        | None => Err(ValidationError::NotConfirmed(name)),
    }
}

fn current_workspace_id(snapshot: &RuntimeSnapshot) -> Result<String, ValidationError> {
    snapshot
        .focused_workspace()
        .map(|workspace| workspace.workspace_id.clone())
        .ok_or(ValidationError::MissingCurrent("workspace"))
}

fn current_tab_id(snapshot: &RuntimeSnapshot) -> Result<String, ValidationError> {
    snapshot
        .focused_tab()
        .map(|tab| tab.tab_id.clone())
        .ok_or(ValidationError::MissingCurrent("tab"))
}

fn current_pane_id(snapshot: &RuntimeSnapshot) -> Result<String, ValidationError> {
    snapshot
        .focused_pane()
        .map(|pane| pane.pane_id.clone())
        .ok_or(ValidationError::MissingCurrent("pane"))
}

fn current_agent_pane_id(snapshot: &RuntimeSnapshot) -> Result<String, ValidationError> {
    snapshot
        .focused_agent()
        .map(|agent| agent.pane_id.clone())
        .ok_or(ValidationError::MissingCurrent("agent"))
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, VecDeque};
    use std::path::PathBuf;

    use super::{core_operations, CommandExecutor, ExecutionOutcome, ValidationError};
    use crate::client::{HerdrClient, HerdrOperation, OperationResponse};
    use crate::command::{ArgumentKey, CommandId, CoreCommand, Direction, SplitDirection};
    use crate::context::parse_invocation_context;
    use crate::error::ClientError;
    use crate::model::{
        ApiCapabilities, InstalledPluginInfo, PluginActionInfo, RuntimeSnapshot,
        SessionSnapshotResult, SuccessEnvelope, WorktreeInfo, WorktreeListResult,
    };
    use crate::registry::PaletteItemId;
    use crate::state::{ArgumentValue, CompletedArguments};

    fn runtime() -> RuntimeSnapshot {
        let session: SuccessEnvelope<SessionSnapshotResult> =
            serde_json::from_str(include_str!("../tests/fixtures/session-snapshot.json")).unwrap();
        let worktrees: SuccessEnvelope<WorktreeListResult> =
            serde_json::from_str(include_str!("../tests/fixtures/worktree-list.json")).unwrap();
        RuntimeSnapshot {
            session: session.result.snapshot,
            worktrees: worktrees.result.worktrees,
            invocation: parse_invocation_context(include_str!(
                "../tests/fixtures/plugin-invocation-context.json"
            ))
            .unwrap(),
            capabilities: ApiCapabilities {
                typed_agent_start: true,
                agent_prompt: true,
            },
        }
    }

    fn args(values: &[(ArgumentKey, ArgumentValue)]) -> CompletedArguments {
        CompletedArguments::from_values(values.iter().cloned().collect::<BTreeMap<_, _>>())
    }

    fn text(value: &str) -> ArgumentValue {
        ArgumentValue::Text(value.into())
    }
    fn choice(value: &str) -> ArgumentValue {
        ArgumentValue::Choice(value.into())
    }
    fn yes() -> ArgumentValue {
        ArgumentValue::Confirmation(crate::state::Confirmation::Yes)
    }

    #[test]
    fn every_core_identity_maps_to_the_exact_typed_operation() {
        let snapshot = runtime();
        let cases = vec![
            (
                CoreCommand::SwitchWorkspace,
                args(&[(ArgumentKey::Workspace, choice("ws-2"))]),
                vec![HerdrOperation::WorkspaceFocus {
                    workspace_id: "ws-2".into(),
                }],
            ),
            (
                CoreCommand::CreateWorkspace,
                args(&[
                    (ArgumentKey::Label, text("  New workspace  ")),
                    (ArgumentKey::Cwd, text("  /tmp/new  ")),
                ]),
                vec![HerdrOperation::WorkspaceCreate {
                    label: "New workspace".into(),
                    cwd: Some(PathBuf::from("/tmp/new")),
                }],
            ),
            (
                CoreCommand::RenameWorkspace,
                args(&[(ArgumentKey::Label, text("  Renamed  "))]),
                vec![HerdrOperation::WorkspaceRename {
                    workspace_id: "ws-1".into(),
                    label: "Renamed".into(),
                }],
            ),
            (
                CoreCommand::CloseWorkspace,
                args(&[(ArgumentKey::Confirm, yes())]),
                vec![HerdrOperation::WorkspaceClose {
                    workspace_id: "ws-1".into(),
                }],
            ),
            (
                CoreCommand::OpenWorktree,
                args(&[(ArgumentKey::Worktree, choice("/repo/feature"))]),
                vec![HerdrOperation::WorktreeOpen {
                    workspace_id: "ws-1".into(),
                    path: PathBuf::from("/repo/feature"),
                }],
            ),
            (
                CoreCommand::CreateWorktree,
                args(&[(ArgumentKey::Branch, text("  branch  "))]),
                vec![HerdrOperation::WorktreeCreate {
                    workspace_id: "ws-1".into(),
                    branch: "branch".into(),
                    base: "HEAD".into(),
                    label: None,
                }],
            ),
            (
                CoreCommand::SwitchTab,
                args(&[(ArgumentKey::Tab, choice("tab-2"))]),
                vec![HerdrOperation::TabFocus {
                    tab_id: "tab-2".into(),
                }],
            ),
            (
                CoreCommand::CreateTab,
                args(&[(ArgumentKey::Label, text("  New tab  "))]),
                vec![HerdrOperation::TabCreate {
                    workspace_id: "ws-1".into(),
                    label: "New tab".into(),
                    cwd: None,
                }],
            ),
            (
                CoreCommand::RenameTab,
                args(&[(ArgumentKey::Label, text("  Renamed tab  "))]),
                vec![HerdrOperation::TabRename {
                    tab_id: "tab-1".into(),
                    label: "Renamed tab".into(),
                }],
            ),
            (
                CoreCommand::CloseTab,
                args(&[(ArgumentKey::Confirm, yes())]),
                vec![HerdrOperation::TabClose {
                    tab_id: "tab-1".into(),
                }],
            ),
            (
                CoreCommand::FocusPane(Direction::Left),
                args(&[]),
                vec![HerdrOperation::PaneFocus {
                    pane_id: "pane-a".into(),
                    direction: Direction::Left,
                }],
            ),
            (
                CoreCommand::FocusPane(Direction::Right),
                args(&[]),
                vec![HerdrOperation::PaneFocus {
                    pane_id: "pane-a".into(),
                    direction: Direction::Right,
                }],
            ),
            (
                CoreCommand::FocusPane(Direction::Up),
                args(&[]),
                vec![HerdrOperation::PaneFocus {
                    pane_id: "pane-a".into(),
                    direction: Direction::Up,
                }],
            ),
            (
                CoreCommand::FocusPane(Direction::Down),
                args(&[]),
                vec![HerdrOperation::PaneFocus {
                    pane_id: "pane-a".into(),
                    direction: Direction::Down,
                }],
            ),
            (
                CoreCommand::SplitPane(SplitDirection::Right),
                args(&[(ArgumentKey::Cwd, text("  /tmp/split  "))]),
                vec![HerdrOperation::PaneSplit {
                    pane_id: "pane-a".into(),
                    direction: SplitDirection::Right,
                    cwd: Some(PathBuf::from("/tmp/split")),
                }],
            ),
            (
                CoreCommand::SplitPane(SplitDirection::Down),
                args(&[]),
                vec![HerdrOperation::PaneSplit {
                    pane_id: "pane-a".into(),
                    direction: SplitDirection::Down,
                    cwd: None,
                }],
            ),
            (
                CoreCommand::TogglePaneZoom,
                args(&[]),
                vec![HerdrOperation::PaneZoomToggle {
                    pane_id: "pane-a".into(),
                }],
            ),
            (
                CoreCommand::RenameFocusedPane,
                args(&[(ArgumentKey::Label, text("  build logs  "))]),
                vec![HerdrOperation::PaneRename {
                    pane_id: "pane-a".into(),
                    label: Some("build logs".into()),
                }],
            ),
            (
                CoreCommand::ClosePane,
                args(&[(ArgumentKey::Confirm, yes())]),
                vec![HerdrOperation::PaneClose {
                    pane_id: "pane-a".into(),
                }],
            ),
            (
                CoreCommand::RunCommandInNewPane,
                args(&[
                    (ArgumentKey::Command, text("  echo hi  ")),
                    (ArgumentKey::Direction, choice("down")),
                ]),
                vec![HerdrOperation::PaneSplit {
                    pane_id: "pane-a".into(),
                    direction: SplitDirection::Down,
                    cwd: None,
                }],
            ),
            (
                CoreCommand::SwitchAgent,
                args(&[(ArgumentKey::Agent, choice("pane-d"))]),
                vec![HerdrOperation::AgentFocus {
                    pane_id: "pane-d".into(),
                }],
            ),
            (
                CoreCommand::RenameFocusedAgent,
                args(&[(ArgumentKey::Name, text("  review pair #2  "))]),
                vec![HerdrOperation::AgentRename {
                    pane_id: "pane-a".into(),
                    name: Some("review pair #2".into()),
                }],
            ),
            (
                CoreCommand::PromptFocusedAgent,
                args(&[(ArgumentKey::Prompt, text("  keep whitespace  "))]),
                vec![HerdrOperation::AgentPrompt {
                    pane_id: "pane-a".into(),
                    text: "  keep whitespace  ".into(),
                }],
            ),
            (
                CoreCommand::StartAgent,
                args(&[(ArgumentKey::Kind, text("  codex  "))]),
                vec![HerdrOperation::AgentStart {
                    name: "codex".into(),
                    kind: "codex".into(),
                    pane_id: "pane-a".into(),
                }],
            ),
        ];
        assert_eq!(cases.len(), CoreCommand::all().len());
        for (command, values, expected) in cases {
            assert_eq!(
                core_operations(command, &values, &snapshot).unwrap(),
                expected,
                "{}",
                command.stable_id()
            );
        }
    }

    #[test]
    fn optional_values_are_omitted_and_visible_text_is_preserved() {
        let snapshot = runtime();
        let workspace = core_operations(
            CoreCommand::CreateWorkspace,
            &args(&[
                (ArgumentKey::Label, text("name")),
                (ArgumentKey::Cwd, text(" \t ")),
            ]),
            &snapshot,
        )
        .unwrap();
        assert_eq!(
            workspace,
            vec![HerdrOperation::WorkspaceCreate {
                label: "name".into(),
                cwd: None
            }]
        );
        let worktree = core_operations(
            CoreCommand::CreateWorktree,
            &args(&[
                (ArgumentKey::Branch, text("branch")),
                (ArgumentKey::Base, text("  main  ")),
                (ArgumentKey::Label, text("  label  ")),
            ]),
            &snapshot,
        )
        .unwrap();
        assert_eq!(
            worktree,
            vec![HerdrOperation::WorktreeCreate {
                workspace_id: "ws-1".into(),
                branch: "branch".into(),
                base: "main".into(),
                label: Some("label".into())
            }]
        );
    }

    #[test]
    fn rename_focused_pane_maps_blank_to_clear_and_requires_the_focused_pane() {
        let snapshot = runtime();
        assert_eq!(
            core_operations(
                CoreCommand::RenameFocusedPane,
                &args(&[(ArgumentKey::Label, text(" \t "))]),
                &snapshot,
            )
            .unwrap(),
            [HerdrOperation::PaneRename {
                pane_id: "pane-a".into(),
                label: None,
            }]
        );

        let mut without_pane = snapshot;
        without_pane.session.focused_pane_id = None;
        assert_eq!(
            core_operations(
                CoreCommand::RenameFocusedPane,
                &args(&[(ArgumentKey::Label, text("build logs"))]),
                &without_pane,
            ),
            Err(ValidationError::MissingCurrent("pane"))
        );
    }

    #[test]
    fn rename_focused_agent_maps_blank_to_clear_and_requires_the_focused_agent() {
        let snapshot = runtime();
        assert_eq!(
            core_operations(
                CoreCommand::RenameFocusedAgent,
                &args(&[(ArgumentKey::Name, text(" \t "))]),
                &snapshot,
            )
            .unwrap(),
            [HerdrOperation::AgentRename {
                pane_id: "pane-a".into(),
                name: None,
            }]
        );

        let mut without_agent = snapshot;
        without_agent
            .session
            .agents
            .retain(|agent| agent.pane_id != "pane-a");
        assert_eq!(
            core_operations(
                CoreCommand::RenameFocusedAgent,
                &args(&[(ArgumentKey::Name, text("review pair #2"))]),
                &without_agent,
            ),
            Err(ValidationError::MissingCurrent("agent"))
        );
    }

    #[derive(Default)]
    struct FakeClient {
        dispatches: VecDeque<Result<OperationResponse, ClientError>>,
        operations: Vec<HerdrOperation>,
        capabilities: VecDeque<Result<ApiCapabilities, ClientError>>,
        sessions: VecDeque<Result<crate::model::SessionSnapshot, ClientError>>,
        worktrees: VecDeque<Result<Vec<WorktreeInfo>, ClientError>>,
        refresh_calls: Vec<&'static str>,
        worktree_workspace_ids: Vec<String>,
    }

    impl FakeClient {
        fn with_dispatches(
            dispatches: impl IntoIterator<Item = Result<OperationResponse, ClientError>>,
        ) -> Self {
            Self {
                dispatches: dispatches.into_iter().collect(),
                ..Self::default()
            }
        }

        fn refresh_from(snapshot: &RuntimeSnapshot) -> Self {
            Self {
                capabilities: [Ok(snapshot.capabilities)].into(),
                sessions: [Ok(snapshot.session.clone())].into(),
                worktrees: [Ok(snapshot.worktrees.clone())].into(),
                ..Self::default()
            }
        }
    }

    impl HerdrClient for FakeClient {
        fn api_capabilities(&mut self) -> Result<ApiCapabilities, ClientError> {
            self.refresh_calls.push("capabilities");
            self.capabilities.pop_front().unwrap_or_else(|| {
                Err(ClientError::Process {
                    message: "unexpected capabilities request".into(),
                })
            })
        }
        fn session_snapshot(&mut self) -> Result<crate::model::SessionSnapshot, ClientError> {
            self.refresh_calls.push("snapshot");
            self.sessions.pop_front().unwrap_or_else(|| {
                Err(ClientError::Process {
                    message: "unexpected snapshot request".into(),
                })
            })
        }
        fn worktree_list(&mut self, workspace_id: &str) -> Result<Vec<WorktreeInfo>, ClientError> {
            self.refresh_calls.push("worktrees");
            self.worktree_workspace_ids.push(workspace_id.into());
            self.worktrees.pop_front().unwrap_or_else(|| {
                Err(ClientError::Process {
                    message: "unexpected worktree request".into(),
                })
            })
        }
        fn plugin_list(&mut self) -> Result<Vec<InstalledPluginInfo>, ClientError> {
            Ok(Vec::new())
        }
        fn plugin_action_list(&mut self) -> Result<Vec<PluginActionInfo>, ClientError> {
            Ok(Vec::new())
        }
        fn open_palette(&mut self) -> Result<(), ClientError> {
            Ok(())
        }
        fn dispatch(
            &mut self,
            operation: &HerdrOperation,
        ) -> Result<OperationResponse, ClientError> {
            self.operations.push(operation.clone());
            self.dispatches
                .pop_front()
                .unwrap_or(Ok(OperationResponse::Empty))
        }
    }

    #[test]
    fn workspace_tab_and_agent_items_map_to_exact_public_focus_operations() {
        let mut executor = CommandExecutor::new(FakeClient::with_dispatches([
            Ok(OperationResponse::Empty),
            Ok(OperationResponse::Empty),
            Ok(OperationResponse::Empty),
        ]));
        let snapshot = runtime();

        assert_eq!(
            executor.focus_entity(PaletteItemId::Workspace("ws-2".into()), &snapshot),
            ExecutionOutcome::Succeeded
        );
        assert_eq!(
            executor.focus_entity(PaletteItemId::Tab("tab-2".into()), &snapshot),
            ExecutionOutcome::Succeeded
        );
        assert_eq!(
            executor.focus_entity(PaletteItemId::Agent("pane-d".into()), &snapshot),
            ExecutionOutcome::Succeeded
        );
        assert_eq!(
            executor.client().operations,
            [
                HerdrOperation::WorkspaceFocus {
                    workspace_id: "ws-2".into(),
                },
                HerdrOperation::TabFocus {
                    tab_id: "tab-2".into(),
                },
                HerdrOperation::AgentFocus {
                    pane_id: "pane-d".into(),
                },
            ]
        );
    }

    #[test]
    fn cross_workspace_tab_focus_dispatches_only_tab_focus() {
        let mut executor =
            CommandExecutor::new(FakeClient::with_dispatches([Ok(OperationResponse::Empty)]));

        assert_eq!(
            executor.focus_entity(PaletteItemId::Tab("tab-2".into()), &runtime()),
            ExecutionOutcome::Succeeded
        );
        assert_eq!(
            executor.client().operations,
            [HerdrOperation::TabFocus {
                tab_id: "tab-2".into(),
            }]
        );
    }

    #[test]
    fn entity_stale_error_refreshes_session_once_and_retries_same_typed_id() {
        let snapshot = runtime();
        let mut client = FakeClient::refresh_from(&snapshot);
        client.dispatches = [
            Err(ClientError::Api {
                code: "tab_not_found".into(),
                message: "stale".into(),
            }),
            Ok(OperationResponse::Empty),
        ]
        .into();
        let mut executor = CommandExecutor::new(client);

        assert_eq!(
            executor.focus_entity(PaletteItemId::Tab("tab-2".into()), &snapshot),
            ExecutionOutcome::Succeeded
        );
        assert_eq!(
            executor.client().operations,
            [
                HerdrOperation::TabFocus {
                    tab_id: "tab-2".into(),
                },
                HerdrOperation::TabFocus {
                    tab_id: "tab-2".into(),
                },
            ]
        );
        assert_eq!(executor.client().refresh_calls, ["snapshot", "worktrees"]);
        assert_eq!(executor.client().worktree_workspace_ids, ["ws-1"]);
    }

    #[test]
    fn entity_stale_refresh_never_retargets_same_label_replacement() {
        let snapshot = runtime();
        let mut refreshed = snapshot.clone();
        refreshed
            .session
            .agents
            .retain(|agent| agent.pane_id != "pane-d");
        let mut replacement = snapshot
            .session
            .agents
            .iter()
            .find(|agent| agent.pane_id == "pane-d")
            .cloned()
            .unwrap();
        replacement.pane_id = "pane-replacement".into();
        refreshed.session.agents.push(replacement);
        let mut client = FakeClient::refresh_from(&refreshed);
        client.dispatches = [Err(ClientError::Api {
            code: "agent_not_found".into(),
            message: "stale".into(),
        })]
        .into();
        let mut executor = CommandExecutor::new(client);

        assert!(matches!(
            executor.focus_entity(PaletteItemId::Agent("pane-d".into()), &snapshot),
            ExecutionOutcome::Failed {
                refreshed: Some(value),
                ..
            } if value == refreshed
        ));
        assert_eq!(
            executor.client().operations,
            [HerdrOperation::AgentFocus {
                pane_id: "pane-d".into(),
            }]
        );
        assert_eq!(executor.client().refresh_calls, ["snapshot", "worktrees"]);
    }

    #[test]
    fn wrong_resource_or_transport_error_does_not_entity_refresh() {
        let snapshot = runtime();
        for error in [
            ClientError::Api {
                code: "workspace_not_found".into(),
                message: "wrong resource".into(),
            },
            ClientError::Process {
                message: "transport failed".into(),
            },
        ] {
            let mut executor = CommandExecutor::new(FakeClient::with_dispatches([Err(error)]));
            assert!(matches!(
                executor.focus_entity(PaletteItemId::Tab("tab-2".into()), &snapshot),
                ExecutionOutcome::Failed {
                    refreshed: None,
                    ..
                }
            ));
            assert_eq!(executor.client().operations.len(), 1);
            assert!(executor.client().refresh_calls.is_empty());
        }
    }

    #[test]
    fn entity_retry_does_not_retry_a_second_failure() {
        let snapshot = runtime();
        let mut client = FakeClient::refresh_from(&snapshot);
        client.dispatches = [
            Err(ClientError::Api {
                code: "workspace_not_found".into(),
                message: "stale".into(),
            }),
            Err(ClientError::Api {
                code: "workspace_not_found".into(),
                message: "still stale".into(),
            }),
        ]
        .into();
        let mut executor = CommandExecutor::new(client);

        assert!(matches!(
            executor.focus_entity(PaletteItemId::Workspace("ws-2".into()), &snapshot),
            ExecutionOutcome::Failed {
                refreshed: Some(_),
                ..
            }
        ));
        assert_eq!(executor.client().operations.len(), 2);
        assert_eq!(executor.client().refresh_calls, ["snapshot", "worktrees"]);
    }

    #[test]
    fn plugin_dispatches_its_qualified_id_without_palette_arguments() {
        let client = FakeClient::with_dispatches([Ok(OperationResponse::Empty)]);
        let mut executor = CommandExecutor::new(client);
        assert!(matches!(
            executor.execute(
                CommandId::Plugin("demo.tool.run".into()),
                &args(&[]),
                &runtime()
            ),
            ExecutionOutcome::Succeeded
        ));
        assert_eq!(
            executor.client().operations,
            vec![HerdrOperation::PluginActionInvoke {
                action_id: "demo.tool.run".into()
            }]
        );
    }

    #[test]
    fn run_command_creates_a_pane_then_runs_command() {
        let client = FakeClient::with_dispatches([
            Ok(OperationResponse::PaneCreated {
                pane_id: "pane-new".into(),
            }),
            Ok(OperationResponse::Empty),
        ]);
        let mut executor = CommandExecutor::new(client);
        assert!(matches!(
            executor.execute(
                CommandId::Core(CoreCommand::RunCommandInNewPane),
                &args(&[
                    (ArgumentKey::Command, text(" echo hi ")),
                    (ArgumentKey::Direction, choice("right")),
                ]),
                &runtime(),
            ),
            ExecutionOutcome::Succeeded
        ));
        assert_eq!(
            executor.client().operations,
            vec![
                HerdrOperation::PaneSplit {
                    pane_id: "pane-a".into(),
                    direction: SplitDirection::Right,
                    cwd: None,
                },
                HerdrOperation::PaneRun {
                    pane_id: "pane-new".into(),
                    command: " echo hi ".into(),
                },
            ]
        );
    }

    #[test]
    fn run_command_rejects_an_empty_split_response_without_running_the_command() {
        let client = FakeClient::with_dispatches([Ok(OperationResponse::Empty)]);
        let mut executor = CommandExecutor::new(client);
        let outcome = executor.execute(
            CommandId::Core(CoreCommand::RunCommandInNewPane),
            &args(&[
                (ArgumentKey::Command, text("echo hi")),
                (ArgumentKey::Direction, choice("right")),
            ]),
            &runtime(),
        );
        assert!(
            matches!(outcome, ExecutionOutcome::Failed { ref message, refreshed: None } if message == "Herdr did not return the newly created pane")
        );
        assert_eq!(
            executor.client().operations,
            vec![HerdrOperation::PaneSplit {
                pane_id: "pane-a".into(),
                direction: SplitDirection::Right,
                cwd: None,
            }]
        );
    }

    #[test]
    fn run_command_requires_a_created_pane_and_reports_partial_success() {
        let client = FakeClient::with_dispatches([
            Ok(OperationResponse::PaneCreated {
                pane_id: "pane-new".into(),
            }),
            Err(ClientError::Api {
                code: "run_failed".into(),
                message: "bad command".into(),
            }),
        ]);
        let mut executor = CommandExecutor::new(client);
        let outcome = executor.execute(
            CommandId::Core(CoreCommand::RunCommandInNewPane),
            &args(&[
                (ArgumentKey::Command, text(" echo hi ")),
                (ArgumentKey::Direction, choice("right")),
            ]),
            &runtime(),
        );
        assert!(
            matches!(outcome, ExecutionOutcome::Failed { ref message, refreshed: None } if message.contains("pane-new") && message.contains("new pane remains"))
        );
        assert_eq!(
            executor.client().operations,
            vec![
                HerdrOperation::PaneSplit {
                    pane_id: "pane-a".into(),
                    direction: SplitDirection::Right,
                    cwd: None
                },
                HerdrOperation::PaneRun {
                    pane_id: "pane-new".into(),
                    command: " echo hi ".into()
                },
            ]
        );
    }

    #[test]
    fn rename_focused_agent_missing_current_agent_does_not_dispatch() {
        let mut snapshot = runtime();
        snapshot
            .session
            .agents
            .retain(|agent| agent.pane_id != "pane-a");
        let mut executor = CommandExecutor::new(FakeClient::default());

        assert!(matches!(
            executor.execute(
                CommandId::Core(CoreCommand::RenameFocusedAgent),
                &args(&[(ArgumentKey::Name, text("review pair #2"))]),
                &snapshot,
            ),
            ExecutionOutcome::Failed { ref message, refreshed: None }
                if message == "the current agent is unavailable"
        ));
        assert!(executor.client().operations.is_empty());
    }

    #[test]
    fn rename_pane_stale_retry_uses_only_the_same_pane_id() {
        let snapshot = runtime();
        let mut client = FakeClient::refresh_from(&snapshot);
        client.dispatches = [
            Err(ClientError::Api {
                code: "pane_not_found".into(),
                message: "stale pane".into(),
            }),
            Ok(OperationResponse::Empty),
        ]
        .into();
        let mut executor = CommandExecutor::new(client);
        let values = args(&[(ArgumentKey::Label, text("build logs"))]);

        assert_eq!(
            executor.execute(
                CommandId::Core(CoreCommand::RenameFocusedPane),
                &values,
                &snapshot,
            ),
            ExecutionOutcome::Succeeded
        );
        assert_eq!(
            executor.client().operations,
            [
                HerdrOperation::PaneRename {
                    pane_id: "pane-a".into(),
                    label: Some("build logs".into()),
                },
                HerdrOperation::PaneRename {
                    pane_id: "pane-a".into(),
                    label: Some("build logs".into()),
                },
            ]
        );
    }

    #[test]
    fn rename_pane_stale_refresh_never_retargets_a_same_labeled_replacement() {
        let snapshot = runtime();
        let mut refreshed = snapshot.clone();
        let mut replacement = refreshed
            .session
            .panes
            .iter()
            .find(|pane| pane.pane_id == "pane-a")
            .cloned()
            .unwrap();
        replacement.pane_id = "pane-replacement".into();
        replacement.label = Some("agent".into());
        refreshed
            .session
            .panes
            .retain(|pane| pane.pane_id != "pane-a");
        refreshed.session.panes.push(replacement);

        let mut client = FakeClient::refresh_from(&refreshed);
        client.dispatches = [Err(ClientError::Api {
            code: "pane_not_found".into(),
            message: "gone".into(),
        })]
        .into();
        let mut executor = CommandExecutor::new(client);
        let outcome = executor.execute(
            CommandId::Core(CoreCommand::RenameFocusedPane),
            &args(&[(ArgumentKey::Label, text("build logs"))]),
            &snapshot,
        );

        assert!(matches!(
            outcome,
            ExecutionOutcome::Failed {
                refreshed: Some(ref value),
                ..
            } if value == &refreshed
        ));
        assert_eq!(
            executor.client().operations,
            [HerdrOperation::PaneRename {
                pane_id: "pane-a".into(),
                label: Some("build logs".into()),
            }]
        );
    }

    #[test]
    fn rename_pane_only_refreshes_for_pane_not_found() {
        let snapshot = runtime();
        let mut executor =
            CommandExecutor::new(FakeClient::with_dispatches([Err(ClientError::Api {
                code: "agent_not_found".into(),
                message: "wrong resource".into(),
            })]));

        assert!(matches!(
            executor.execute(
                CommandId::Core(CoreCommand::RenameFocusedPane),
                &args(&[(ArgumentKey::Label, text("build logs"))]),
                &snapshot,
            ),
            ExecutionOutcome::Failed {
                refreshed: None,
                ..
            }
        ));
        assert_eq!(executor.client().operations.len(), 1);
        assert!(executor.client().refresh_calls.is_empty());
    }

    #[test]
    fn rename_agent_stale_retry_uses_only_the_same_pane_id() {
        let snapshot = runtime();
        let mut client = FakeClient::refresh_from(&snapshot);
        client.dispatches = [
            Err(ClientError::Api {
                code: "agent_not_found".into(),
                message: "stale agent".into(),
            }),
            Ok(OperationResponse::Empty),
        ]
        .into();
        let mut executor = CommandExecutor::new(client);
        let values = args(&[(ArgumentKey::Name, text("review pair #2"))]);

        assert_eq!(
            executor.execute(
                CommandId::Core(CoreCommand::RenameFocusedAgent),
                &values,
                &snapshot,
            ),
            ExecutionOutcome::Succeeded
        );
        assert_eq!(
            executor.client().operations,
            [
                HerdrOperation::AgentRename {
                    pane_id: "pane-a".into(),
                    name: Some("review pair #2".into()),
                },
                HerdrOperation::AgentRename {
                    pane_id: "pane-a".into(),
                    name: Some("review pair #2".into()),
                },
            ]
        );
    }

    #[test]
    fn rename_agent_stale_refresh_never_retargets_a_same_named_replacement() {
        let snapshot = runtime();
        let mut refreshed = snapshot.clone();
        let mut replacement = refreshed
            .session
            .agents
            .iter()
            .find(|agent| agent.pane_id == "pane-a")
            .cloned()
            .unwrap();
        replacement.pane_id = "pane-replacement".into();
        replacement.name = Some("reviewer".into());
        refreshed
            .session
            .agents
            .retain(|agent| agent.pane_id != "pane-a");
        refreshed.session.agents.push(replacement);

        let mut client = FakeClient::refresh_from(&refreshed);
        client.dispatches = [Err(ClientError::Api {
            code: "agent_not_found".into(),
            message: "gone".into(),
        })]
        .into();
        let mut executor = CommandExecutor::new(client);
        let outcome = executor.execute(
            CommandId::Core(CoreCommand::RenameFocusedAgent),
            &args(&[(ArgumentKey::Name, text("review pair #2"))]),
            &snapshot,
        );

        assert!(matches!(
            outcome,
            ExecutionOutcome::Failed { refreshed: Some(ref value), .. } if value == &refreshed
        ));
        assert_eq!(
            executor.client().operations,
            [HerdrOperation::AgentRename {
                pane_id: "pane-a".into(),
                name: Some("review pair #2".into()),
            }]
        );
    }

    #[test]
    fn rename_agent_only_refreshes_for_agent_not_found() {
        let snapshot = runtime();
        let mut executor =
            CommandExecutor::new(FakeClient::with_dispatches([Err(ClientError::Api {
                code: "pane_not_found".into(),
                message: "wrong resource".into(),
            })]));

        assert!(matches!(
            executor.execute(
                CommandId::Core(CoreCommand::RenameFocusedAgent),
                &args(&[(ArgumentKey::Name, text("review pair #2"))]),
                &snapshot,
            ),
            ExecutionOutcome::Failed {
                refreshed: None,
                ..
            }
        ));
        assert_eq!(executor.client().operations.len(), 1);
        assert!(executor.client().refresh_calls.is_empty());
    }

    #[test]
    fn every_retryable_family_refreshes_once_and_retries_only_when_its_stable_target_survives() {
        let snapshot = runtime();
        let cases = [
            (
                CoreCommand::SwitchWorkspace,
                args(&[(ArgumentKey::Workspace, choice("ws-2"))]),
                "workspace_not_found",
            ),
            (
                CoreCommand::SwitchTab,
                args(&[(ArgumentKey::Tab, choice("tab-2"))]),
                "tab_not_found",
            ),
            (
                CoreCommand::FocusPane(Direction::Right),
                args(&[]),
                "pane_not_found",
            ),
            (CoreCommand::TogglePaneZoom, args(&[]), "pane_not_found"),
            (
                CoreCommand::RenameFocusedPane,
                args(&[(ArgumentKey::Label, text("build logs"))]),
                "pane_not_found",
            ),
            (
                CoreCommand::SwitchAgent,
                args(&[(ArgumentKey::Agent, choice("pane-d"))]),
                "agent_not_found",
            ),
            (
                CoreCommand::RenameFocusedAgent,
                args(&[(ArgumentKey::Name, text("review pair #2"))]),
                "agent_not_found",
            ),
            (
                CoreCommand::PromptFocusedAgent,
                args(&[(ArgumentKey::Prompt, text("hello"))]),
                "agent_not_found",
            ),
        ];
        for (command, values, code) in cases {
            let mut client = FakeClient::refresh_from(&snapshot);
            client.dispatches = [
                Err(ClientError::Api {
                    code: code.into(),
                    message: "stale".into(),
                }),
                Ok(OperationResponse::Empty),
            ]
            .into();
            let mut executor = CommandExecutor::new(client);
            assert!(
                matches!(
                    executor.execute(CommandId::Core(command), &values, &snapshot),
                    ExecutionOutcome::Succeeded
                ),
                "{}",
                command.stable_id()
            );
            assert_eq!(
                executor.client().operations.len(),
                2,
                "{}",
                command.stable_id()
            );
        }
    }

    #[test]
    fn stale_retry_refuses_changed_targets_and_returns_the_complete_refresh() {
        let snapshot = runtime();
        let mut refreshed = snapshot.clone();
        refreshed
            .session
            .workspaces
            .retain(|workspace| workspace.workspace_id != "ws-2");
        let mut client = FakeClient::refresh_from(&refreshed);
        client.dispatches = [Err(ClientError::Api {
            code: "workspace_not_found".into(),
            message: "gone".into(),
        })]
        .into();
        let mut executor = CommandExecutor::new(client);
        let outcome = executor.execute(
            CommandId::Core(CoreCommand::SwitchWorkspace),
            &args(&[(ArgumentKey::Workspace, choice("ws-2"))]),
            &snapshot,
        );
        assert!(
            matches!(outcome, ExecutionOutcome::Failed { refreshed: Some(ref value), .. } if value == &refreshed)
        );
        assert_eq!(executor.client().operations.len(), 1);
    }

    #[test]
    fn every_retryable_family_refuses_a_changed_stable_target_after_one_complete_refresh() {
        let snapshot = runtime();
        let cases = [
            (
                CoreCommand::SwitchWorkspace,
                args(&[(ArgumentKey::Workspace, choice("ws-2"))]),
                "workspace_not_found",
            ),
            (
                CoreCommand::SwitchTab,
                args(&[(ArgumentKey::Tab, choice("tab-2"))]),
                "tab_not_found",
            ),
            (
                CoreCommand::FocusPane(Direction::Right),
                args(&[]),
                "target_pane_not_found",
            ),
            (CoreCommand::TogglePaneZoom, args(&[]), "pane_not_found"),
            (
                CoreCommand::RenameFocusedPane,
                args(&[(ArgumentKey::Label, text("build logs"))]),
                "pane_not_found",
            ),
            (
                CoreCommand::SwitchAgent,
                args(&[(ArgumentKey::Agent, choice("pane-d"))]),
                "agent_not_found",
            ),
            (
                CoreCommand::RenameFocusedAgent,
                args(&[(ArgumentKey::Name, text("review pair #2"))]),
                "agent_not_found",
            ),
            (
                CoreCommand::PromptFocusedAgent,
                args(&[(ArgumentKey::Prompt, text("hello"))]),
                "agent_not_found",
            ),
        ];
        for (command, values, code) in cases {
            let mut refreshed = snapshot.clone();
            match command {
                CoreCommand::SwitchWorkspace => refreshed
                    .session
                    .workspaces
                    .retain(|workspace| workspace.workspace_id != "ws-2"),
                CoreCommand::SwitchTab => {
                    refreshed.session.tabs.retain(|tab| tab.tab_id != "tab-2")
                }
                CoreCommand::FocusPane(_)
                | CoreCommand::TogglePaneZoom
                | CoreCommand::RenameFocusedPane => refreshed
                    .session
                    .panes
                    .retain(|pane| pane.pane_id != "pane-a"),
                CoreCommand::SwitchAgent => refreshed
                    .session
                    .agents
                    .retain(|agent| agent.pane_id != "pane-d"),
                CoreCommand::RenameFocusedAgent | CoreCommand::PromptFocusedAgent => refreshed
                    .session
                    .agents
                    .retain(|agent| agent.pane_id != "pane-a"),
                CoreCommand::CreateWorkspace
                | CoreCommand::RenameWorkspace
                | CoreCommand::CloseWorkspace
                | CoreCommand::OpenWorktree
                | CoreCommand::CreateWorktree
                | CoreCommand::CreateTab
                | CoreCommand::RenameTab
                | CoreCommand::CloseTab
                | CoreCommand::SplitPane(_)
                | CoreCommand::ClosePane
                | CoreCommand::RunCommandInNewPane
                | CoreCommand::StartAgent => unreachable!("only retryable commands are tabled"),
            }
            let mut client = FakeClient::refresh_from(&refreshed);
            client.dispatches = [Err(ClientError::Api {
                code: code.into(),
                message: "gone".into(),
            })]
            .into();
            let mut executor = CommandExecutor::new(client);
            let outcome = executor.execute(CommandId::Core(command), &values, &snapshot);
            assert!(
                matches!(outcome, ExecutionOutcome::Failed { refreshed: Some(ref value), .. } if value == &refreshed),
                "{}",
                command.stable_id()
            );
            assert_eq!(
                executor.client().operations.len(),
                1,
                "{}",
                command.stable_id()
            );
            assert_eq!(
                executor.client().refresh_calls,
                ["capabilities", "snapshot", "worktrees"]
            );
            assert_eq!(executor.client().worktree_workspace_ids, ["ws-1"]);
        }
    }

    #[test]
    fn stale_refresh_ignores_not_git_worktree_and_retries_the_exact_target() {
        let previous = runtime();
        let refreshed_capabilities = ApiCapabilities {
            typed_agent_start: false,
            agent_prompt: false,
        };
        let mut refreshed_session = previous.session.clone();
        refreshed_session.version = "0.7.5".into();
        assert_ne!(refreshed_session, previous.session);
        let expected = RuntimeSnapshot {
            session: refreshed_session.clone(),
            worktrees: Vec::new(),
            invocation: previous.invocation.clone(),
            capabilities: refreshed_capabilities,
        };
        let client = FakeClient {
            dispatches: [
                Err(ClientError::Api {
                    code: "tab_not_found".into(),
                    message: "stale target".into(),
                }),
                Err(ClientError::Process {
                    message: "retry failed".into(),
                }),
            ]
            .into(),
            capabilities: [Ok(refreshed_capabilities)].into(),
            sessions: [Ok(refreshed_session)].into(),
            worktrees: [Err(ClientError::Api {
                code: "not_git_worktree".into(),
                message: "workspace is not a Git worktree".into(),
            })]
            .into(),
            ..FakeClient::default()
        };
        let mut executor = CommandExecutor::new(client);

        let outcome = executor.execute(
            CommandId::Core(CoreCommand::SwitchTab),
            &args(&[(ArgumentKey::Tab, choice("tab-2"))]),
            &previous,
        );

        assert!(matches!(
            outcome,
            ExecutionOutcome::Failed { refreshed: Some(ref runtime), .. } if runtime == &expected
        ));
        assert_eq!(
            executor.client().operations,
            [
                HerdrOperation::TabFocus {
                    tab_id: "tab-2".into(),
                },
                HerdrOperation::TabFocus {
                    tab_id: "tab-2".into(),
                },
            ]
        );
        assert_eq!(
            executor.client().refresh_calls,
            ["capabilities", "snapshot", "worktrees"]
        );
        assert_eq!(executor.client().worktree_workspace_ids, ["ws-1"]);
    }

    #[test]
    fn nonretryable_worktree_staleness_refreshes_once_and_returns_the_runtime() {
        let snapshot = runtime();
        let mut client = FakeClient::refresh_from(&snapshot);
        client.dispatches = [Err(ClientError::Api {
            code: "worktree_not_found".into(),
            message: "gone".into(),
        })]
        .into();
        let mut executor = CommandExecutor::new(client);
        let outcome = executor.execute(
            CommandId::Core(CoreCommand::OpenWorktree),
            &args(&[(ArgumentKey::Worktree, choice("/repo/feature"))]),
            &snapshot,
        );
        assert!(
            matches!(outcome, ExecutionOutcome::Failed { refreshed: Some(ref value), .. } if value == &snapshot)
        );
        assert_eq!(executor.client().operations.len(), 1);
        assert_eq!(
            executor.client().refresh_calls,
            ["capabilities", "snapshot", "worktrees"]
        );
        assert_eq!(executor.client().worktree_workspace_ids, ["ws-1"]);
    }

    #[test]
    fn wrong_resource_stale_codes_and_non_api_failures_do_not_refresh_or_retry() {
        let snapshot = runtime();
        let cases = [
            (
                ClientError::Api {
                    code: "workspace_not_found".into(),
                    message: "wrong resource".into(),
                },
                "wrong-resource stale code",
            ),
            (
                ClientError::Process {
                    message: "transport failed".into(),
                },
                "non-API failure",
            ),
        ];
        for (error, case) in cases {
            let client = FakeClient::with_dispatches([Err(error)]);
            let mut executor = CommandExecutor::new(client);
            assert!(
                matches!(
                    executor.execute(
                        CommandId::Core(CoreCommand::SwitchTab),
                        &args(&[(ArgumentKey::Tab, choice("tab-2"))]),
                        &snapshot,
                    ),
                    ExecutionOutcome::Failed {
                        refreshed: None,
                        ..
                    }
                ),
                "{case}"
            );
            assert_eq!(executor.client().operations.len(), 1, "{case}");
            assert!(executor.client().refresh_calls.is_empty(), "{case}");
        }
    }

    #[test]
    fn stale_refresh_without_a_focused_workspace_skips_worktrees_and_returns_the_snapshot() {
        let snapshot = runtime();
        let mut refreshed = snapshot.clone();
        refreshed.session.focused_workspace_id = None;
        refreshed
            .session
            .workspaces
            .retain(|workspace| workspace.workspace_id != "ws-2");
        refreshed.worktrees.clear();
        let mut client = FakeClient::refresh_from(&refreshed);
        client.dispatches = [Err(ClientError::Api {
            code: "workspace_not_found".into(),
            message: "gone".into(),
        })]
        .into();
        let mut executor = CommandExecutor::new(client);
        let outcome = executor.execute(
            CommandId::Core(CoreCommand::SwitchWorkspace),
            &args(&[(ArgumentKey::Workspace, choice("ws-2"))]),
            &snapshot,
        );
        assert!(
            matches!(outcome, ExecutionOutcome::Failed { refreshed: Some(ref value), .. } if value == &refreshed)
        );
        assert_eq!(executor.client().operations.len(), 1);
        assert_eq!(
            executor.client().refresh_calls,
            ["capabilities", "snapshot"]
        );
        assert!(executor.client().worktree_workspace_ids.is_empty());
    }

    #[test]
    fn retryable_commands_do_not_retry_a_second_failure() {
        let snapshot = runtime();
        let mut client = FakeClient::refresh_from(&snapshot);
        client.dispatches = [
            Err(ClientError::Api {
                code: "tab_not_found".into(),
                message: "stale".into(),
            }),
            Err(ClientError::Api {
                code: "tab_not_found".into(),
                message: "still stale".into(),
            }),
        ]
        .into();
        let mut executor = CommandExecutor::new(client);
        assert!(matches!(
            executor.execute(
                CommandId::Core(CoreCommand::SwitchTab),
                &args(&[(ArgumentKey::Tab, choice("tab-2"))]),
                &snapshot
            ),
            ExecutionOutcome::Failed {
                refreshed: Some(_),
                ..
            }
        ));
        assert_eq!(executor.client().operations.len(), 2);
    }

    #[test]
    fn create_rename_split_run_plugin_and_close_never_refresh_or_retry() {
        let snapshot = runtime();
        let cases = [
            (
                CommandId::Core(CoreCommand::CreateWorkspace),
                args(&[(ArgumentKey::Label, text("new"))]),
                "workspace_not_found",
            ),
            (
                CommandId::Core(CoreCommand::RenameWorkspace),
                args(&[(ArgumentKey::Label, text("new"))]),
                "workspace_not_found",
            ),
            (
                CommandId::Core(CoreCommand::SplitPane(SplitDirection::Right)),
                args(&[]),
                "pane_not_found",
            ),
            (
                CommandId::Core(CoreCommand::RunCommandInNewPane),
                args(&[
                    (ArgumentKey::Command, text("echo")),
                    (ArgumentKey::Direction, choice("right")),
                ]),
                "pane_not_found",
            ),
            (
                CommandId::Plugin("demo.run".into()),
                args(&[]),
                "pane_not_found",
            ),
            (
                CommandId::Core(CoreCommand::ClosePane),
                args(&[(ArgumentKey::Confirm, yes())]),
                "pane_not_found",
            ),
        ];
        for (command, values, code) in cases {
            let client = FakeClient::with_dispatches([Err(ClientError::Api {
                code: code.into(),
                message: "stale".into(),
            })]);
            let mut executor = CommandExecutor::new(client);
            assert!(matches!(
                executor.execute(command, &values, &snapshot),
                ExecutionOutcome::Failed {
                    refreshed: None,
                    ..
                }
            ));
            assert_eq!(executor.client().operations.len(), 1);
        }
    }
}
