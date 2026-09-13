use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use herdr_command_palette::app::{AppError, EventSource, PaletteApp};
use herdr_command_palette::client::{HerdrClient, HerdrOperation, OperationResponse};
use herdr_command_palette::command::{ArgumentKey, CommandId, CoreCommand};
use herdr_command_palette::context::parse_invocation_context;
use herdr_command_palette::error::ClientError;
use herdr_command_palette::model::{
    capabilities_from_schema, ApiCapabilities, InstalledPluginInfo, PluginActionContext,
    PluginActionInfo, PluginActionListResult, PluginListResult, SessionSnapshot,
    SessionSnapshotResult, SuccessEnvelope, WorktreeInfo, WorktreeListResult,
};
use herdr_command_palette::registry::PaletteItemId;
use herdr_command_palette::state::LoadingState;
use ratatui::{backend::TestBackend, layout::Rect, Terminal};

fn fixture<T: serde::de::DeserializeOwned>(name: &str) -> T {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn session_snapshot() -> SessionSnapshot {
    let session: SuccessEnvelope<SessionSnapshotResult> = fixture("session-snapshot.json");
    session.result.snapshot
}

fn worktrees() -> Vec<WorktreeInfo> {
    let worktrees: SuccessEnvelope<WorktreeListResult> = fixture("worktree-list.json");
    worktrees.result.worktrees
}

fn invocation() -> herdr_command_palette::model::PluginInvocationContext {
    parse_invocation_context(include_str!("fixtures/plugin-invocation-context.json")).unwrap()
}

fn plugin_inputs() -> (Vec<InstalledPluginInfo>, Vec<PluginActionInfo>) {
    let plugins: SuccessEnvelope<PluginListResult> = fixture("plugin-list.json");
    let actions: SuccessEnvelope<PluginActionListResult> = fixture("plugin-actions.json");
    (plugins.result.plugins, actions.result.actions)
}

#[derive(Debug, Clone)]
enum Dispatch {
    Empty,
    StaleWorkspace,
    StaleAgent,
    Failure(&'static str),
}

#[derive(Default)]
struct Trace {
    calls: Vec<&'static str>,
    operations: Vec<HerdrOperation>,
}

struct FakeClient {
    trace: Rc<RefCell<Trace>>,
    snapshot: SessionSnapshot,
    refreshed_snapshot: Option<SessionSnapshot>,
    worktrees: Vec<WorktreeInfo>,
    plugins: Vec<InstalledPluginInfo>,
    actions: Vec<PluginActionInfo>,
    capabilities: ApiCapabilities,
    dispatches: VecDeque<Dispatch>,
}

impl FakeClient {
    fn new(
        capabilities: ApiCapabilities,
        dispatches: impl IntoIterator<Item = Dispatch>,
    ) -> (Self, Rc<RefCell<Trace>>) {
        let snapshot = session_snapshot();
        let worktrees = worktrees();
        let (plugins, actions) = plugin_inputs();
        let trace = Rc::new(RefCell::new(Trace::default()));
        (
            Self {
                trace: Rc::clone(&trace),
                snapshot,
                refreshed_snapshot: None,
                worktrees,
                plugins,
                actions,
                capabilities,
                dispatches: dispatches.into_iter().collect(),
            },
            trace,
        )
    }

    fn call(&self, call: &'static str) {
        self.trace.borrow_mut().calls.push(call);
    }
}

impl HerdrClient for FakeClient {
    fn api_capabilities(&mut self) -> Result<ApiCapabilities, ClientError> {
        self.call("capabilities");
        Ok(self.capabilities)
    }

    fn session_snapshot(&mut self) -> Result<SessionSnapshot, ClientError> {
        self.call("snapshot");
        if self
            .trace
            .borrow()
            .calls
            .iter()
            .filter(|call| **call == "snapshot")
            .count()
            > 1
        {
            return Ok(self
                .refreshed_snapshot
                .clone()
                .unwrap_or_else(|| self.snapshot.clone()));
        }
        Ok(self.snapshot.clone())
    }

    fn worktree_list(&mut self, _: &str) -> Result<Vec<WorktreeInfo>, ClientError> {
        self.call("worktrees");
        Ok(self.worktrees.clone())
    }

    fn plugin_list(&mut self) -> Result<Vec<InstalledPluginInfo>, ClientError> {
        self.call("plugins");
        Ok(self.plugins.clone())
    }

    fn plugin_action_list(&mut self) -> Result<Vec<PluginActionInfo>, ClientError> {
        self.call("actions");
        Ok(self.actions.clone())
    }

    fn open_palette(&mut self) -> Result<(), ClientError> {
        Ok(())
    }

    fn dispatch(&mut self, operation: &HerdrOperation) -> Result<OperationResponse, ClientError> {
        self.trace.borrow_mut().operations.push(operation.clone());
        match self.dispatches.pop_front().unwrap_or(Dispatch::Empty) {
            Dispatch::Empty => Ok(OperationResponse::Empty),
            Dispatch::StaleWorkspace => Err(ClientError::Api {
                code: "workspace_not_found".into(),
                message: "stale workspace".into(),
            }),
            Dispatch::StaleAgent => Err(ClientError::Api {
                code: "agent_not_found".into(),
                message: "stale agent".into(),
            }),
            Dispatch::Failure(message) => Err(ClientError::Process {
                message: message.into(),
            }),
        }
    }
}

struct Events(VecDeque<Event>);

impl EventSource for Events {
    fn next_event(&mut self) -> Result<Option<Event>, AppError> {
        self.0.pop_front().map(Some).ok_or_else(|| {
            AppError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "scripted event input exhausted",
            ))
        })
    }
}

