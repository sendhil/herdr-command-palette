use std::fmt;

use crate::model::RuntimeSnapshot;
pub use crate::model::{Direction, SplitDirection};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreCommand {
    SwitchWorkspace,
    CreateWorkspace,
    RenameWorkspace,
    CloseWorkspace,
    OpenWorktree,
    CreateWorktree,
    SwitchTab,
    CreateTab,
    RenameTab,
    CloseTab,
    FocusPane(Direction),
    SplitPane(SplitDirection),
    TogglePaneZoom,
    RenameFocusedPane,
    ClosePane,
    RunCommandInNewPane,
    SwitchAgent,
    RenameFocusedAgent,
    PromptFocusedAgent,
    StartAgent,
}

const CORE_COMMANDS: [CoreCommand; 24] = [
    CoreCommand::SwitchWorkspace,
    CoreCommand::CreateWorkspace,
    CoreCommand::RenameWorkspace,
    CoreCommand::CloseWorkspace,
    CoreCommand::OpenWorktree,
    CoreCommand::CreateWorktree,
    CoreCommand::SwitchTab,
    CoreCommand::CreateTab,
    CoreCommand::RenameTab,
    CoreCommand::CloseTab,
    CoreCommand::FocusPane(Direction::Left),
    CoreCommand::FocusPane(Direction::Right),
    CoreCommand::FocusPane(Direction::Up),
    CoreCommand::FocusPane(Direction::Down),
    CoreCommand::SplitPane(SplitDirection::Right),
    CoreCommand::SplitPane(SplitDirection::Down),
    CoreCommand::TogglePaneZoom,
    CoreCommand::RenameFocusedPane,
    CoreCommand::ClosePane,
    CoreCommand::RunCommandInNewPane,
    CoreCommand::SwitchAgent,
    CoreCommand::RenameFocusedAgent,
    CoreCommand::PromptFocusedAgent,
    CoreCommand::StartAgent,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandId {
    Core(CoreCommand),
    Plugin(String),
}

impl CommandId {
    pub fn stable_id(&self) -> &str {
        match self {
            Self::Core(command) => command.stable_id(),
            Self::Plugin(id) => id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ArgumentKey {
    Workspace,
    Label,
    Cwd,
    Worktree,
    Branch,
    Base,
    Tab,
    Direction,
    Command,
    Agent,
    Prompt,
    Kind,
    Name,
    Confirm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgumentKind {
    Text,
    Choice,
    Confirm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgumentDefault {
    Value(String),
    From(ArgumentKey),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgumentChoice {
    pub value: String,
    pub label: String,
}

impl ArgumentChoice {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextRule {
    Trimmed,
    Visible,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgumentStep {
    key: ArgumentKey,
    kind: ArgumentKind,
    optional: bool,
    default: Option<ArgumentDefault>,
    choices: Vec<ArgumentChoice>,
    prompt: Option<String>,
    text_rule: Option<TextRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArgumentValidationError {
    #[error("this argument is not a text step")]
    NotText,
    #[error("a value is required")]
    Required,
}

impl ArgumentStep {
    pub fn key(&self) -> ArgumentKey {
        self.key
    }

    pub fn kind(&self) -> ArgumentKind {
        self.kind
    }

    pub fn optional(&self) -> bool {
        self.optional
    }

    pub fn default(&self) -> Option<&ArgumentDefault> {
        self.default.as_ref()
    }

    pub fn choices(&self) -> &[ArgumentChoice] {
        &self.choices
    }

    pub fn prompt(&self) -> Option<&str> {
        self.prompt.as_deref()
    }

    pub fn normalize_text(&self, input: &str) -> Result<Option<String>, ArgumentValidationError> {
        let Some(rule) = self.text_rule else {
            return Err(ArgumentValidationError::NotText);
        };
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return if self.optional {
                Ok(None)
            } else {
                Err(ArgumentValidationError::Required)
            };
        }
        match rule {
            TextRule::Trimmed => Ok(Some(trimmed.to_owned())),
            TextRule::Visible => Ok(Some(input.to_owned())),
        }
    }

    fn text(
        key: ArgumentKey,
        optional: bool,
        default: Option<ArgumentDefault>,
        rule: TextRule,
    ) -> Self {
        Self {
            key,
            kind: ArgumentKind::Text,
            optional,
            default,
            choices: Vec::new(),
            prompt: None,
            text_rule: Some(rule),
        }
    }

    fn choice(
        key: ArgumentKey,
        choices: Vec<ArgumentChoice>,
        default: Option<ArgumentDefault>,
    ) -> Self {
        Self {
            key,
            kind: ArgumentKind::Choice,
            optional: false,
            default,
            choices,
            prompt: None,
            text_rule: None,
        }
    }

    fn confirm(prompt: String) -> Self {
        Self {
            key: ArgumentKey::Confirm,
            kind: ArgumentKind::Confirm,
            optional: false,
            default: None,
            choices: Vec::new(),
            prompt: Some(prompt),
            text_rule: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandCategory {
    Workspace,
    Worktree,
    Tab,
    Pane,
    Agent,
    Plugin,
}

impl CommandCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Worktree => "worktree",
            Self::Tab => "tab",
            Self::Pane => "pane",
            Self::Agent => "agent",
            Self::Plugin => "plugin",
        }
    }
}

impl fmt::Display for CommandCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub id: CommandId,
    pub title: String,
    pub description: Option<String>,
    pub category: CommandCategory,
    pub aliases: Vec<String>,
    pub priority: u16,
    pub steps: Vec<ArgumentStep>,
    pub plugin_name: Option<String>,
}

impl CommandSpec {
    pub fn stable_id(&self) -> &str {
        self.id.stable_id()
    }
}

impl CoreCommand {
    pub fn all() -> &'static [CoreCommand] {
        &CORE_COMMANDS
    }

    pub fn stable_id(self) -> &'static str {
        match self {
            Self::SwitchWorkspace => "workspace.switch",
            Self::CreateWorkspace => "workspace.create",
            Self::RenameWorkspace => "workspace.rename",
            Self::CloseWorkspace => "workspace.close",
            Self::OpenWorktree => "worktree.open",
            Self::CreateWorktree => "worktree.create",
            Self::SwitchTab => "tab.switch",
            Self::CreateTab => "tab.create",
            Self::RenameTab => "tab.rename",
            Self::CloseTab => "tab.close",
            Self::FocusPane(Direction::Left) => "pane.focus-left",
            Self::FocusPane(Direction::Right) => "pane.focus-right",
            Self::FocusPane(Direction::Up) => "pane.focus-up",
            Self::FocusPane(Direction::Down) => "pane.focus-down",
            Self::SplitPane(SplitDirection::Right) => "pane.split-right",
            Self::SplitPane(SplitDirection::Down) => "pane.split-down",
            Self::TogglePaneZoom => "pane.zoom-toggle",
            Self::RenameFocusedPane => "pane.rename",
            Self::ClosePane => "pane.close",
            Self::RunCommandInNewPane => "pane.run",
            Self::SwitchAgent => "agent.switch",
            Self::RenameFocusedAgent => "agent.rename",
            Self::PromptFocusedAgent => "agent.prompt",
            Self::StartAgent => "agent.start",
        }
    }

    pub fn spec(self, runtime: &RuntimeSnapshot) -> Option<CommandSpec> {
        self.is_available(runtime)
            .then(|| self.catalog_spec(runtime))
    }

    pub fn catalog_spec(self, runtime: &RuntimeSnapshot) -> CommandSpec {
        let (title, description, category, aliases, priority) = self.metadata();
        CommandSpec {
            id: CommandId::Core(self),
            title: title.to_owned(),
            description: Some(description.to_owned()),
            category,
            aliases: aliases.iter().map(|alias| (*alias).to_owned()).collect(),
            priority,
            steps: self.steps(runtime),
            plugin_name: None,
        }
    }

    fn metadata(
        self,
    ) -> (
        &'static str,
        &'static str,
        CommandCategory,
        &'static [&'static str],
        u16,
    ) {
        match self {
            Self::SwitchWorkspace => (
                "Switch workspace",
                "Focus another workspace",
                CommandCategory::Workspace,
                &["workspace", "ws"],
                20,
            ),
            Self::CreateWorkspace => (
                "Create workspace",
                "Create and focus a workspace",
                CommandCategory::Workspace,
                &["new workspace"],
                30,
            ),
            Self::RenameWorkspace => (
                "Rename workspace",
                "Rename the current workspace",
                CommandCategory::Workspace,
                &["workspace name"],
                40,
            ),
            Self::CloseWorkspace => (
                "Close workspace",
                "Close the current workspace",
                CommandCategory::Workspace,
                &["remove workspace"],
                90,
            ),
            Self::OpenWorktree => (
                "Open worktree",
                "Open an existing Git worktree",
                CommandCategory::Worktree,
                &["checkout", "branch workspace"],
                20,
            ),
            Self::CreateWorktree => (
                "Create linked worktree",
                "Create and open a linked worktree",
                CommandCategory::Worktree,
                &["new worktree", "branch"],
                30,
            ),
            Self::SwitchTab => (
                "Switch tab",
                "Focus another tab",
                CommandCategory::Tab,
                &["tab", "goto tab"],
                20,
            ),
            Self::CreateTab => (
                "Create tab",
                "Create and focus a tab",
                CommandCategory::Tab,
                &["new tab"],
                30,
            ),
            Self::RenameTab => (
                "Rename tab",
                "Rename the current tab",
                CommandCategory::Tab,
                &["tab name"],
                40,
            ),
            Self::CloseTab => (
                "Close tab",
                "Close the current tab",
                CommandCategory::Tab,
                &["remove tab"],
                90,
            ),
            Self::FocusPane(Direction::Left) => (
                "Focus pane left",
                "Focus the pane to the left",
                CommandCategory::Pane,
                &["left pane"],
                10,
            ),
            Self::FocusPane(Direction::Right) => (
                "Focus pane right",
                "Focus the pane to the right",
                CommandCategory::Pane,
                &["right pane"],
                10,
            ),
            Self::FocusPane(Direction::Up) => (
                "Focus pane up",
                "Focus the pane above",
                CommandCategory::Pane,
                &["upper pane"],
                10,
            ),
            Self::FocusPane(Direction::Down) => (
                "Focus pane down",
                "Focus the pane below",
                CommandCategory::Pane,
                &["lower pane"],
                10,
            ),
            Self::SplitPane(SplitDirection::Right) => (
                "Split pane right",
                "Create a pane on the right",
                CommandCategory::Pane,
                &["new pane right"],
                30,
            ),
            Self::SplitPane(SplitDirection::Down) => (
                "Split pane down",
                "Create a pane below",
                CommandCategory::Pane,
                &["new pane below"],
                30,
            ),
            Self::TogglePaneZoom => (
                "Toggle pane zoom",
                "Toggle zoom for the focused pane",
                CommandCategory::Pane,
                &["maximize pane", "unzoom"],
                10,
            ),
            Self::RenameFocusedPane => (
                "Rename focused pane",
                "Rename the focused pane",
                CommandCategory::Pane,
                &["pane name", "rename current pane"],
                20,
            ),
            Self::ClosePane => (
                "Close pane",
                "Close the focused pane",
                CommandCategory::Pane,
                &["remove pane"],
                90,
            ),
            Self::RunCommandInNewPane => (
                "Run command in new pane",
                "Split and run a shell command",
                CommandCategory::Pane,
                &["command", "terminal"],
                30,
            ),
            Self::SwitchAgent => (
                "Switch agent",
                "Focus another running agent",
                CommandCategory::Agent,
                &["agent", "goto agent"],
                10,
            ),
            Self::RenameFocusedAgent => (
                "Rename focused agent",
                "Rename the agent in the focused pane",
                CommandCategory::Agent,
                &["agent name"],
                20,
            ),
            Self::PromptFocusedAgent => (
                "Prompt focused agent",
                "Send text to the focused agent",
                CommandCategory::Agent,
                &["ask agent", "send prompt"],
                10,
            ),
            Self::StartAgent => (
                "Start agent",
                "Start an agent in the focused pane",
                CommandCategory::Agent,
                &["launch agent", "new agent"],
                40,
            ),
        }
    }

    fn is_available(self, runtime: &RuntimeSnapshot) -> bool {
        let focused_workspace = runtime.focused_workspace();
        let focused_tab = runtime.focused_tab();
        let focused_pane = runtime.focused_pane();
        let current_tab_count = focused_workspace.map_or(0, |workspace| {
            runtime
                .session
                .tabs
                .iter()
                .filter(|tab| tab.workspace_id == workspace.workspace_id)
                .count()
        });
        let current_pane_count = focused_tab.map_or(0, |tab| {
            runtime
                .session
                .panes
                .iter()
                .filter(|pane| pane.tab_id == tab.tab_id)
                .count()
        });
        match self {
            Self::SwitchWorkspace => runtime.session.workspaces.len() >= 2,
            Self::CreateWorkspace => true,
            Self::RenameWorkspace => focused_workspace.is_some(),
            Self::CloseWorkspace => {
                focused_workspace.is_some() && runtime.session.workspaces.len() > 1
            }
            Self::OpenWorktree => {
                focused_workspace.is_some()
                    && runtime
                        .worktrees
                        .iter()
                        .any(|worktree| worktree.open_workspace_id.is_none())
            }
            Self::CreateWorktree => {
                focused_workspace.is_some_and(|workspace| workspace.worktree.is_some())
            }
            Self::SwitchTab => focused_workspace.is_some() && current_tab_count >= 2,
            Self::CreateTab => focused_workspace.is_some(),
            Self::RenameTab => focused_tab.is_some(),
            Self::CloseTab => focused_tab.is_some() && current_tab_count >= 2,
            Self::FocusPane(Direction::Left) => runtime.has_neighbor(Direction::Left),
            Self::FocusPane(Direction::Right) => runtime.has_neighbor(Direction::Right),
            Self::FocusPane(Direction::Up) => runtime.has_neighbor(Direction::Up),
            Self::FocusPane(Direction::Down) => runtime.has_neighbor(Direction::Down),
            Self::SplitPane(SplitDirection::Right) => focused_pane.is_some(),
            Self::SplitPane(SplitDirection::Down) => focused_pane.is_some(),
            Self::TogglePaneZoom => focused_pane.is_some(),
            Self::RenameFocusedPane => focused_pane.is_some(),
            Self::ClosePane => focused_pane.is_some() && current_pane_count >= 2,
            Self::RunCommandInNewPane => focused_pane.is_some(),
            Self::SwitchAgent => runtime.session.agents.iter().any(|agent| !agent.focused),
            Self::RenameFocusedAgent => runtime.focused_agent().is_some(),
            Self::PromptFocusedAgent => {
                runtime.capabilities.agent_prompt && runtime.focused_agent().is_some()
            }
            Self::StartAgent => {
                focused_pane.is_some()
                    && runtime.capabilities.typed_agent_start
                    && focused_pane.is_some_and(|pane| !runtime.has_active_agent(&pane.pane_id))
            }
        }
    }

    fn steps(self, runtime: &RuntimeSnapshot) -> Vec<ArgumentStep> {
        let current_tab_id = runtime.session.focused_tab_id.as_deref().or_else(|| {
            runtime
                .focused_workspace()
                .map(|workspace| workspace.active_tab_id.as_str())
        });
        let focused_pane_cwd = runtime
            .focused_pane()
            .and_then(|pane| pane.cwd.as_deref())
            .map(|cwd| ArgumentDefault::Value(cwd.to_owned()));
        match self {
            Self::SwitchWorkspace => vec![ArgumentStep::choice(
                ArgumentKey::Workspace,
                runtime
                    .session
                    .workspaces
                    .iter()
                    .filter(|workspace| {
                        Some(workspace.workspace_id.as_str())
                            != runtime.session.focused_workspace_id.as_deref()
                    })
                    .map(|workspace| ArgumentChoice::new(&workspace.workspace_id, &workspace.label))
                    .collect(),
                None,
            )],
            Self::CreateWorkspace => vec![
                ArgumentStep::text(ArgumentKey::Label, false, None, TextRule::Trimmed),
                ArgumentStep::text(
                    ArgumentKey::Cwd,
                    true,
                    runtime
                        .current_workspace_cwd()
                        .map(|cwd| ArgumentDefault::Value(cwd.to_owned())),
                    TextRule::Trimmed,
                ),
            ],
            Self::RenameWorkspace => vec![ArgumentStep::text(
                ArgumentKey::Label,
                false,
                runtime
                    .focused_workspace()
                    .map(|workspace| ArgumentDefault::Value(workspace.label.clone())),
                TextRule::Trimmed,
            )],
            Self::CloseWorkspace => vec![ArgumentStep::confirm(format!(
                "Close workspace {}?",
                runtime
                    .focused_workspace()
                    .map_or("", |workspace| workspace.label.as_str())
            ))],
            Self::OpenWorktree => vec![ArgumentStep::choice(
                ArgumentKey::Worktree,
                runtime
                    .worktrees
                    .iter()
                    .filter(|worktree| worktree.open_workspace_id.is_none())
                    .map(|worktree| ArgumentChoice::new(&worktree.path, &worktree.label))
                    .collect(),
                None,
            )],
            Self::CreateWorktree => vec![
                ArgumentStep::text(ArgumentKey::Branch, false, None, TextRule::Trimmed),
                ArgumentStep::text(
                    ArgumentKey::Base,
                    true,
                    Some(ArgumentDefault::Value("HEAD".into())),
                    TextRule::Trimmed,
                ),
                ArgumentStep::text(ArgumentKey::Label, true, None, TextRule::Trimmed),
            ],
            Self::SwitchTab => vec![ArgumentStep::choice(
                ArgumentKey::Tab,
                runtime
                    .session
                    .tabs
                    .iter()
                    .filter(|tab| {
                        Some(tab.workspace_id.as_str())
                            == runtime.session.focused_workspace_id.as_deref()
                    })
                    .filter(|tab| Some(tab.tab_id.as_str()) != current_tab_id)
                    .map(|tab| ArgumentChoice::new(&tab.tab_id, &tab.label))
                    .collect(),
                None,
            )],
            Self::CreateTab => vec![
                ArgumentStep::text(ArgumentKey::Label, false, None, TextRule::Trimmed),
                ArgumentStep::text(ArgumentKey::Cwd, true, focused_pane_cwd, TextRule::Trimmed),
            ],
            Self::RenameTab => vec![ArgumentStep::text(
                ArgumentKey::Label,
                false,
                runtime
                    .focused_tab()
                    .map(|tab| ArgumentDefault::Value(tab.label.clone())),
                TextRule::Trimmed,
            )],
            Self::CloseTab => vec![ArgumentStep::confirm(format!(
                "Close tab {}?",
                runtime.focused_tab().map_or("", |tab| tab.label.as_str())
            ))],
            Self::FocusPane(Direction::Left) => Vec::new(),
            Self::FocusPane(Direction::Right) => Vec::new(),
            Self::FocusPane(Direction::Up) => Vec::new(),
            Self::FocusPane(Direction::Down) => Vec::new(),
            Self::SplitPane(SplitDirection::Right) => vec![ArgumentStep::text(
                ArgumentKey::Cwd,
                true,
                focused_pane_cwd,
                TextRule::Trimmed,
            )],
            Self::SplitPane(SplitDirection::Down) => vec![ArgumentStep::text(
                ArgumentKey::Cwd,
                true,
                focused_pane_cwd,
                TextRule::Trimmed,
            )],
            Self::TogglePaneZoom => Vec::new(),
            Self::RenameFocusedPane => vec![ArgumentStep::text(
                ArgumentKey::Label,
                true,
                runtime.focused_pane().and_then(|pane| {
                    pane.label
                        .as_deref()
                        .map(str::trim)
                        .filter(|label| !label.is_empty())
                        .map(|label| ArgumentDefault::Value(label.to_owned()))
                }),
                TextRule::Trimmed,
            )],
            Self::ClosePane => vec![ArgumentStep::confirm(format!(
                "Close pane {}?",
                runtime.focused_pane().map_or("", |pane| match &pane.label {
                    Some(label) => label,
                    None => &pane.pane_id,
                })
            ))],
            Self::RunCommandInNewPane => vec![
                ArgumentStep::text(ArgumentKey::Command, false, None, TextRule::Visible),
                ArgumentStep::choice(
                    ArgumentKey::Direction,
                    vec![
                        ArgumentChoice::new("right", "Right"),
                        ArgumentChoice::new("down", "Down"),
                    ],
                    Some(ArgumentDefault::Value("right".into())),
                ),
            ],
            Self::SwitchAgent => vec![ArgumentStep::choice(
                ArgumentKey::Agent,
                runtime
                    .session
                    .agents
                    .iter()
                    .filter(|agent| !agent.focused)
                    .map(|agent| {
                        let label = match (agent.name.as_deref(), agent.display_agent.as_deref()) {
                            (Some(name), Some(display)) => format!("{name} ({display})"),
                            (Some(name), None) => name.to_owned(),
                            (None, Some(display)) => display.to_owned(),
                            (None, None) => agent.pane_id.clone(),
                        };
                        ArgumentChoice::new(&agent.pane_id, label)
                    })
                    .collect(),
                None,
            )],
            Self::RenameFocusedAgent => vec![ArgumentStep::text(
                ArgumentKey::Name,
                true,
                runtime.focused_agent().and_then(|agent| {
                    agent
                        .name
                        .as_deref()
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(|name| ArgumentDefault::Value(name.to_owned()))
                }),
                TextRule::Trimmed,
            )],
            Self::PromptFocusedAgent => vec![ArgumentStep::text(
                ArgumentKey::Prompt,
                false,
                None,
                TextRule::Visible,
            )],
            Self::StartAgent => vec![
                ArgumentStep::text(ArgumentKey::Kind, false, None, TextRule::Trimmed),
                ArgumentStep::text(
                    ArgumentKey::Name,
                    true,
                    Some(ArgumentDefault::From(ArgumentKey::Kind)),
                    TextRule::Trimmed,
                ),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::parse_invocation_context;
    use crate::model::{
        ApiCapabilities, SessionSnapshotResult, SuccessEnvelope, WorktreeListResult,
    };

    fn runtime(capable: bool) -> RuntimeSnapshot {
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
                typed_agent_start: capable,
                agent_prompt: capable,
            },
        }
    }

    fn signature(step: &ArgumentStep) -> (ArgumentKey, ArgumentKind, bool) {
        (step.key(), step.kind(), step.optional())
    }

    #[test]
    fn core_catalog_is_the_exact_twenty_four_item_contract() {
        let runtime = runtime(true);
        let expected = [
            (
                "workspace.switch",
                "Switch workspace",
                "Focus another workspace",
                CommandCategory::Workspace,
                &["workspace", "ws"][..],
                20,
                &[(ArgumentKey::Workspace, ArgumentKind::Choice, false)][..],
            ),
            (
                "workspace.create",
                "Create workspace",
                "Create and focus a workspace",
                CommandCategory::Workspace,
                &["new workspace"][..],
                30,
                &[
                    (ArgumentKey::Label, ArgumentKind::Text, false),
                    (ArgumentKey::Cwd, ArgumentKind::Text, true),
                ][..],
            ),
            (
                "workspace.rename",
                "Rename workspace",
                "Rename the current workspace",
                CommandCategory::Workspace,
                &["workspace name"][..],
                40,
                &[(ArgumentKey::Label, ArgumentKind::Text, false)][..],
            ),
            (
                "workspace.close",
                "Close workspace",
                "Close the current workspace",
                CommandCategory::Workspace,
                &["remove workspace"][..],
                90,
                &[(ArgumentKey::Confirm, ArgumentKind::Confirm, false)][..],
            ),
            (
                "worktree.open",
                "Open worktree",
                "Open an existing Git worktree",
                CommandCategory::Worktree,
                &["checkout", "branch workspace"][..],
                20,
                &[(ArgumentKey::Worktree, ArgumentKind::Choice, false)][..],
            ),
            (
                "worktree.create",
                "Create linked worktree",
                "Create and open a linked worktree",
                CommandCategory::Worktree,
                &["new worktree", "branch"][..],
                30,
                &[
                    (ArgumentKey::Branch, ArgumentKind::Text, false),
                    (ArgumentKey::Base, ArgumentKind::Text, true),
                    (ArgumentKey::Label, ArgumentKind::Text, true),
                ][..],
            ),
            (
                "tab.switch",
                "Switch tab",
                "Focus another tab",
                CommandCategory::Tab,
                &["tab", "goto tab"][..],
                20,
                &[(ArgumentKey::Tab, ArgumentKind::Choice, false)][..],
            ),
            (
                "tab.create",
                "Create tab",
                "Create and focus a tab",
                CommandCategory::Tab,
                &["new tab"][..],
                30,
                &[
                    (ArgumentKey::Label, ArgumentKind::Text, false),
                    (ArgumentKey::Cwd, ArgumentKind::Text, true),
                ][..],
            ),
            (
                "tab.rename",
                "Rename tab",
                "Rename the current tab",
                CommandCategory::Tab,
                &["tab name"][..],
                40,
                &[(ArgumentKey::Label, ArgumentKind::Text, false)][..],
            ),
            (
                "tab.close",
                "Close tab",
                "Close the current tab",
                CommandCategory::Tab,
                &["remove tab"][..],
                90,
                &[(ArgumentKey::Confirm, ArgumentKind::Confirm, false)][..],
            ),
            (
                "pane.focus-left",
                "Focus pane left",
                "Focus the pane to the left",
                CommandCategory::Pane,
                &["left pane"][..],
                10,
                &[][..],
            ),
            (
                "pane.focus-right",
                "Focus pane right",
                "Focus the pane to the right",
                CommandCategory::Pane,
                &["right pane"][..],
                10,
                &[][..],
            ),
            (
                "pane.focus-up",
                "Focus pane up",
                "Focus the pane above",
                CommandCategory::Pane,
                &["upper pane"][..],
                10,
                &[][..],
            ),
            (
                "pane.focus-down",
                "Focus pane down",
                "Focus the pane below",
                CommandCategory::Pane,
                &["lower pane"][..],
                10,
                &[][..],
            ),
            (
                "pane.split-right",
                "Split pane right",
                "Create a pane on the right",
                CommandCategory::Pane,
                &["new pane right"][..],
                30,
                &[(ArgumentKey::Cwd, ArgumentKind::Text, true)][..],
            ),
            (
                "pane.split-down",
                "Split pane down",
                "Create a pane below",
                CommandCategory::Pane,
                &["new pane below"][..],
                30,
                &[(ArgumentKey::Cwd, ArgumentKind::Text, true)][..],
            ),
            (
                "pane.zoom-toggle",
                "Toggle pane zoom",
                "Toggle zoom for the focused pane",
                CommandCategory::Pane,
                &["maximize pane", "unzoom"][..],
                10,
                &[][..],
            ),
            (
                "pane.rename",
                "Rename focused pane",
                "Rename the focused pane",
                CommandCategory::Pane,
                &["pane name", "rename current pane"][..],
                20,
                &[(ArgumentKey::Label, ArgumentKind::Text, true)][..],
            ),
            (
                "pane.close",
                "Close pane",
                "Close the focused pane",
                CommandCategory::Pane,
                &["remove pane"][..],
                90,
                &[(ArgumentKey::Confirm, ArgumentKind::Confirm, false)][..],
            ),
            (
                "pane.run",
                "Run command in new pane",
                "Split and run a shell command",
                CommandCategory::Pane,
                &["command", "terminal"][..],
                30,
                &[
                    (ArgumentKey::Command, ArgumentKind::Text, false),
                    (ArgumentKey::Direction, ArgumentKind::Choice, false),
                ][..],
            ),
            (
                "agent.switch",
                "Switch agent",
                "Focus another running agent",
                CommandCategory::Agent,
                &["agent", "goto agent"][..],
                10,
                &[(ArgumentKey::Agent, ArgumentKind::Choice, false)][..],
            ),
            (
                "agent.rename",
                "Rename focused agent",
                "Rename the agent in the focused pane",
                CommandCategory::Agent,
                &["agent name"][..],
                20,
                &[(ArgumentKey::Name, ArgumentKind::Text, true)][..],
            ),
            (
                "agent.prompt",
                "Prompt focused agent",
                "Send text to the focused agent",
                CommandCategory::Agent,
                &["ask agent", "send prompt"][..],
                10,
                &[(ArgumentKey::Prompt, ArgumentKind::Text, false)][..],
            ),
            (
                "agent.start",
                "Start agent",
                "Start an agent in the focused pane",
                CommandCategory::Agent,
                &["launch agent", "new agent"][..],
                40,
                &[
                    (ArgumentKey::Kind, ArgumentKind::Text, false),
                    (ArgumentKey::Name, ArgumentKind::Text, true),
                ][..],
            ),
        ];

        assert_eq!(CoreCommand::all().len(), expected.len());
        for (command, expected) in CoreCommand::all().iter().copied().zip(expected) {
            assert_eq!(command.stable_id(), expected.0);
            let spec = command.catalog_spec(&runtime);
            assert_eq!(spec.title, expected.1);
            assert_eq!(spec.description.as_deref(), Some(expected.2));
            assert_eq!(spec.category, expected.3);
            assert_eq!(
                spec.aliases.iter().map(String::as_str).collect::<Vec<_>>(),
                expected.4
            );
            assert_eq!(spec.priority, expected.5);
            assert_eq!(
                spec.steps.iter().map(signature).collect::<Vec<_>>(),
                expected.6
            );
        }
    }

    #[test]
    fn dynamic_steps_contain_exact_choices_prompts_and_defaults() {
        let runtime = runtime(true);
        let switch_workspace = CoreCommand::SwitchWorkspace.spec(&runtime).unwrap();
        assert_eq!(
            switch_workspace.steps[0].choices(),
            &[ArgumentChoice::new("ws-2", "Docs")]
        );
        let create_workspace = CoreCommand::CreateWorkspace.spec(&runtime).unwrap();
        assert_eq!(
            create_workspace.steps[1].default(),
            Some(&ArgumentDefault::Value("/invoked/cwd".into()))
        );
        let rename_workspace = CoreCommand::RenameWorkspace.spec(&runtime).unwrap();
        assert_eq!(
            rename_workspace.steps[0].default(),
            Some(&ArgumentDefault::Value("Palette".into()))
        );
        assert_eq!(
            CoreCommand::CloseWorkspace.spec(&runtime).unwrap().steps[0].prompt(),
            Some("Close workspace Palette?")
        );
        assert_eq!(
            CoreCommand::OpenWorktree.spec(&runtime).unwrap().steps[0].choices(),
            &[ArgumentChoice::new("/repo/feature", "feature/palette")]
        );
        assert_eq!(
            CoreCommand::CreateWorktree.spec(&runtime).unwrap().steps[1].default(),
            Some(&ArgumentDefault::Value("HEAD".into()))
        );
        assert_eq!(
            CoreCommand::CreateTab.spec(&runtime).unwrap().steps[1].default(),
            Some(&ArgumentDefault::Value("/repo/main".into()))
        );
        assert_eq!(
            CoreCommand::RenameTab.spec(&runtime).unwrap().steps[0].default(),
            Some(&ArgumentDefault::Value("Code".into()))
        );
        assert_eq!(
            CoreCommand::SplitPane(SplitDirection::Right)
                .spec(&runtime)
                .unwrap()
                .steps[0]
                .default(),
            Some(&ArgumentDefault::Value("/repo/main".into()))
        );
        assert_eq!(
            CoreCommand::ClosePane.spec(&runtime).unwrap().steps[0].prompt(),
            Some("Close pane agent?")
        );
        assert_eq!(
            CoreCommand::RenameFocusedPane.spec(&runtime).unwrap().steps[0].default(),
            Some(&ArgumentDefault::Value("agent".into()))
        );
        let run = CoreCommand::RunCommandInNewPane.spec(&runtime).unwrap();
        assert_eq!(
            run.steps[1].choices(),
            &[
                ArgumentChoice::new("right", "Right"),
                ArgumentChoice::new("down", "Down")
            ]
        );
        assert_eq!(
            run.steps[1].default(),
            Some(&ArgumentDefault::Value("right".into()))
        );
        assert_eq!(
            CoreCommand::SwitchAgent.spec(&runtime).unwrap().steps[0].choices(),
            &[ArgumentChoice::new("pane-d", "builder (Codex)")]
        );
        let rename_agent = CoreCommand::RenameFocusedAgent.spec(&runtime).unwrap();
        assert_eq!(
            rename_agent.steps[0].default(),
            Some(&ArgumentDefault::Value("reviewer".into()))
        );
        assert_eq!(
            CoreCommand::StartAgent
                .spec(&runtime)
                .unwrap_or_else(|| {
                    let mut start_runtime = runtime.clone();
                    start_runtime
                        .session
                        .agents
                        .retain(|agent| agent.pane_id != "pane-a");
                    CoreCommand::StartAgent.spec(&start_runtime).unwrap()
                })
                .steps[1]
                .default(),
            Some(&ArgumentDefault::From(ArgumentKey::Kind))
        );
    }

    #[test]
    fn rename_focused_pane_requires_a_pane_and_uses_only_its_custom_label() {
        let runtime = runtime(true);
        let rename = CoreCommand::RenameFocusedPane.spec(&runtime).unwrap();
        assert_eq!(rename.steps[0].key(), ArgumentKey::Label);
        assert!(rename.steps[0].optional());
        assert_eq!(
            rename.steps[0].default(),
            Some(&ArgumentDefault::Value("agent".into()))
        );

        let mut unlabeled = runtime.clone();
        let focused = unlabeled
            .session
            .panes
            .iter_mut()
            .find(|pane| pane.pane_id == "pane-a")
            .unwrap();
        focused.label = None;
        focused.terminal_title = Some("shell title".into());
        focused.display_agent = Some("Claude".into());
        assert_eq!(
            CoreCommand::RenameFocusedPane
                .spec(&unlabeled)
                .unwrap()
                .steps[0]
                .default(),
            None
        );

        let mut absent = runtime;
        absent.session.focused_pane_id = None;
        assert!(CoreCommand::RenameFocusedPane.spec(&absent).is_none());
    }

    #[test]
    fn rename_focused_agent_requires_an_agent_record_and_uses_only_custom_name() {
        let runtime = runtime(true);
        let rename = CoreCommand::RenameFocusedAgent.spec(&runtime).unwrap();
        assert_eq!(rename.steps[0].key(), ArgumentKey::Name);
        assert!(rename.steps[0].optional());
        assert_eq!(
            rename.steps[0].default(),
            Some(&ArgumentDefault::Value("reviewer".into()))
        );

        let mut unnamed = runtime.clone();
        let focused = unnamed
            .session
            .agents
            .iter_mut()
            .find(|agent| agent.pane_id == "pane-a")
            .unwrap();
        focused.name = None;
        focused.display_agent = Some("Claude".into());
        assert_eq!(
            CoreCommand::RenameFocusedAgent
                .spec(&unnamed)
                .unwrap()
                .steps[0]
                .default(),
            None
        );

        let mut done = runtime.clone();
        done.session
            .agents
            .iter_mut()
            .find(|agent| agent.pane_id == "pane-a")
            .unwrap()
            .agent_status = crate::model::AgentStatus::Done;
        assert!(CoreCommand::RenameFocusedAgent.spec(&done).is_some());

        let mut absent = runtime;
        absent
            .session
            .agents
            .retain(|agent| agent.pane_id != "pane-a");
        assert!(CoreCommand::RenameFocusedAgent.spec(&absent).is_none());
    }

    #[test]
    fn switch_workspace_is_available_without_focused_workspace_metadata() {
        let mut runtime = runtime(true);
        runtime.session.focused_workspace_id = None;

        let switch_workspace = CoreCommand::SwitchWorkspace.spec(&runtime).unwrap();
        assert_eq!(
            switch_workspace.steps[0].choices(),
            &[
                ArgumentChoice::new("ws-1", "Palette"),
                ArgumentChoice::new("ws-2", "Docs"),
            ]
        );
    }

    #[test]
    fn switch_tab_uses_the_workspace_active_tab_when_focused_tab_metadata_is_absent() {
        let mut runtime = runtime(true);
        runtime.session.focused_tab_id = None;
        runtime.session.workspaces[0].tab_count = 2;
        runtime.session.tabs.push(crate::model::TabInfo {
            tab_id: "tab-extra".into(),
            workspace_id: "ws-1".into(),
            number: 2,
            label: "Extra".into(),
            focused: false,
            pane_count: 1,
            agent_status: crate::model::AgentStatus::Idle,
        });

        let switch_tab = CoreCommand::SwitchTab.spec(&runtime).unwrap();
        assert_eq!(
            switch_tab.steps[0].choices(),
            &[ArgumentChoice::new("tab-extra", "Extra")]
        );
    }

    #[test]
    fn defaults_are_exhaustive_for_every_catalog_identity() {
        let runtime = runtime(true);
        let expected = vec![
            vec![None],
            vec![None, Some(ArgumentDefault::Value("/invoked/cwd".into()))],
            vec![Some(ArgumentDefault::Value("Palette".into()))],
            vec![None],
            vec![None],
            vec![None, Some(ArgumentDefault::Value("HEAD".into())), None],
            vec![None],
            vec![None, Some(ArgumentDefault::Value("/repo/main".into()))],
            vec![Some(ArgumentDefault::Value("Code".into()))],
            vec![None],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![Some(ArgumentDefault::Value("/repo/main".into()))],
            vec![Some(ArgumentDefault::Value("/repo/main".into()))],
            vec![],
            vec![Some(ArgumentDefault::Value("agent".into()))],
            vec![None],
            vec![None, Some(ArgumentDefault::Value("right".into()))],
            vec![None],
            vec![Some(ArgumentDefault::Value("reviewer".into()))],
            vec![None],
            vec![None, Some(ArgumentDefault::From(ArgumentKey::Kind))],
        ];
        for (command, expected_defaults) in CoreCommand::all().iter().copied().zip(expected) {
            let spec = command.catalog_spec(&runtime);
            let defaults = spec
                .steps
                .iter()
                .map(|step| step.default().cloned())
                .collect::<Vec<_>>();
            assert_eq!(defaults, expected_defaults, "{}", command.stable_id());
        }
    }

    #[test]
    fn every_runtime_availability_rule_is_applied() {
        let base = runtime(true);
        let available = |command: CoreCommand| command.spec(&base).is_some();
        assert_eq!(
            CoreCommand::all()
                .iter()
                .copied()
                .map(available)
                .collect::<Vec<_>>(),
            vec![
                true, true, true, true, true, true, false, true, true, false, false, true, false,
                true, true, true, true, true, true, true, true, true, true, false,
            ]
        );

        let mut old = base.clone();
        old.capabilities = ApiCapabilities {
            typed_agent_start: false,
            agent_prompt: false,
        };
        assert!(CoreCommand::PromptFocusedAgent.spec(&old).is_none());
        assert!(CoreCommand::StartAgent.spec(&old).is_none());
        assert!(CoreCommand::PromptFocusedAgent.spec(&base).is_some());

        let mut no_agent = base.clone();
        no_agent
            .session
            .agents
            .retain(|agent| agent.pane_id != "pane-a");
        assert!(CoreCommand::PromptFocusedAgent.spec(&no_agent).is_none());
        assert!(CoreCommand::StartAgent.spec(&no_agent).is_some());
        no_agent.session.agents.clear();
        assert!(CoreCommand::SwitchAgent.spec(&no_agent).is_none());

        let mut no_provenance = base.clone();
        no_provenance.session.workspaces[0].worktree = None;
        assert!(CoreCommand::CreateWorktree.spec(&no_provenance).is_none());

        let mut one_pane = base.clone();
        one_pane
            .session
            .panes
            .retain(|pane| pane.tab_id != "tab-1" || pane.pane_id == "pane-a");
        assert!(CoreCommand::ClosePane.spec(&one_pane).is_none());

        let mut sparse = base.clone();
        sparse.session.workspaces.truncate(1);
        sparse.worktrees.clear();
        sparse.session.tabs.push(crate::model::TabInfo {
            tab_id: "tab-extra".into(),
            workspace_id: "ws-1".into(),
            number: 2,
            label: "Extra".into(),
            focused: false,
            pane_count: 1,
            agent_status: crate::model::AgentStatus::Idle,
        });
        assert!(CoreCommand::SwitchWorkspace.spec(&sparse).is_none());
        assert!(CoreCommand::CloseWorkspace.spec(&sparse).is_none());
        assert!(CoreCommand::OpenWorktree.spec(&sparse).is_none());
        assert!(CoreCommand::SwitchTab.spec(&sparse).is_some());
        assert!(CoreCommand::CloseTab.spec(&sparse).is_some());

        sparse.session.focused_workspace_id = None;
        sparse.session.focused_tab_id = None;
        sparse.session.focused_pane_id = None;
        assert!(CoreCommand::CreateWorkspace.spec(&sparse).is_some());
        for command in [
            CoreCommand::RenameWorkspace,
            CoreCommand::CreateWorktree,
            CoreCommand::CreateTab,
            CoreCommand::RenameTab,
            CoreCommand::SplitPane(SplitDirection::Right),
            CoreCommand::TogglePaneZoom,
            CoreCommand::RenameFocusedPane,
            CoreCommand::ClosePane,
            CoreCommand::RunCommandInNewPane,
        ] {
            assert!(command.spec(&sparse).is_none(), "{}", command.stable_id());
        }
    }

    #[test]
    fn text_rules_trim_identifiers_but_preserve_visible_prompt_and_command_text() {
        let runtime = runtime(true);
        let label = &CoreCommand::CreateWorkspace.spec(&runtime).unwrap().steps[0];
        assert_eq!(
            label.normalize_text("  name  ").unwrap(),
            Some("name".into())
        );
        assert!(label.normalize_text(" \t ").is_err());
        let cwd = &CoreCommand::CreateWorkspace.spec(&runtime).unwrap().steps[1];
        assert_eq!(cwd.normalize_text("  ").unwrap(), None);
        let rename_agent = &CoreCommand::RenameFocusedAgent
            .spec(&runtime)
            .unwrap()
            .steps[0];
        assert_eq!(
            rename_agent.normalize_text("  reviewer  ").unwrap(),
            Some("reviewer".into())
        );
        assert_eq!(rename_agent.normalize_text(" \t ").unwrap(), None);
        let rename_pane = &CoreCommand::RenameFocusedPane.spec(&runtime).unwrap().steps[0];
        assert_eq!(
            rename_pane.normalize_text("  build logs  ").unwrap(),
            Some("build logs".into())
        );
        assert_eq!(rename_pane.normalize_text(" \t ").unwrap(), None);
        let command = &CoreCommand::RunCommandInNewPane
            .spec(&runtime)
            .unwrap()
            .steps[0];
        assert_eq!(
            command.normalize_text("  echo hi  ").unwrap(),
            Some("  echo hi  ".into())
        );
        assert!(command.normalize_text(" \n\t ").is_err());
    }
}
