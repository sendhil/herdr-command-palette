use std::io;
use std::time::Duration;

use crossterm::event::{self, Event};
use ratatui::layout::Rect;
use thiserror::Error;

use crate::client::{CliHerdrClient, HerdrClient, HerdrOperation, OperationResponse};
use crate::context::parse_invocation_context;
use crate::error::ClientError;
use crate::executor::{CommandExecutor, ExecutionOutcome};
use crate::input::{handle_key, handle_mouse, InputAction};
use crate::model::{
    InstalledPluginInfo, PluginActionInfo, PluginInvocationContext, RuntimeSnapshot,
};
use crate::registry::PaletteCatalog;
use crate::state::{LoadingState, PaletteState};
use crate::terminal::{CrosstermTerminalOps, TerminalGuard};

#[derive(Debug, Error)]
pub enum AppError {
    #[error("command palette startup failed: {0}")]
    Client(#[from] ClientError),
    #[error("terminal I/O failed: {0}")]
    Io(#[from] io::Error),
}

pub struct Bootstrap {
    pub runtime: RuntimeSnapshot,
    pub plugins: Vec<InstalledPluginInfo>,
    pub actions: Vec<PluginActionInfo>,
}

pub fn load_runtime<C: HerdrClient>(
    client: &mut C,
    invocation: PluginInvocationContext,
) -> Result<Bootstrap, AppError> {
    let capabilities = client.api_capabilities()?;
    let session = client.session_snapshot()?;
    let worktrees = match session.focused_workspace_id.as_deref() {
        Some(workspace_id) => match client.worktree_list(workspace_id) {
            Ok(worktrees) => worktrees,
            Err(error) if error.is_not_git_worktree() => Vec::new(),
            Err(error) => return Err(error.into()),
        },
        None => Vec::new(),
    };
    let plugins = client.plugin_list()?;
    let actions = client.plugin_action_list()?;
    Ok(Bootstrap {
        runtime: RuntimeSnapshot {
            session,
            worktrees,
            invocation,
            capabilities,
        },
        plugins,
        actions,
    })
}

pub trait EventSource {
    fn next_event(&mut self) -> Result<Option<Event>, AppError>;
}

pub struct CrosstermEventSource;

impl EventSource for CrosstermEventSource {
    fn next_event(&mut self) -> Result<Option<Event>, AppError> {
        if event::poll(Duration::from_millis(250))? {
            Ok(Some(event::read()?))
        } else {
            Ok(None)
        }
    }
}

pub struct PaletteApp<C> {
    executor: CommandExecutor<C>,
    invocation: PluginInvocationContext,
    runtime: Option<RuntimeSnapshot>,
    plugins: Vec<InstalledPluginInfo>,
    actions: Vec<PluginActionInfo>,
    catalog: PaletteCatalog,
    state: PaletteState,
}

impl<C> PaletteApp<C> {
    pub fn new(client: C, invocation: PluginInvocationContext) -> Self {
        let catalog = PaletteCatalog::empty();
        let mut state = PaletteState::new(Vec::new());
        state.loading = LoadingState::Loading;
        Self {
            executor: CommandExecutor::new(client),
            invocation,
            runtime: None,
            plugins: Vec::new(),
            actions: Vec::new(),
            catalog,
            state,
        }
    }

    pub fn state(&self) -> &PaletteState {
        &self.state
    }

    pub fn catalog(&self) -> &PaletteCatalog {
        &self.catalog
    }

    pub fn runtime(&self) -> Option<&RuntimeSnapshot> {
        self.runtime.as_ref()
    }

    pub fn fail_fatally(&mut self, message: impl Into<String>) {
        self.state.loading = LoadingState::Fatal(message.into());
    }
}

impl<C: HerdrClient> PaletteApp<C> {
    fn bootstrap(&mut self) {
        self.state.loading = LoadingState::Loading;
        match load_runtime(self.executor.client_mut(), self.invocation.clone()) {
            Ok(loaded) => {
                self.runtime = Some(loaded.runtime);
                self.plugins = loaded.plugins;
                self.actions = loaded.actions;
                self.rebuild_catalog();
                self.state.loading = LoadingState::Ready;
                self.state.clear_error();
            }
            Err(error) => {
                self.state.loading = LoadingState::Failed(error.to_string());
            }
        }
    }

    fn rebuild_catalog(&mut self) {
        let Some(runtime) = self.runtime.as_ref() else {
            self.catalog = PaletteCatalog::empty();
            self.state.set_ranked(Vec::new());
            return;
        };
        // Build and rank the next generation before replacing either half of the
        // catalog/ranking pair, so ranked command indices never cross refreshes.
        let catalog = PaletteCatalog::new(runtime, &self.plugins, &self.actions);
        let ranked = catalog.rank(self.state.query.text());
        self.catalog = catalog;
        self.state.set_ranked(ranked);
    }

    pub fn run_with<E, D>(&mut self, events: &mut E, mut draw: D) -> Result<(), AppError>
    where
        E: EventSource,
        D: FnMut(
            &PaletteState,
            &PaletteCatalog,
            Option<&RuntimeSnapshot>,
        ) -> Result<Rect, AppError>,
    {
        let mut area = draw(&self.state, &self.catalog, self.runtime.as_ref())?;
        if !self.state.loading.is_fatal() {
            self.bootstrap();
            area = draw(&self.state, &self.catalog, self.runtime.as_ref())?;
        }

        loop {
            let Some(event) = events.next_event()? else {
                continue;
            };
            let action = match event {
                Event::Key(key) => handle_key(&mut self.state, &self.catalog, key, area),
                Event::Mouse(mouse) => handle_mouse(&mut self.state, &self.catalog, mouse, area),
                Event::Resize(_, _) => InputAction::Redraw,
                Event::FocusGained | Event::FocusLost | Event::Paste(_) => InputAction::None,
            };
            match action {
                InputAction::None => {}
                InputAction::Close => return Ok(()),
                InputAction::Redraw => {
                    area = draw(&self.state, &self.catalog, self.runtime.as_ref())?;
                }
                InputAction::RetryBootstrap => {
                    self.bootstrap();
                    area = draw(&self.state, &self.catalog, self.runtime.as_ref())?;
                }
                InputAction::FocusEntity(id) => {
                    let Some(runtime) = self.runtime.clone() else {
                        self.state.set_error("runtime is not available");
                        area = draw(&self.state, &self.catalog, self.runtime.as_ref())?;
                        continue;
                    };
                    match self.executor.focus_entity(id, &runtime) {
                        ExecutionOutcome::Succeeded => return Ok(()),
                        ExecutionOutcome::Failed { message, refreshed } => {
                            if let Some(refreshed) = refreshed {
                                self.runtime = Some(refreshed);
                                self.rebuild_catalog();
                            }
                            self.state.set_error(message);
                            area = draw(&self.state, &self.catalog, self.runtime.as_ref())?;
                        }
                    }
                }
                InputAction::ExecuteCommand(command) => {
                    let Some(runtime) = self.runtime.clone() else {
                        self.state.set_error("runtime is not available");
                        area = draw(&self.state, &self.catalog, self.runtime.as_ref())?;
                        continue;
                    };
                    let outcome =
                        self.executor
                            .execute(command, &self.state.completed_arguments(), &runtime);
                    match outcome {
                        ExecutionOutcome::Succeeded => return Ok(()),
                        ExecutionOutcome::Failed { message, refreshed } => {
                            if let Some(refreshed) = refreshed {
                                self.runtime = Some(refreshed);
                                self.rebuild_catalog();
                            }
                            self.state.set_error(message);
                            area = draw(&self.state, &self.catalog, self.runtime.as_ref())?;
                        }
                    }
                }
            }
        }
    }
}

enum RuntimeClient {
    Cli(CliHerdrClient),
    Unavailable,
}

impl HerdrClient for RuntimeClient {
    fn api_capabilities(&mut self) -> Result<crate::model::ApiCapabilities, ClientError> {
        match self {
            Self::Cli(client) => client.api_capabilities(),
            Self::Unavailable => Err(ClientError::MissingBinPath),
        }
    }
    fn session_snapshot(&mut self) -> Result<crate::model::SessionSnapshot, ClientError> {
        match self {
            Self::Cli(client) => client.session_snapshot(),
            Self::Unavailable => Err(ClientError::MissingBinPath),
        }
    }
    fn worktree_list(
        &mut self,
        workspace_id: &str,
    ) -> Result<Vec<crate::model::WorktreeInfo>, ClientError> {
        match self {
            Self::Cli(client) => client.worktree_list(workspace_id),
            Self::Unavailable => Err(ClientError::MissingBinPath),
        }
    }
    fn plugin_list(&mut self) -> Result<Vec<InstalledPluginInfo>, ClientError> {
        match self {
            Self::Cli(client) => client.plugin_list(),
            Self::Unavailable => Err(ClientError::MissingBinPath),
        }
    }
    fn plugin_action_list(&mut self) -> Result<Vec<PluginActionInfo>, ClientError> {
        match self {
            Self::Cli(client) => client.plugin_action_list(),
            Self::Unavailable => Err(ClientError::MissingBinPath),
        }
    }
    fn open_palette(&mut self) -> Result<(), ClientError> {
        match self {
            Self::Cli(client) => client.open_palette(),
            Self::Unavailable => Err(ClientError::MissingBinPath),
        }
    }
    fn dispatch(&mut self, operation: &HerdrOperation) -> Result<OperationResponse, ClientError> {
        match self {
            Self::Cli(client) => client.dispatch(operation),
            Self::Unavailable => Err(ClientError::MissingBinPath),
        }
    }
}

pub fn run_open() -> Result<(), AppError> {
    let mut client = CliHerdrClient::from_env()?;
    client.open_palette()?;
    Ok(())
}

fn invocation_from_env() -> Result<PluginInvocationContext, String> {
    std::env::var("HERDR_PLUGIN_CONTEXT_JSON")
        .map_err(|_| "HERDR_PLUGIN_CONTEXT_JSON is missing".to_owned())
        .and_then(|raw| {
            parse_invocation_context(&raw)
                .map_err(|_| "HERDR_PLUGIN_CONTEXT_JSON is invalid".to_owned())
        })
}

fn app_from_invocation<C>(
    client: C,
    invocation: Result<PluginInvocationContext, String>,
) -> PaletteApp<C> {
    match invocation {
        Ok(invocation) => PaletteApp::new(client, invocation),
        Err(message) => {
            let mut app = PaletteApp::new(client, PluginInvocationContext::default());
            app.fail_fatally(message);
            app
        }
    }
}

pub fn run_palette() -> Result<(), AppError> {
    let invocation = invocation_from_env();
    let client = CliHerdrClient::from_env();
    let client_failed = client.is_err();
    let guard = TerminalGuard::enter(CrosstermTerminalOps)?;
    let result = {
        let stdout = io::stdout();
        let backend = ratatui::backend::CrosstermBackend::new(stdout);
        let mut terminal = ratatui::Terminal::new(backend)?;
        let mut app = app_from_invocation(
            match client {
                Ok(client) => RuntimeClient::Cli(client),
                Err(_) => RuntimeClient::Unavailable,
            },
            invocation,
        );
        if client_failed && !app.state().loading.is_fatal() {
            app.fail_fatally("HERDR_BIN_PATH is missing or unavailable");
        }
        let mut events = CrosstermEventSource;
        app.run_with(&mut events, |state, catalog, runtime| {
            let area = terminal.size()?;
            terminal.draw(|frame| crate::view::render(frame, state, catalog, runtime))?;
            Ok(area.into())
        })
    };
    drop(guard);
    result
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::io;
    use std::sync::{Mutex, OnceLock};

    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::Rect;

    use super::{
        app_from_invocation, invocation_from_env, load_runtime, AppError, EventSource, PaletteApp,
    };
    use crate::client::{HerdrClient, HerdrOperation, OperationResponse};
    use crate::command::{CommandId, CoreCommand};
    use crate::context::parse_invocation_context;
    use crate::error::ClientError;
    use crate::model::{
        ApiCapabilities, InstalledPluginInfo, PluginActionInfo, SessionSnapshot, WorktreeInfo,
    };
    use crate::registry::PaletteItemId;

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    #[derive(Default)]
    struct FakeClient {
        calls: Vec<String>,
        session: Option<SessionSnapshot>,
        capability_failures: usize,
        worktree_error: Option<ClientError>,
        plugins: Vec<InstalledPluginInfo>,
        actions: Vec<PluginActionInfo>,
        dispatch_error: Option<ClientError>,
        remove_focused_pane_on_refresh: bool,
    }

    struct FakeEvents {
        events: VecDeque<Event>,
        seen: usize,
    }

    impl FakeEvents {
        fn new(events: impl IntoIterator<Item = Event>) -> Self {
            Self {
                events: events.into_iter().collect(),
                seen: 0,
            }
        }
    }

    impl EventSource for FakeEvents {
        fn next_event(&mut self) -> Result<Option<Event>, AppError> {
            match self.events.pop_front() {
                Some(event) => {
                    self.seen += 1;
                    Ok(Some(event))
                }
                None => Err(AppError::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "scripted event input exhausted",
                ))),
            }
        }
    }