#[test]
fn events_fail_fast_when_scripted_input_is_exhausted() {
    let error = Events(VecDeque::new())
        .next_event()
        .expect_err("exhausted scripted input should fail");

    assert!(matches!(
        error,
        AppError::Io(error)
            if error.kind() == std::io::ErrorKind::UnexpectedEof
                && error.to_string() == "scripted event input exhausted"
    ));
}

fn key(character: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
}

fn enter() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
}

fn esc() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
}

fn backspace() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE))
}

fn events(text: &str) -> Vec<Event> {
    text.chars().map(key).collect()
}

fn replace_prefill(current: &str, replacement: &str) -> Vec<Event> {
    std::iter::repeat_with(backspace)
        .take(current.chars().count())
        .chain(events(replacement))
        .collect()
}

fn run_app(
    client: FakeClient,
    events: impl IntoIterator<Item = Event>,
    observe: impl FnMut(
        &herdr_command_palette::state::PaletteState,
        &herdr_command_palette::registry::PaletteCatalog,
        Option<&herdr_command_palette::model::RuntimeSnapshot>,
    ),
) {
    run_app_with_invocation(client, invocation(), events, observe);
}

fn run_app_with_invocation(
    client: FakeClient,
    invocation: herdr_command_palette::model::PluginInvocationContext,
    events: impl IntoIterator<Item = Event>,
    mut observe: impl FnMut(
        &herdr_command_palette::state::PaletteState,
        &herdr_command_palette::registry::PaletteCatalog,
        Option<&herdr_command_palette::model::RuntimeSnapshot>,
    ),
) {
    let mut app = PaletteApp::new(client, invocation);
    let mut events = Events(events.into_iter().collect());
    app.run_with(&mut events, |state, registry, runtime| {
        observe(state, registry, runtime);
        Ok(Rect::new(0, 0, 100, 30))
    })
    .unwrap();
}

const CAPABLE: ApiCapabilities = ApiCapabilities {
    typed_agent_start: true,
    agent_prompt: true,
};

#[test]
fn immediate_core_execution_and_plugin_invocation_cross_the_palette_boundary() {
    let (client, trace) = FakeClient::new(CAPABLE, [Dispatch::Empty]);
    let mut input = events("zoom");
    input.push(enter());
    run_app(client, input, |_, _, _| {});
    assert_eq!(
        trace.borrow().operations,
        [HerdrOperation::PaneZoomToggle {
            pane_id: "pane-a".into()
        }]
    );

    let (client, trace) = FakeClient::new(CAPABLE, [Dispatch::Empty]);
    let mut input = events("run demo");
    input.push(enter());
    run_app(client, input, |_, _, _| {});
    assert_eq!(
        trace.borrow().operations,
        [HerdrOperation::PluginActionInvoke {
            action_id: "demo.tools.run".into()
        }]
    );
}

