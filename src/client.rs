use std::ffi::OsString;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
#[cfg(test)]
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;

pub use crate::error::{ClientError, StreamKind};
use crate::model::{
    capabilities_from_schema, ApiCapabilities, Direction, ErrorEnvelope, InstalledPluginInfo,
    PaneInfo, PluginActionInfo, PluginActionListResult, PluginListResult, SessionSnapshot,
    SessionSnapshotResult, SplitDirection, SuccessEnvelope, WorktreeInfo, WorktreeListResult,
};

const DIAGNOSTIC_STDERR_LIMIT: usize = 256 * 1024;
/// JSON list responses may be large; keep them bounded without truncating normal action lists.
const JSON_STDOUT_LIMIT: usize = 4 * 1024 * 1024;
const DEFAULT_STDOUT_LIMIT: usize = 256 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(5);
const PALETTE_PLUGIN_ID: &str = "herdr.command-palette";
const PALETTE_ENTRYPOINT: &str = "palette";

#[cfg(test)]
static FORCE_READER_PANIC: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
static READER_PANIC_RELEASE: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
static READER_PANIC_ENTERED: AtomicUsize = AtomicUsize::new(0);

pub trait HerdrClient {
    fn api_capabilities(&mut self) -> Result<ApiCapabilities, ClientError>;
    fn session_snapshot(&mut self) -> Result<SessionSnapshot, ClientError>;
    fn worktree_list(&mut self, workspace_id: &str) -> Result<Vec<WorktreeInfo>, ClientError>;
    fn plugin_list(&mut self) -> Result<Vec<InstalledPluginInfo>, ClientError>;
    fn plugin_action_list(&mut self) -> Result<Vec<PluginActionInfo>, ClientError>;
    fn open_palette(&mut self) -> Result<(), ClientError>;
    fn dispatch(&mut self, operation: &HerdrOperation) -> Result<OperationResponse, ClientError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestClass {
    BootstrapRead,
    OpenOrAction,
    Mutation,
}

impl RequestClass {
    pub const fn default_timeout(self) -> Duration {
        match self {
            Self::BootstrapRead => Duration::from_secs(3),
            Self::OpenOrAction => Duration::from_secs(5),
            Self::Mutation => Duration::from_secs(30),
        }
    }
}

impl std::fmt::Display for RequestClass {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BootstrapRead => formatter.write_str("bootstrap read"),
            Self::OpenOrAction => formatter.write_str("open/action"),
            Self::Mutation => formatter.write_str("mutation"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientTimeouts {
    pub bootstrap_read: Duration,
    pub open_or_action: Duration,
    pub mutation: Duration,
}

impl ClientTimeouts {
    const fn for_class(self, class: RequestClass) -> Duration {
        match class {
            RequestClass::BootstrapRead => self.bootstrap_read,
            RequestClass::OpenOrAction => self.open_or_action,
            RequestClass::Mutation => self.mutation,
        }
    }
}

impl Default for ClientTimeouts {
    fn default() -> Self {
        Self {
            bootstrap_read: RequestClass::BootstrapRead.default_timeout(),
            open_or_action: RequestClass::OpenOrAction.default_timeout(),
            mutation: RequestClass::Mutation.default_timeout(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationResponse {
    Empty,
    PaneCreated { pane_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HerdrOperation {
    WorkspaceFocus {
        workspace_id: String,
    },
    WorkspaceCreate {
        label: String,
        cwd: Option<PathBuf>,
    },
    WorkspaceRename {
        workspace_id: String,
        label: String,
    },
    WorkspaceClose {
        workspace_id: String,
    },
    WorktreeOpen {
        workspace_id: String,
        path: PathBuf,
    },
    WorktreeCreate {
        workspace_id: String,
        branch: String,
        base: String,
        label: Option<String>,
    },
    TabFocus {
        tab_id: String,
    },
    TabCreate {
        workspace_id: String,
        label: String,
        cwd: Option<PathBuf>,
    },
    TabRename {
        tab_id: String,
        label: String,
    },
    TabClose {
        tab_id: String,
    },
    PaneFocus {
        pane_id: String,
        direction: Direction,
    },
    PaneSplit {
        pane_id: String,
        direction: SplitDirection,
        cwd: Option<PathBuf>,
    },
    PaneZoomToggle {
        pane_id: String,
    },
    PaneRename {
        pane_id: String,
        label: Option<String>,
    },
    PaneClose {
        pane_id: String,
    },
    PaneRun {
        pane_id: String,
        command: String,
    },
    AgentFocus {
        pane_id: String,
    },
    AgentRename {
        pane_id: String,
        name: Option<String>,
    },
    AgentPrompt {
        pane_id: String,
        text: String,
    },
    AgentStart {
        name: String,
        kind: String,
        pane_id: String,
    },
    PluginActionInvoke {
        action_id: String,
    },
}

impl HerdrOperation {
    pub fn argv(&self) -> Vec<OsString> {
        match self {
            Self::WorkspaceFocus { workspace_id } => strings(["workspace", "focus"])
                .into_iter()
                .chain([OsString::from(workspace_id)])
                .collect(),
            Self::WorkspaceCreate { label, cwd } => {
                let mut argv = strings(["workspace", "create", "--label"]);
                argv.push(OsString::from(label));
                if let Some(cwd) = cwd {
                    argv.push(OsString::from("--cwd"));
                    argv.push(cwd.as_os_str().to_owned());
                }
                argv.push(OsString::from("--focus"));
                argv
            }
            Self::WorkspaceRename {
                workspace_id,
                label,
            } => with_values(
                ["workspace", "rename"],
                [workspace_id.as_str(), label.as_str()],
            ),
            Self::WorkspaceClose { workspace_id } => {
                with_values(["workspace", "close"], [workspace_id.as_str()])
            }
            Self::WorktreeOpen { workspace_id, path } => {
                let mut argv =
                    with_values(["worktree", "open", "--workspace"], [workspace_id.as_str()]);
                argv.push(OsString::from("--path"));
                argv.push(path.as_os_str().to_owned());
                argv.extend(strings(["--focus", "--json"]));
                argv
            }
            Self::WorktreeCreate {
                workspace_id,
                branch,
                base,
                label,
            } => {
                let mut argv = with_values(
                    ["worktree", "create", "--workspace"],
                    [workspace_id.as_str()],
                );
                argv.push(OsString::from("--branch"));
                argv.push(OsString::from(branch));
                argv.push(OsString::from("--base"));
                argv.push(OsString::from(base));
                if let Some(label) = label {
                    argv.push(OsString::from("--label"));
                    argv.push(OsString::from(label));
                }
                argv.extend(strings(["--focus", "--json"]));
                argv
            }
            Self::TabFocus { tab_id } => with_values(["tab", "focus"], [tab_id.as_str()]),
            Self::TabCreate {
                workspace_id,
                label,
                cwd,
            } => {
                let mut argv =
                    with_values(["tab", "create", "--workspace"], [workspace_id.as_str()]);
                argv.push(OsString::from("--label"));
                argv.push(OsString::from(label));
                if let Some(cwd) = cwd {
                    argv.push(OsString::from("--cwd"));
                    argv.push(cwd.as_os_str().to_owned());
                }
                argv.push(OsString::from("--focus"));
                argv
            }
            Self::TabRename { tab_id, label } => {
                with_values(["tab", "rename"], [tab_id.as_str(), label.as_str()])
            }
            Self::TabClose { tab_id } => with_values(["tab", "close"], [tab_id.as_str()]),
            Self::PaneFocus { pane_id, direction } => with_values(
                ["pane", "focus", "--pane"],
                [pane_id.as_str(), "--direction", direction_name(*direction)],
            ),
            Self::PaneSplit {
                pane_id,
                direction,
                cwd,
            } => {
                let mut argv = with_values(
                    ["pane", "split", "--pane"],
                    [
                        pane_id.as_str(),
                        "--direction",
                        split_direction_name(*direction),
                        "--ratio",
                        "0.5",
                    ],
                );
                if let Some(cwd) = cwd {
                    argv.push(OsString::from("--cwd"));
                    argv.push(cwd.as_os_str().to_owned());
                }
                argv.push(OsString::from("--focus"));
                argv
            }
            Self::PaneZoomToggle { pane_id } => {
                with_values(["pane", "zoom", "--pane"], [pane_id.as_str(), "--toggle"])
            }
            Self::PaneRename { pane_id, label } => match label {
                Some(label) => with_values(["pane", "rename"], [pane_id.as_str(), label.as_str()]),
                None => with_values(["pane", "rename"], [pane_id.as_str(), "--clear"]),
            },
            Self::PaneClose { pane_id } => with_values(["pane", "close"], [pane_id.as_str()]),
            Self::PaneRun { pane_id, command } => {
                with_values(["pane", "run"], [pane_id.as_str(), command.as_str()])
            }
            Self::AgentFocus { pane_id } => with_values(["agent", "focus"], [pane_id.as_str()]),
            Self::AgentRename { pane_id, name } => match name {
                Some(name) => with_values(["agent", "rename"], [pane_id.as_str(), name.as_str()]),
                None => with_values(["agent", "rename"], [pane_id.as_str(), "--clear"]),
            },
            Self::AgentPrompt { pane_id, text } => {
                with_values(["agent", "prompt"], [pane_id.as_str(), text.as_str()])
            }
            Self::AgentStart {
                name,
                kind,
                pane_id,
            } => with_values(
                ["agent", "start"],
                [
                    name.as_str(),
                    "--kind",
                    kind.as_str(),
                    "--pane",
                    pane_id.as_str(),
                    "--timeout",
                    "30000",
                ],
            ),
            Self::PluginActionInvoke { action_id } => {
                with_values(["plugin", "action", "invoke"], [action_id.as_str()])
            }
        }
    }

    pub const fn request_class(&self) -> RequestClass {
        match self {
            Self::PluginActionInvoke { .. } => RequestClass::OpenOrAction,
            Self::WorkspaceFocus { .. }
            | Self::WorkspaceCreate { .. }
            | Self::WorkspaceRename { .. }
            | Self::WorkspaceClose { .. }
            | Self::WorktreeOpen { .. }
            | Self::WorktreeCreate { .. }
            | Self::TabFocus { .. }
            | Self::TabCreate { .. }
            | Self::TabRename { .. }
            | Self::TabClose { .. }
            | Self::PaneFocus { .. }
            | Self::PaneSplit { .. }
            | Self::PaneZoomToggle { .. }
            | Self::PaneRename { .. }
            | Self::PaneClose { .. }
            | Self::PaneRun { .. }
            | Self::AgentFocus { .. }
            | Self::AgentRename { .. }
            | Self::AgentPrompt { .. }
            | Self::AgentStart { .. } => RequestClass::Mutation,
        }
    }

    const fn response_codec(&self) -> ResponseCodec {
        match self {
            Self::PaneRun { .. } => ResponseCodec::Empty,
            Self::PaneSplit { .. } => ResponseCodec::PaneCreated,
            Self::WorkspaceFocus { .. }
            | Self::WorkspaceCreate { .. }
            | Self::WorkspaceRename { .. }
            | Self::WorkspaceClose { .. }
            | Self::WorktreeOpen { .. }
            | Self::WorktreeCreate { .. }
            | Self::TabFocus { .. }
            | Self::TabCreate { .. }
            | Self::TabRename { .. }
            | Self::TabClose { .. }
            | Self::PaneFocus { .. }
            | Self::PaneZoomToggle { .. }
            | Self::PaneRename { .. }
            | Self::PaneClose { .. }
            | Self::AgentFocus { .. }
            | Self::AgentRename { .. }
            | Self::AgentPrompt { .. }
            | Self::AgentStart { .. }
            | Self::PluginActionInvoke { .. } => ResponseCodec::JsonAcknowledgement,
        }
    }
}

fn strings<const N: usize>(values: [&str; N]) -> Vec<OsString> {
    values.into_iter().map(OsString::from).collect()
}

fn with_values<const P: usize, const V: usize>(
    prefix: [&str; P],
    values: [&str; V],
) -> Vec<OsString> {
    prefix
        .into_iter()
        .chain(values)
        .map(OsString::from)
        .collect()
}

const fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::Left => "left",
        Direction::Right => "right",
        Direction::Up => "up",
        Direction::Down => "down",
    }
}

const fn split_direction_name(direction: SplitDirection) -> &'static str {
    match direction {
        SplitDirection::Right => "right",
        SplitDirection::Down => "down",
    }
}

#[derive(Debug, Clone, Copy)]
enum ResponseCodec {
    Empty,
    JsonAcknowledgement,
    PaneCreated,
}

pub struct CliHerdrClient {
    bin_path: PathBuf,
    timeouts: ClientTimeouts,
}

impl CliHerdrClient {
    pub fn from_env() -> Result<Self, ClientError> {
        Self::from_env_with_timeouts(ClientTimeouts::default())
    }

    pub fn from_env_with_timeouts(timeouts: ClientTimeouts) -> Result<Self, ClientError> {
        let bin_path = std::env::var_os("HERDR_BIN_PATH")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or(ClientError::MissingBinPath)?;
        Ok(Self { bin_path, timeouts })
    }

    fn invoke(
        &self,
        argv: &[OsString],
        class: RequestClass,
    ) -> Result<CapturedOutput, ClientError> {
        self.invoke_with_stdout_limit(argv, class, DEFAULT_STDOUT_LIMIT)
    }

    fn invoke_with_stdout_limit(
        &self,
        argv: &[OsString],
        class: RequestClass,
        stdout_limit: usize,
    ) -> Result<CapturedOutput, ClientError> {
        let mut command = Command::new(&self.bin_path);
        command
            .args(argv)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn().map_err(|error| ClientError::Spawn {
            message: sanitize_text(error.to_string().as_bytes()),
        })?;

        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                terminate_and_reap(&mut child)?;
                return Err(ClientError::Process {
                    message: "failed to capture Herdr stdout".into(),
                });
            }
        };
        let stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => {
                terminate_and_reap(&mut child)?;
                return Err(ClientError::Process {
                    message: "failed to capture Herdr stderr".into(),
                });
            }
        };
        let (event_sender, event_receiver) = mpsc::channel();
        let stdout_reader = spawn_reader(
            stdout,
            StreamKind::Stdout,
            stdout_limit,
            event_sender.clone(),
        );
        let stderr_reader = spawn_reader(
            stderr,
            StreamKind::Stderr,
            DIAGNOSTIC_STDERR_LIMIT,
            event_sender.clone(),
        );
        drop(event_sender);

        let started = Instant::now();
        let timeout = self.timeouts.for_class(class);
        let mut status = None;
        let mut stdout = None;
        let mut stderr = None;
        let mut stdout_exceeded = false;
        let mut stderr_exceeded = false;
        let mut terminal_error = None;
        let mut terminated = false;

        loop {
            let mut reader_disconnected = false;
            loop {
                match event_receiver.try_recv() {
                    Ok(ReaderEvent::Overflow(StreamKind::Stdout)) => stdout_exceeded = true,
                    Ok(ReaderEvent::Overflow(StreamKind::Stderr)) => stderr_exceeded = true,
                    Ok(ReaderEvent::Finished(StreamKind::Stdout, result)) => stdout = Some(result),
                    Ok(ReaderEvent::Finished(StreamKind::Stderr, result)) => stderr = Some(result),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        reader_disconnected = true;
                        break;
                    }
                }
            }

            if terminal_error.is_none()
                && reader_disconnected
                && (stdout.is_none() || stderr.is_none())
            {
                terminal_error = Some(ClientError::Process {
                    message: sanitize_text(
                        b"capture event channel disconnected before both streams completed",
                    ),
                });
            }

            if status.is_none() {
                match child.try_wait() {
                    Ok(Some(child_status)) => status = Some(child_status),
                    Ok(None) => {}
                    Err(error) => {
                        terminal_error = Some(ClientError::Process {
                            message: sanitize_text(error.to_string().as_bytes()),
                        });
                    }
                }
            }

            if terminal_error.is_none() {
                terminal_error = output_limit_error(
                    stdout_exceeded,
                    stderr_exceeded,
                    stdout_limit,
                    DIAGNOSTIC_STDERR_LIMIT,
                );
            }
            if terminal_error.is_none() && started.elapsed() >= timeout {
                terminal_error = Some(ClientError::Timeout { class });
            }
            if terminal_error.is_some() && !terminated {
                if let Err(error) = terminate_and_reap(&mut child) {
                    terminal_error = Some(error);
                }
                terminated = true;
            }

            if terminal_error.is_some()
                || (status.is_some() && stdout.is_some() && stderr.is_some())
            {
                break;
            }
            thread::sleep(POLL_INTERVAL);
        }

        let reader_error = join_readers(stdout_reader, stderr_reader);
        reader_error?;
        if let Some(error) = terminal_error {
            return Err(error);
        }
        let stdout = capture_result(stdout.expect("stdout reader completed"), StreamKind::Stdout)?;
        let stderr = capture_result(stderr.expect("stderr reader completed"), StreamKind::Stderr)?;
        let output = CapturedOutput {
            status: status.expect("child status completed"),
            stdout: stdout.bytes,
            stderr: stderr.bytes,
        };
        output.ensure_success()?;
        Ok(output)
    }

    fn json<T: DeserializeOwned>(
        &self,
        argv: &[OsString],
        class: RequestClass,
        context: &'static str,
    ) -> Result<T, ClientError> {
        let output = self.invoke_with_stdout_limit(argv, class, JSON_STDOUT_LIMIT)?;
        deserialize_json(&output.stdout, context)
    }

    fn json_bounded<T: DeserializeOwned>(
        &self,
        argv: &[OsString],
        class: RequestClass,
        context: &'static str,
    ) -> Result<T, ClientError> {
        let output = self.invoke(argv, class)?;
        deserialize_json(&output.stdout, context)
    }

    fn json_ack(
        &self,
        argv: &[OsString],
        class: RequestClass,
        context: &'static str,
    ) -> Result<(), ClientError> {
        let response: SuccessEnvelope<TaggedResult> = self.json(argv, class, context)?;
        if response.result.response_type.trim().is_empty() {
            return Err(ClientError::UnexpectedResponse {
                context,
                message: "response type is empty".into(),
            });
        }
        Ok(())
    }
}

impl HerdrClient for CliHerdrClient {
    fn api_capabilities(&mut self) -> Result<ApiCapabilities, ClientError> {
        let argv = strings(["api", "schema", "--json"]);
        let schema: Value =
            self.json(&argv, RequestClass::BootstrapRead, "API capability schema")?;
        capabilities_from_schema(&schema).map_err(|error| ClientError::UnexpectedResponse {
            context: "API capability schema",
            message: sanitize_text(error.to_string().as_bytes()),
        })
    }