    #[test]
    fn fake_events_fail_fast_when_scripted_input_is_exhausted() {
        let error = FakeEvents::new([])
            .next_event()
            .expect_err("exhausted scripted input should fail");

        assert!(matches!(
            error,
            AppError::Io(error)
                if error.kind() == io::ErrorKind::UnexpectedEof
                    && error.to_string() == "scripted event input exhausted"
        ));
    }

    fn esc() -> Event {
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
    }

    fn key(character: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
    }

    fn enter() -> Event {
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
    }

    fn draw_count(app: &mut PaletteApp<FakeClient>, events: &[Event]) -> usize {
        let mut events = FakeEvents::new(events.iter().cloned());
        let mut draws = 0;
        app.run_with(&mut events, |_, _, _| {
            draws += 1;
            Ok(Rect::new(0, 0, 64, 16))
        })
        .unwrap();
        draws
    }

    impl HerdrClient for FakeClient {
        fn api_capabilities(&mut self) -> Result<ApiCapabilities, ClientError> {
            self.calls.push("capabilities".into());
            if self.capability_failures > 0 {
                self.capability_failures -= 1;
                return Err(ClientError::Process {
                    message: "temporary failure".into(),
                });
            }
            Ok(ApiCapabilities {
                typed_agent_start: true,
                agent_prompt: true,
            })
        }
        fn session_snapshot(&mut self) -> Result<SessionSnapshot, ClientError> {
            self.calls.push("snapshot".into());
            let mut session = self.session.clone().ok_or(ClientError::Process {
                message: "missing fixture".into(),
            })?;
            let is_refresh = self.calls.iter().filter(|call| *call == "snapshot").count() > 1;
            if self.remove_focused_pane_on_refresh && is_refresh {
                session.panes.retain(|pane| pane.pane_id != "pane-a");
                session.focused_pane_id = None;
            }
            Ok(session)
        }
        fn worktree_list(&mut self, id: &str) -> Result<Vec<WorktreeInfo>, ClientError> {
            self.calls.push(format!("worktrees:{id}"));
            match self.worktree_error.take() {
                Some(error) => Err(error),
                None => Ok(Vec::new()),
            }
        }
        fn plugin_list(&mut self) -> Result<Vec<InstalledPluginInfo>, ClientError> {
            self.calls.push("plugins".into());
            Ok(self.plugins.clone())
        }
        fn plugin_action_list(&mut self) -> Result<Vec<PluginActionInfo>, ClientError> {
            self.calls.push("actions".into());
            Ok(self.actions.clone())
        }
        fn open_palette(&mut self) -> Result<(), ClientError> {
            Ok(())
        }
        fn dispatch(&mut self, _: &HerdrOperation) -> Result<OperationResponse, ClientError> {
            self.calls.push("dispatch".into());
            match self.dispatch_error.take() {
                Some(error) => Err(error),
                None => Ok(OperationResponse::Empty),
            }
        }
    }