#[test]
fn live_workspace_local_tab_remote_tab_and_global_agent_focus_cross_the_palette_boundary() {
    let (client, trace) = FakeClient::new(CAPABLE, [Dispatch::Empty]);
    let mut input = events("docs");
    input.push(enter());
    run_app(client, input, |_, _, _| {});
    assert_eq!(
        trace.borrow().operations,
        [HerdrOperation::WorkspaceFocus {
            workspace_id: "ws-2".into(),
        }]
    );

    let (client, trace) = FakeClient::new(CAPABLE, [Dispatch::Empty]);
    let mut input = events("code");
    input.push(enter());
    run_app(client, input, |_, _, _| {});
    assert_eq!(
        trace.borrow().operations,
        [HerdrOperation::TabFocus {
            tab_id: "tab-1".into(),
        }]
    );

    let (client, trace) = FakeClient::new(CAPABLE, [Dispatch::Empty]);
    let mut input = events("tab: notes");
    input.push(enter());
    run_app(client, input, |_, _, _| {});
    assert_eq!(
        trace.borrow().operations,
        [HerdrOperation::TabFocus {
            tab_id: "tab-2".into(),
        }]
    );

    let (client, trace) = FakeClient::new(CAPABLE, [Dispatch::Empty]);
    let mut input = events("builder");
    input.push(enter());
    run_app(client, input, |_, _, _| {});
    assert_eq!(
        trace.borrow().operations,
        [HerdrOperation::AgentFocus {
            pane_id: "pane-d".into(),
        }]
    );
}

#[test]
fn stale_duplicate_label_replacement_never_retargets_and_keeps_palette_open() {
    let (mut client, trace) = FakeClient::new(CAPABLE, [Dispatch::StaleAgent]);
    let mut refreshed = client.snapshot.clone();
    let mut replacement_agent = refreshed
        .agents
        .iter()
        .find(|agent| agent.pane_id == "pane-a")
        .cloned()
        .unwrap();
    let mut replacement_pane = refreshed
        .panes
        .iter()
        .find(|pane| pane.pane_id == "pane-a")
        .cloned()
        .unwrap();
    refreshed.agents.retain(|agent| agent.pane_id != "pane-a");
    refreshed.panes.retain(|pane| pane.pane_id != "pane-a");
    replacement_agent.pane_id = "pane-replacement".into();
    replacement_pane.pane_id = "pane-replacement".into();
    refreshed.agents.push(replacement_agent);
    refreshed.panes.push(replacement_pane);
    client.refreshed_snapshot = Some(refreshed);

    let mut input = events("reviewer");
    input.push(enter());
    input.push(esc());
    let mut saw_replacement = false;
    run_app(client, input, |state, catalog, runtime| {
        if runtime.is_some_and(|runtime| {
            runtime
                .session
                .agents
                .iter()
                .any(|agent| agent.pane_id == "pane-replacement")
        }) {
            saw_replacement = true;
            assert_eq!(state.query.text(), "reviewer");
            assert!(catalog
                .get(&PaletteItemId::Agent("pane-a".into()))
                .is_none());
            assert!(catalog
                .get(&PaletteItemId::Agent("pane-replacement".into()))
                .is_some());
        }
    });

    assert!(saw_replacement);
    let trace = trace.borrow();
    assert_eq!(
        trace.operations,
        [HerdrOperation::AgentFocus {
            pane_id: "pane-a".into(),
        }]
    );
    assert_eq!(
        trace.calls,
        [
            "capabilities",
            "snapshot",
            "worktrees",
            "plugins",
            "actions",
            "snapshot",
            "worktrees",
        ]
    );
}

#[test]
fn text_and_choice_forms_submit_typed_operations() {
    let (client, trace) = FakeClient::new(CAPABLE, [Dispatch::Empty]);
    let mut input = events("create workspace");
    input.push(enter());
    input.extend(events("Flow workspace"));
    input.push(enter());
    input.push(enter());
    run_app(client, input, |_, _, _| {});
    assert_eq!(
        trace.borrow().operations,
        [HerdrOperation::WorkspaceCreate {
            label: "Flow workspace".into(),
            cwd: Some("/invoked/cwd".into()),
        }]
    );

    let (client, trace) = FakeClient::new(CAPABLE, [Dispatch::Empty]);
    let mut input = events("switch workspace");
    input.push(enter());
    input.push(enter());
    input.push(esc());
    input.push(esc());
    run_app(client, input, |_, _, _| {});
    assert_eq!(
        trace.borrow().operations,
        [HerdrOperation::WorkspaceFocus {
            workspace_id: "ws-2".into()
        }]
    );
}