    fn session_snapshot(&mut self) -> Result<SessionSnapshot, ClientError> {
        let argv = strings(["api", "snapshot"]);
        let response: SuccessEnvelope<SessionSnapshotResult> =
            self.json(&argv, RequestClass::BootstrapRead, "session snapshot")?;
        expect_response_type(
            &response.result.response_type,
            "session_snapshot",
            "session snapshot",
        )?;
        Ok(response.result.snapshot)
    }

    fn worktree_list(&mut self, workspace_id: &str) -> Result<Vec<WorktreeInfo>, ClientError> {
        let argv = with_values(
            ["worktree", "list", "--workspace"],
            [workspace_id, "--json"],
        );
        let response: SuccessEnvelope<WorktreeListResult> =
            self.json(&argv, RequestClass::BootstrapRead, "worktree list")?;
        expect_response_type(
            &response.result.response_type,
            "worktree_list",
            "worktree list",
        )?;
        Ok(response.result.worktrees)
    }

    fn plugin_list(&mut self) -> Result<Vec<InstalledPluginInfo>, ClientError> {
        let argv = strings(["plugin", "list", "--json"]);
        let response: SuccessEnvelope<PluginListResult> =
            self.json(&argv, RequestClass::BootstrapRead, "plugin list")?;
        expect_response_type(&response.result.response_type, "plugin_list", "plugin list")?;
        Ok(response.result.plugins)
    }