    fn session() -> SessionSnapshot {
        let value: crate::model::SuccessEnvelope<crate::model::SessionSnapshotResult> =
            serde_json::from_str(include_str!("../tests/fixtures/session-snapshot.json")).unwrap();
        value.result.snapshot
    }

    #[test]
    fn bootstrap_orders_live_reads_and_only_lists_focused_worktree() {
        let mut client = FakeClient {
            calls: Vec::new(),
            session: Some(session()),
            ..FakeClient::default()
        };
        let invocation = parse_invocation_context("{}").unwrap();
        let _ = load_runtime(&mut client, invocation).unwrap();
        assert_eq!(
            client.calls,
            [
                "capabilities",
                "snapshot",
                "worktrees:ws-1",
                "plugins",
                "actions"
            ]
        );
    }

    #[test]
    fn bootstrap_ignores_not_git_worktree_and_continues_live_reads() {
        let expected_session = session();
        let expected_plugins = serde_json::from_str::<
            crate::model::SuccessEnvelope<crate::model::PluginListResult>,
        >(include_str!("../tests/fixtures/plugin-list.json"))
        .unwrap()
        .result
        .plugins;
        let expected_actions = serde_json::from_str::<
            crate::model::SuccessEnvelope<crate::model::PluginActionListResult>,
        >(include_str!("../tests/fixtures/plugin-actions.json"))
        .unwrap()
        .result
        .actions;
        let mut client = FakeClient {
            calls: Vec::new(),
            session: Some(expected_session.clone()),
            worktree_error: Some(ClientError::Api {
                code: "not_git_worktree".into(),
                message: "workspace is not a Git worktree".into(),
            }),
            plugins: expected_plugins.clone(),
            actions: expected_actions.clone(),
            ..FakeClient::default()
        };

        let loaded = load_runtime(&mut client, parse_invocation_context("{}").unwrap()).unwrap();

        assert_eq!(loaded.runtime.session, expected_session);
        assert_eq!(
            loaded.runtime.capabilities,
            ApiCapabilities {
                typed_agent_start: true,
                agent_prompt: true,
            }
        );
        assert!(loaded.runtime.worktrees.is_empty());
        assert_eq!(loaded.plugins, expected_plugins);
        assert_eq!(loaded.actions, expected_actions);
        assert_eq!(
            client.calls,
            [
                "capabilities",
                "snapshot",
                "worktrees:ws-1",
                "plugins",
                "actions"
            ]
        );
    }