#[test]
fn validation_and_command_failure_keep_the_palette_state_visible() {
    let (client, trace) = FakeClient::new(CAPABLE, []);
    let mut saw_validation = false;
    let mut input = events("prompt focused agent");
    input.push(enter());
    input.extend(events("   "));
    input.push(enter());
    input.push(esc());
    input.push(esc());
    run_app(client, input, |state, _, _| {
        saw_validation |= state.error.as_deref() == Some("a value is required")
            && state.active_text().is_some_and(|text| text.text() == "   ");
    });
    assert!(saw_validation);
    assert!(trace.borrow().operations.is_empty());

    let (client, trace) = FakeClient::new(CAPABLE, [Dispatch::Failure("command failed")]);
    let mut saw_failure = false;
    let mut input = events("create workspace");
    input.push(enter());
    input.extend(events("Failed workspace"));
    input.push(enter());
    input.extend(std::iter::repeat_with(backspace).take("/invoked/cwd".len()));
    input.extend(events("/tmp/failed-workspace"));
    input.push(enter());
    input.push(esc());
    input.push(esc());
    input.push(esc());
    run_app(client, input, |state, _, _| {
        saw_failure |= state.error.as_deref() == Some("failed while running Herdr: command failed")
            && state
                .active_text()
                .is_some_and(|text| text.text() == "/tmp/failed-workspace")
            && state.completed_arguments().text(ArgumentKey::Label) == Some("Failed workspace")
            && state.completed_arguments().text(ArgumentKey::Cwd) == Some("/tmp/failed-workspace")
            && matches!(state.loading, LoadingState::Ready);
    });
    assert!(saw_failure);
    assert_eq!(
        trace.borrow().operations,
        [HerdrOperation::WorkspaceCreate {
            label: "Failed workspace".into(),
            cwd: Some("/tmp/failed-workspace".into()),
        }]
    );
}

#[test]
fn rename_focused_pane_form_prefills_renames_and_clears() {
    let (client, trace) = FakeClient::new(CAPABLE, [Dispatch::Empty]);
    let mut input = events("rename focused pane");
    input.push(enter());
    input.extend(replace_prefill("agent", "build logs"));
    input.push(enter());
    let mut saw_prefill = false;
    run_app(client, input, |state, _, _| {
        saw_prefill |= state
            .active_text()
            .is_some_and(|text| text.text() == "agent");
    });
    assert!(saw_prefill);
    assert_eq!(
        trace.borrow().operations,
        [HerdrOperation::PaneRename {
            pane_id: "pane-a".into(),
            label: Some("build logs".into()),
        }]
    );

    let (client, trace) = FakeClient::new(CAPABLE, [Dispatch::Empty]);
    let mut clear = events("pane name");
    clear.push(enter());
    clear.extend(replace_prefill("agent", ""));
    clear.push(enter());
    run_app(client, clear, |_, _, _| {});
    assert_eq!(
        trace.borrow().operations,
        [HerdrOperation::PaneRename {
            pane_id: "pane-a".into(),
            label: None,
        }]
    );
}

#[test]
fn rename_focused_pane_failure_retains_submitted_label_and_visible_error() {
    let (client, trace) = FakeClient::new(CAPABLE, [Dispatch::Failure("rename failed")]);
    let mut input = events("rename focused pane");
    input.push(enter());
    input.extend(replace_prefill("agent", "build logs"));
    input.push(enter());
    input.push(esc());
    input.push(esc());
    let mut saw_failure = false;
    run_app(client, input, |state, catalog, runtime| {
        if state.error.as_deref() == Some("failed while running Herdr: rename failed")
            && state
                .active_text()
                .is_some_and(|text| text.text() == "build logs")
            && matches!(state.loading, LoadingState::Ready)
        {
            let backend = TestBackend::new(36, 12);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| herdr_command_palette::view::render(frame, state, catalog, runtime))
                .unwrap();
            let rendered = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(
                rendered.contains("✗ failed while running Herdr: ren…"),
                "{rendered:?}"
            );
            saw_failure = true;
        }
    });
    assert!(saw_failure);
    assert_eq!(
        trace.borrow().operations,
        [HerdrOperation::PaneRename {
            pane_id: "pane-a".into(),
            label: Some("build logs".into()),
        }]
    );
}