    fn plugin_action_list(&mut self) -> Result<Vec<PluginActionInfo>, ClientError> {
        let argv = strings(["plugin", "action", "list"]);
        let response: SuccessEnvelope<PluginActionListResult> =
            self.json(&argv, RequestClass::BootstrapRead, "plugin action list")?;
        expect_response_type(
            &response.result.response_type,
            "plugin_action_list",
            "plugin action list",
        )?;
        Ok(response.result.actions)
    }

    fn open_palette(&mut self) -> Result<(), ClientError> {
        let argv = with_values(
            ["plugin", "pane", "open", "--plugin"],
            [PALETTE_PLUGIN_ID, "--entrypoint", PALETTE_ENTRYPOINT],
        );
        self.json_ack(&argv, RequestClass::OpenOrAction, "open palette")
    }

    fn dispatch(&mut self, operation: &HerdrOperation) -> Result<OperationResponse, ClientError> {
        let argv = operation.argv();
        let class = operation.request_class();
        match operation.response_codec() {
            ResponseCodec::Empty => {
                self.invoke(&argv, class)?;
                Ok(OperationResponse::Empty)
            }
            ResponseCodec::JsonAcknowledgement => {
                self.json_ack(&argv, class, "operation acknowledgement")?;
                Ok(OperationResponse::Empty)
            }
            ResponseCodec::PaneCreated => {
                let response: SuccessEnvelope<PaneInfoResult> =
                    self.json_bounded(&argv, class, "pane split")?;
                expect_response_type(&response.result.response_type, "pane_info", "pane split")?;
                Ok(OperationResponse::PaneCreated {
                    pane_id: response.result.pane.pane_id,
                })
            }
        }
    }
}

fn expect_response_type(
    actual: &str,
    expected: &str,
    context: &'static str,
) -> Result<(), ClientError> {
    if actual == expected {
        Ok(())
    } else {
        Err(ClientError::UnexpectedResponse {
            context,
            message: format!(
                "expected {expected}, received {}",
                sanitize_text(actual.as_bytes())
            ),
        })
    }
}

#[derive(Debug, Deserialize)]
struct TaggedResult {
    #[serde(rename = "type")]
    response_type: String,
}

#[derive(Debug, Deserialize)]
struct PaneInfoResult {
    #[serde(rename = "type")]
    response_type: String,
    pane: PaneInfo,
}

struct CapturedOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl CapturedOutput {
    fn ensure_success(&self) -> Result<(), ClientError> {
        if self.status.success() {
            return Ok(());
        }

        if let Some(error) = parse_error_envelope(&self.stderr) {
            return Err(ClientError::Api {
                code: sanitize_text(error.error.code.as_bytes()),
                message: sanitize_text(error.error.message.as_bytes()),
            });
        }

        let stderr = sanitize_text(&self.stderr);
        let message = if stderr.is_empty() {
            match self.status.code() {
                Some(code) => format!("exit status {code}"),
                None => "terminated by signal".into(),
            }
        } else {
            stderr
        };
        Err(ClientError::Exit { message })
    }
}

fn parse_error_envelope(bytes: &[u8]) -> Option<ErrorEnvelope> {
    serde_json::from_slice(bytes).ok()
}

fn deserialize_json<T: DeserializeOwned>(
    bytes: &[u8],
    context: &'static str,
) -> Result<T, ClientError> {
    serde_json::from_slice(bytes).map_err(|error| ClientError::InvalidJson {
        context,
        message: sanitize_text(error.to_string().as_bytes()),
    })
}

struct CapturedStream {
    bytes: Vec<u8>,
}

enum ReaderEvent {
    Overflow(StreamKind),
    Finished(StreamKind, Result<CapturedStream, std::io::Error>),
}

fn spawn_reader<R: Read + Send + 'static>(
    reader: R,
    stream: StreamKind,
    limit: usize,
    sender: mpsc::Sender<ReaderEvent>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        #[cfg(test)]
        if FORCE_READER_PANIC.load(Ordering::SeqCst) {
            READER_PANIC_ENTERED.fetch_add(1, Ordering::SeqCst);
            while !READER_PANIC_RELEASE.load(Ordering::SeqCst) {
                thread::yield_now();
            }
            panic!("forced capture thread failure");
        }