    #[test]
    fn bootstrap_stops_on_non_not_git_worktree_api_errors_before_plugin_reads() {
        let mut client = FakeClient {
            calls: Vec::new(),
            session: Some(session()),
            worktree_error: Some(ClientError::Api {
                code: "worktree_not_found".into(),
                message: "workspace disappeared".into(),
            }),
            ..FakeClient::default()
        };

        let error = match load_runtime(&mut client, parse_invocation_context("{}").unwrap()) {
            Err(error) => error,
            Ok(_) => panic!("non-not_git_worktree errors must remain fatal"),
        };

        assert!(matches!(
            error,
            AppError::Client(ClientError::Api { ref code, ref message })
                if code == "worktree_not_found" && message == "workspace disappeared"
        ));
        assert_eq!(client.calls, ["capabilities", "snapshot", "worktrees:ws-1"]);
    }

    #[test]
    fn bootstrap_without_a_focused_workspace_skips_worktree_listing() {
        let mut snapshot = session();
        snapshot.focused_workspace_id = None;
        let mut client = FakeClient {
            calls: Vec::new(),
            session: Some(snapshot),
            ..FakeClient::default()
        };

        let loaded = load_runtime(&mut client, parse_invocation_context("{}").unwrap()).unwrap();

        assert!(loaded.runtime.worktrees.is_empty());
        assert_eq!(
            client.calls,
            ["capabilities", "snapshot", "plugins", "actions"]
        );
    }