#[test]
fn rename_focused_agent_form_prefills_renames_and_clears() {
    let (client, trace) = FakeClient::new(CAPABLE, [Dispatch::Empty]);
    let mut input = events("rename focused agent");
    input.push(enter());
    input.extend(replace_prefill("reviewer", "review pair #2"));
    input.push(enter());
    let mut saw_prefill = false;
    run_app(client, input, |state, _, _| {
        saw_prefill |= state
            .active_text()
            .is_some_and(|text| text.text() == "reviewer");
    });
    assert!(saw_prefill);
    assert_eq!(
        trace.borrow().operations,
        [HerdrOperation::AgentRename {
            pane_id: "pane-a".into(),
            name: Some("review pair #2".into()),
        }]
    );

    let (client, trace) = FakeClient::new(CAPABLE, [Dispatch::Empty]);
    let mut clear = events("agent name");
    clear.push(enter());
    clear.extend(replace_prefill("reviewer", ""));
    clear.push(enter());
    run_app(client, clear, |_, _, _| {});
    assert_eq!(
        trace.borrow().operations,
        [HerdrOperation::AgentRename {
            pane_id: "pane-a".into(),
            name: None,
        }]
    );
}

#[test]
fn rename_focused_agent_failure_retains_submitted_name_and_visible_error() {
    let (client, trace) = FakeClient::new(CAPABLE, [Dispatch::Failure("rename failed")]);
    let mut input = events("rename focused agent");
    input.push(enter());
    input.extend(replace_prefill("reviewer", "review pair #2"));
    input.push(enter());
    input.push(esc());
    input.push(esc());
    let mut saw_failure = false;
    run_app(client, input, |state, catalog, runtime| {
        if state.error.as_deref() == Some("failed while running Herdr: rename failed")
            && state
                .active_text()
                .is_some_and(|text| text.text() == "review pair #2")
            && matches!(state.loading, LoadingState::Ready)
        {
            let backend = TestBackend::new(36, 12);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| herdr_command_palette::view::render(frame, state, catalog, runtime))
                .unwrap();
            let rendered = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(
                rendered.contains("✗ failed while running Herdr: ren…"),
                "{rendered:?}"
            );
            saw_failure = true;
        }
    });
    assert!(saw_failure);
    assert_eq!(
        trace.borrow().operations,
        [HerdrOperation::AgentRename {
            pane_id: "pane-a".into(),
            name: Some("review pair #2".into()),
        }]
    );
}

#[test]
fn safe_stale_retry_refreshes_then_retries_the_same_live_target() {
    let (client, trace) = FakeClient::new(CAPABLE, [Dispatch::StaleWorkspace, Dispatch::Empty]);
    let mut input = events("switch workspace");
    input.push(enter());
    input.push(enter());
    input.push(esc());
    input.push(esc());
    run_app(client, input, |_, _, _| {});

    let trace = trace.borrow();
    assert_eq!(
        trace.operations,
        [
            HerdrOperation::WorkspaceFocus {
                workspace_id: "ws-2".into()
            },
            HerdrOperation::WorkspaceFocus {
                workspace_id: "ws-2".into()
            },
        ]
    );
    assert_eq!(
        trace.calls,
        [
            "capabilities",
            "snapshot",
            "worktrees",
            "plugins",
            "actions",
            "capabilities",
            "snapshot",
            "worktrees",
        ]
    );
}