        let result = capture_stream(reader, limit, || {
            let _ = sender.send(ReaderEvent::Overflow(stream));
        });
        let _ = sender.send(ReaderEvent::Finished(stream, result));
    })
}

fn capture_stream<R: Read, F: FnOnce()>(
    mut reader: R,
    limit: usize,
    overflow_signal: F,
) -> Result<CapturedStream, std::io::Error> {
    let mut bytes = Vec::with_capacity(limit);
    let mut overflow_signal = Some(overflow_signal);
    let mut chunk = [0_u8; 8192];
    loop {
        let count = reader.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        let remaining = limit.saturating_sub(bytes.len());
        let retained = remaining.min(count);
        bytes.extend_from_slice(&chunk[..retained]);
        if retained < count {
            if let Some(signal) = overflow_signal.take() {
                signal();
            }
        }
    }
    Ok(CapturedStream { bytes })
}

/// A drain batch containing both overflow events reports stdout. If batches differ,
/// the first observed overflow terminates the process before a later batch can change it.
fn output_limit_error(
    stdout_exceeded: bool,
    stderr_exceeded: bool,
    stdout_limit: usize,
    stderr_limit: usize,
) -> Option<ClientError> {
    match (stdout_exceeded, stderr_exceeded) {
        (true, _) => Some(ClientError::OutputLimit {
            stream: StreamKind::Stdout,
            limit: stdout_limit,
        }),
        (false, true) => Some(ClientError::OutputLimit {
            stream: StreamKind::Stderr,
            limit: stderr_limit,
        }),
        (false, false) => None,
    }
}