    #[test]
    fn invocation_environment_reports_real_missing_and_malformed_context_values() {
        let _lock = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let original = std::env::var_os("HERDR_PLUGIN_CONTEXT_JSON");

        for (raw, expected) in [
            (None, "HERDR_PLUGIN_CONTEXT_JSON is missing"),
            (Some("not-json"), "HERDR_PLUGIN_CONTEXT_JSON is invalid"),
        ] {
            match raw {
                Some(raw) => std::env::set_var("HERDR_PLUGIN_CONTEXT_JSON", raw),
                None => std::env::remove_var("HERDR_PLUGIN_CONTEXT_JSON"),
            }
            let mut app = app_from_invocation(
                FakeClient {
                    calls: Vec::new(),
                    session: Some(session()),
                    ..FakeClient::default()
                },
                invocation_from_env(),
            );
            assert!(matches!(
                app.state().loading,
                crate::state::LoadingState::Fatal(ref message) if message == expected
            ));
            let mut events = FakeEvents::new([esc()]);
            let mut draws = 0;
            app.run_with(&mut events, |_, _, _| {
                draws += 1;
                Ok(Rect::new(0, 0, 64, 16))
            })
            .unwrap();
            assert_eq!(draws, 1);
            assert_eq!(events.seen, 1);
            assert!(app.executor.into_client().calls.is_empty());
        }

        match original {
            Some(value) => std::env::set_var("HERDR_PLUGIN_CONTEXT_JSON", value),
            None => std::env::remove_var("HERDR_PLUGIN_CONTEXT_JSON"),
        }
    }