#[test]
fn bootstrap_excludes_self_action_and_exposes_any_context_action_for_workspace_or_selection() {
    let (mut client, _) = FakeClient::new(CAPABLE, []);
    client.actions.push(PluginActionInfo {
        plugin_id: "demo.tools".into(),
        action_id: "workspace-or-selection".into(),
        title: "Workspace or selection".into(),
        description: None,
        contexts: vec![
            PluginActionContext::Workspace,
            PluginActionContext::Selection,
        ],
        command: vec!["demo".into()],
        platforms: None,
    });
    let mut workspace_invocation = invocation();
    workspace_invocation.selected_text = None;
    let mut saw_workspace_context = false;
    run_app_with_invocation(
        client,
        workspace_invocation,
        [esc()],
        |_, registry, runtime| {
            let Some(_) = runtime else {
                return;
            };
            saw_workspace_context = registry
                .get(&PaletteItemId::Command(CommandId::Plugin(
                    "herdr.command-palette.open".into(),
                )))
                .is_none()
                && registry
                    .get(&PaletteItemId::Command(CommandId::Plugin(
                        "demo.tools.workspace-or-selection".into(),
                    )))
                    .is_some();
        },
    );
    assert!(saw_workspace_context);

    let (mut client, _) = FakeClient::new(CAPABLE, []);
    client.snapshot.focused_workspace_id = None;
    client.snapshot.focused_tab_id = None;
    client.snapshot.focused_pane_id = None;
    client.actions.push(PluginActionInfo {
        plugin_id: "demo.tools".into(),
        action_id: "workspace-or-selection".into(),
        title: "Workspace or selection".into(),
        description: None,
        contexts: vec![
            PluginActionContext::Workspace,
            PluginActionContext::Selection,
        ],
        command: vec!["demo".into()],
        platforms: None,
    });
    let mut selection_invocation = invocation();
    selection_invocation.selected_text = Some("selected text".into());
    let mut saw_selection_context = false;
    run_app_with_invocation(
        client,
        selection_invocation,
        [esc()],
        |_, registry, runtime| {
            if runtime.is_some() {
                saw_selection_context = registry
                    .get(&PaletteItemId::Command(CommandId::Plugin(
                        "demo.tools.workspace-or-selection".into(),
                    )))
                    .is_some();
            }
        },
    );
    assert!(saw_selection_context);
}

#[test]
fn bootstrap_schema_capabilities_gate_agent_commands_for_live_focused_pane() {
    let old_schema: serde_json::Value = fixture("api-schema-0.7.4.json");
    let new_schema: serde_json::Value = fixture("api-schema-capable.json");
    let old = capabilities_from_schema(&old_schema).unwrap();
    let new = capabilities_from_schema(&new_schema).unwrap();

    let (mut client, _) = FakeClient::new(old, []);
    client
        .snapshot
        .agents
        .retain(|agent| agent.pane_id != "pane-a");
    let mut saw_old_schema_commands = false;
    run_app(client, [esc()], |_, registry, runtime| {
        if runtime.is_some() {
            saw_old_schema_commands = registry
                .get(&PaletteItemId::Command(CommandId::Core(
                    CoreCommand::PromptFocusedAgent,
                )))
                .is_none()
                && registry
                    .get(&PaletteItemId::Command(CommandId::Core(
                        CoreCommand::StartAgent,
                    )))
                    .is_none()
                && registry
                    .get(&PaletteItemId::Command(CommandId::Core(
                        CoreCommand::TogglePaneZoom,
                    )))
                    .is_some()
                && registry
                    .get(&PaletteItemId::Command(CommandId::Plugin(
                        "demo.tools.run".into(),
                    )))
                    .is_some();
        }
    });
    assert!(saw_old_schema_commands);

    let (client, _) = FakeClient::new(new, []);
    let mut saw_prompt_agent = false;
    run_app(client, [esc()], |_, registry, runtime| {
        if runtime.is_some() {
            saw_prompt_agent = registry
                .get(&PaletteItemId::Command(CommandId::Core(
                    CoreCommand::PromptFocusedAgent,
                )))
                .is_some();
        }
    });
    assert!(saw_prompt_agent);

    let (mut client, _) = FakeClient::new(new, []);
    client
        .snapshot
        .agents
        .retain(|agent| agent.pane_id != "pane-a");
    let mut saw_start_agent = false;
    run_app(client, [esc()], |_, registry, runtime| {
        if runtime.is_some() {
            saw_start_agent = registry
                .get(&PaletteItemId::Command(CommandId::Core(
                    CoreCommand::StartAgent,
                )))
                .is_some();
        }
    });
    assert!(saw_start_agent);
}