fn join_reader(reader: thread::JoinHandle<()>, stream: StreamKind) -> Result<(), ClientError> {
    reader.join().map_err(|_| ClientError::Process {
        message: format!("{stream} capture thread failed"),
    })
}

fn join_readers(
    stdout_reader: thread::JoinHandle<()>,
    stderr_reader: thread::JoinHandle<()>,
) -> Result<(), ClientError> {
    let stdout_result = join_reader(stdout_reader, StreamKind::Stdout);
    let stderr_result = join_reader(stderr_reader, StreamKind::Stderr);
    stdout_result.and(stderr_result)
}

fn capture_result(
    result: Result<CapturedStream, std::io::Error>,
    stream: StreamKind,
) -> Result<CapturedStream, ClientError> {
    result.map_err(|error| ClientError::Process {
        message: format!(
            "failed to read {stream}: {}",
            sanitize_text(error.to_string().as_bytes())
        ),
    })
}

#[cfg(unix)]
fn terminate_and_reap(child: &mut std::process::Child) -> Result<(), ClientError> {
    // A descendant retaining capture pipes remains in this PGID after its leader is reaped,
    // so POSIX keeps the group allocated and kill(-pgid) still targets that descendant.
    let process_group = child.id() as libc::pid_t;
    let kill_error = match unsafe { libc::kill(-process_group, libc::SIGKILL) } {
        0 => None,
        _ => {
            let error = std::io::Error::last_os_error();
            (error.raw_os_error() != Some(libc::ESRCH)).then_some(error)
        }
    };
    reap_after_termination(child, kill_error)
}