    #[test]
    fn palette_app_has_no_popup_pane_id_constructor_argument() {
        let _ = PaletteApp::new(
            FakeClient {
                calls: Vec::new(),
                session: Some(session()),
                ..FakeClient::default()
            },
            parse_invocation_context("{}").unwrap(),
        );
    }

    #[test]
    fn retry_reboots_after_a_bootstrap_failure_and_escape_closes_the_palette() {
        let mut app = PaletteApp::new(
            FakeClient {
                calls: Vec::new(),
                session: Some(session()),
                capability_failures: 1,
                ..FakeClient::default()
            },
            parse_invocation_context("{}").unwrap(),
        );
        let draws = draw_count(&mut app, &[key('r'), esc()]);
        assert!(draws >= 3);
        assert!(matches!(
            app.state().loading,
            crate::state::LoadingState::Ready
        ));
        assert_eq!(
            app.executor.into_client().calls,
            [
                "capabilities",
                "capabilities",
                "snapshot",
                "worktrees:ws-1",
                "plugins",
                "actions"
            ]
        );
    }

    #[test]
    fn escape_event_closes_the_running_event_loop_without_consuming_later_events() {
        let mut app = PaletteApp::new(
            FakeClient {
                calls: Vec::new(),
                session: Some(session()),
                ..FakeClient::default()
            },
            parse_invocation_context("{}").unwrap(),
        );
        let mut events = FakeEvents::new([esc(), key('r')]);

        app.run_with(&mut events, |_, _, _| Ok(Rect::new(0, 0, 64, 16)))
            .unwrap();

        assert_eq!(events.seen, 1);
    }

    #[test]
    fn successful_entity_focus_exits_without_consuming_later_events() {
        let mut app = PaletteApp::new(
            FakeClient {
                calls: Vec::new(),
                session: Some(session()),
                ..FakeClient::default()
            },
            parse_invocation_context("{}").unwrap(),
        );
        let mut events = FakeEvents::new([
            key('r'),
            key('e'),
            key('v'),
            key('i'),
            key('e'),
            key('w'),
            key('e'),
            key('r'),
            enter(),
            esc(),
        ]);

        app.run_with(&mut events, |_, _, _| Ok(Rect::new(0, 0, 64, 16)))
            .unwrap();

        assert_eq!(events.seen, 9);
        assert_eq!(
            app.executor.into_client().calls,
            [
                "capabilities",
                "snapshot",
                "worktrees:ws-1",
                "plugins",
                "actions",
                "dispatch",
            ]
        );
    }

    #[test]
    fn successful_execution_exits_the_event_loop_without_consuming_later_events() {
        let mut app = PaletteApp::new(
            FakeClient {
                calls: Vec::new(),
                session: Some(session()),
                ..FakeClient::default()
            },
            parse_invocation_context("{}").unwrap(),
        );
        let mut events = FakeEvents::new([key('z'), key('o'), key('o'), key('m'), enter(), esc()]);

        app.run_with(&mut events, |_, _, _| Ok(Rect::new(0, 0, 64, 16)))
            .unwrap();

        assert_eq!(events.seen, 5);
        assert_eq!(
            app.executor
                .into_client()
                .calls
                .iter()
                .filter(|call| *call == "dispatch")
                .count(),
            1
        );
    }

    #[test]
    fn resize_redraws_and_command_failure_keeps_the_palette_open_until_escape() {
        let mut app = PaletteApp::new(
            FakeClient {
                calls: Vec::new(),
                session: Some(session()),
                dispatch_error: Some(ClientError::Process {
                    message: "command failed".into(),
                }),
                ..FakeClient::default()
            },
            parse_invocation_context("{}").unwrap(),
        );
        let draws = draw_count(
            &mut app,
            &[
                Event::Resize(80, 24),
                key('>'),
                Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
                esc(),
            ],
        );
        assert!(draws >= 4);
        assert_eq!(
            app.state().error.as_deref(),
            Some("failed while running Herdr: command failed")
        );
        assert!(app
            .executor
            .into_client()
            .calls
            .contains(&"dispatch".into()));
    }

    #[test]
    fn stale_entity_failure_rebuilds_catalog_before_draw_and_preserves_raw_query() {
        let mut app = PaletteApp::new(
            FakeClient {
                calls: Vec::new(),
                session: Some(session()),
                dispatch_error: Some(ClientError::Api {
                    code: "agent_not_found".into(),
                    message: "stale agent".into(),
                }),
                remove_focused_pane_on_refresh: true,
                ..FakeClient::default()
            },
            parse_invocation_context("{}").unwrap(),
        );
        let mut events = FakeEvents::new([
            key('r'),
            key('e'),
            key('v'),
            key('i'),
            key('e'),
            key('w'),
            key('e'),
            key('r'),
            enter(),
            esc(),
        ]);
        let mut refreshed_drawn = false;
        app.run_with(&mut events, |state, catalog, runtime| {
            if runtime.is_some_and(|runtime| runtime.focused_pane().is_none()) {
                refreshed_drawn = true;
                assert_eq!(state.query.text(), "reviewer");
                assert!(catalog
                    .get(&PaletteItemId::Agent("pane-a".into()))
                    .is_none());
                assert!(state
                    .ranked
                    .iter()
                    .all(|ranked| catalog.get_ranked(ranked).is_some()));
            }
            Ok(Rect::new(0, 0, 64, 16))
        })
        .unwrap();

        assert!(refreshed_drawn);
        assert_eq!(events.seen, 10);
        assert_eq!(
            app.state().error.as_deref(),
            Some("Herdr returned agent_not_found: stale agent")
        );
        assert_eq!(
            app.executor.into_client().calls,
            [
                "capabilities",
                "snapshot",
                "worktrees:ws-1",
                "plugins",
                "actions",
                "dispatch",
                "snapshot",
                "worktrees:ws-1",
            ]
        );
    }

    #[test]
    fn stale_refresh_rebuilds_catalog_before_rendering_or_handling_the_next_event() {
        let mut app = PaletteApp::new(
            FakeClient {
                calls: Vec::new(),
                session: Some(session()),
                dispatch_error: Some(ClientError::Api {
                    code: "pane_not_found".into(),
                    message: "stale pane".into(),
                }),
                remove_focused_pane_on_refresh: true,
                ..FakeClient::default()
            },
            parse_invocation_context("{}").unwrap(),
        );
        let mut events = FakeEvents::new([key('>'), enter(), esc()]);
        let mut refreshed_rendered = false;
        app.run_with(&mut events, |state, catalog, runtime| {
            if runtime.is_some_and(|runtime| runtime.focused_pane().is_none()) {
                refreshed_rendered = true;
                assert!(catalog
                    .get(&PaletteItemId::Command(CommandId::Core(
                        CoreCommand::TogglePaneZoom
                    )))
                    .is_none());
                assert!(state
                    .ranked
                    .iter()
                    .all(|ranked| catalog.get(&ranked.id).is_some()));
            }
            Ok(Rect::new(0, 0, 64, 16))
        })
        .unwrap();

        assert!(refreshed_rendered);
        assert_eq!(events.seen, 3);
        assert_eq!(
            app.executor
                .into_client()
                .calls
                .iter()
                .filter(|call| *call == "dispatch")
                .count(),
            1
        );
    }
}