#[cfg(not(unix))]
fn terminate_and_reap(child: &mut std::process::Child) -> Result<(), ClientError> {
    reap_after_termination(child, child.kill().err())
}

fn reap_after_termination(
    child: &mut std::process::Child,
    kill_error: Option<std::io::Error>,
) -> Result<(), ClientError> {
    match child.wait() {
        Ok(_) => Ok(()),
        Err(wait_error) => {
            let detail = match kill_error {
                Some(kill_error) => {
                    format!("{kill_error}; then failed to reap child: {wait_error}")
                }
                None => format!("failed to reap terminated child: {wait_error}"),
            };
            Err(ClientError::Process {
                message: sanitize_text(detail.as_bytes()),
            })
        }
    }
}

fn sanitize_text(bytes: &[u8]) -> String {
    let stripped = strip_ansi_escapes::strip(bytes);
    String::from_utf8_lossy(&stripped)
        .chars()
        .filter(|character| !character.is_control() || *character == '\n')
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simultaneous_overflow_in_one_drain_prefers_stdout() {
        assert!(matches!(
            output_limit_error(true, true, 17, 23),
            Some(ClientError::OutputLimit {
                stream: StreamKind::Stdout,
                limit: 17,
            })
        ));
    }

    #[cfg(unix)]
    struct ReaderPanicGuard;

    #[cfg(unix)]
    impl Drop for ReaderPanicGuard {
        fn drop(&mut self) {
            READER_PANIC_RELEASE.store(true, Ordering::SeqCst);
            FORCE_READER_PANIC.store(false, Ordering::SeqCst);
        }
    }

    #[cfg(unix)]
    #[test]
    fn disconnected_reader_channel_reaps_child_and_returns_promptly() {
        let temp = tempfile::tempdir().expect("tempdir");
        let pid_file = temp.path().join("child.pid");
        FORCE_READER_PANIC.store(true, Ordering::SeqCst);
        READER_PANIC_RELEASE.store(false, Ordering::SeqCst);
        READER_PANIC_ENTERED.store(0, Ordering::SeqCst);
        let _guard = ReaderPanicGuard;

        let client = CliHerdrClient {
            bin_path: PathBuf::from("/bin/sh"),
            timeouts: ClientTimeouts {
                bootstrap_read: Duration::from_secs(5),
                open_or_action: Duration::from_secs(5),
                mutation: Duration::from_secs(5),
            },
        };
        let argv = vec![
            OsString::from("-c"),
            OsString::from("printf '%s\\n' \"$$\" > \"$1\"; exec sleep 30"),
            OsString::from("sh"),
            pid_file.as_os_str().to_owned(),
        ];
        let worker = thread::spawn(move || client.invoke(&argv, RequestClass::BootstrapRead));

        let deadline = Instant::now() + Duration::from_secs(1);
        while (!pid_file.exists() || READER_PANIC_ENTERED.load(Ordering::SeqCst) != 2)
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(pid_file.exists(), "child did not record its PID");
        assert_eq!(READER_PANIC_ENTERED.load(Ordering::SeqCst), 2);

        let started = Instant::now();
        READER_PANIC_RELEASE.store(true, Ordering::SeqCst);
        let error = match worker.join().expect("invocation thread") {
            Ok(_) => panic!("reader panic unexpectedly succeeded"),
            Err(error) => error,
        };
        assert!(started.elapsed() < Duration::from_millis(500), "{error}");
        assert!(matches!(error, ClientError::Process { .. }));
        let message = error.to_string();
        assert!(message.contains("stdout capture thread failed"));
        assert!(!message.chars().any(char::is_control));

        let pid = std::fs::read_to_string(&pid_file).expect("child PID");
        let status = Command::new("kill")
            .args(["-0", pid.trim()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("probe child");
        assert!(
            !status.success(),
            "reader-disconnected child is still alive"
        );
    }
}
