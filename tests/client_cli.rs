use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use herdr_command_palette::client::{
    CliHerdrClient, ClientError, ClientTimeouts, HerdrClient, HerdrOperation, OperationResponse,
    RequestClass, StreamKind,
};
use herdr_command_palette::model::{Direction, SplitDirection};
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn os(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn every_operation_has_exact_public_cli_argv_and_request_class() {
    let cases = vec![
        (
            HerdrOperation::WorkspaceFocus {
                workspace_id: "ws-1".into(),
            },
            os(&["workspace", "focus", "ws-1"]),
            RequestClass::Mutation,
        ),
        (
            HerdrOperation::WorkspaceCreate {
                label: "My Work".into(),
                cwd: Some(PathBuf::from("/tmp/a b")),
            },
            os(&[
                "workspace",
                "create",
                "--label",
                "My Work",
                "--cwd",
                "/tmp/a b",
                "--focus",
            ]),
            RequestClass::Mutation,
        ),
        (
            HerdrOperation::WorkspaceCreate {
                label: "plain".into(),
                cwd: None,
            },
            os(&["workspace", "create", "--label", "plain", "--focus"]),
            RequestClass::Mutation,
        ),
        (
            HerdrOperation::WorkspaceRename {
                workspace_id: "ws-1".into(),
                label: "New Name".into(),
            },
            os(&["workspace", "rename", "ws-1", "New Name"]),
            RequestClass::Mutation,
        ),
        (
            HerdrOperation::WorkspaceClose {
                workspace_id: "ws-1".into(),
            },
            os(&["workspace", "close", "ws-1"]),
            RequestClass::Mutation,
        ),
        (
            HerdrOperation::WorktreeOpen {
                workspace_id: "ws-1".into(),
                path: PathBuf::from("/tmp/tree one"),
            },
            os(&[
                "worktree",
                "open",
                "--workspace",
                "ws-1",
                "--path",
                "/tmp/tree one",
                "--focus",
                "--json",
            ]),
            RequestClass::Mutation,
        ),
        (
            HerdrOperation::WorktreeCreate {
                workspace_id: "ws-1".into(),
                branch: "feature/a".into(),
                base: "HEAD".into(),
                label: Some("Feature A".into()),
            },
            os(&[
                "worktree",
                "create",
                "--workspace",
                "ws-1",
                "--branch",
                "feature/a",
                "--base",
                "HEAD",
                "--label",
                "Feature A",
                "--focus",
                "--json",
            ]),
            RequestClass::Mutation,
        ),
        (
            HerdrOperation::WorktreeCreate {
                workspace_id: "ws-1".into(),
                branch: "feature/b".into(),
                base: "main".into(),
                label: None,
            },
            os(&[
                "worktree",
                "create",
                "--workspace",
                "ws-1",
                "--branch",
                "feature/b",
                "--base",
                "main",
                "--focus",
                "--json",
            ]),
            RequestClass::Mutation,
        ),
        (
            HerdrOperation::TabFocus {
                tab_id: "tab-1".into(),
            },
            os(&["tab", "focus", "tab-1"]),
            RequestClass::Mutation,
        ),
        (
            HerdrOperation::TabCreate {
                workspace_id: "ws-1".into(),
                label: "Review Tab".into(),
                cwd: Some(PathBuf::from("/tmp/review dir")),
            },
            os(&[
                "tab",
                "create",
                "--workspace",
                "ws-1",
                "--label",
                "Review Tab",
                "--cwd",
                "/tmp/review dir",
                "--focus",
            ]),
            RequestClass::Mutation,
        ),
        (
            HerdrOperation::TabCreate {
                workspace_id: "ws-1".into(),
                label: "Review".into(),
                cwd: None,
            },
            os(&[
                "tab",
                "create",
                "--workspace",
                "ws-1",
                "--label",
                "Review",
                "--focus",
            ]),
            RequestClass::Mutation,
        ),
        (
            HerdrOperation::TabRename {
                tab_id: "tab-1".into(),
                label: "New Tab".into(),
            },
            os(&["tab", "rename", "tab-1", "New Tab"]),
            RequestClass::Mutation,
        ),
        (
            HerdrOperation::TabClose {
                tab_id: "tab-1".into(),
            },
            os(&["tab", "close", "tab-1"]),
            RequestClass::Mutation,
        ),
        (
            HerdrOperation::PaneFocus {
                pane_id: "pane-1".into(),
                direction: Direction::Left,
            },
            os(&["pane", "focus", "--pane", "pane-1", "--direction", "left"]),
            RequestClass::Mutation,
        ),
        (
            HerdrOperation::PaneSplit {
                pane_id: "pane-1".into(),
                direction: SplitDirection::Right,
                cwd: Some(PathBuf::from("/tmp/a b")),
            },
            os(&[
                "pane",
                "split",
                "--pane",
                "pane-1",
                "--direction",
                "right",
                "--ratio",
                "0.5",
                "--cwd",
                "/tmp/a b",
                "--focus",
            ]),
            RequestClass::Mutation,
        ),
        (
            HerdrOperation::PaneSplit {
                pane_id: "pane-1".into(),
                direction: SplitDirection::Down,
                cwd: None,
            },
            os(&[
                "pane",
                "split",
                "--pane",
                "pane-1",
                "--direction",
                "down",
                "--ratio",
                "0.5",
                "--focus",
            ]),
            RequestClass::Mutation,
        ),
        (
            HerdrOperation::PaneZoomToggle {
                pane_id: "pane-1".into(),
            },
            os(&["pane", "zoom", "--pane", "pane-1", "--toggle"]),
            RequestClass::Mutation,
        ),
        (
            HerdrOperation::PaneClose {
                pane_id: "pane-1".into(),
            },
            os(&["pane", "close", "pane-1"]),
            RequestClass::Mutation,
        ),
        (
            HerdrOperation::PaneRun {
                pane_id: "pane-1".into(),
                command: "printf '%s' one two".into(),
            },
            os(&["pane", "run", "pane-1", "printf '%s' one two"]),
            RequestClass::Mutation,
        ),
        (
            HerdrOperation::AgentFocus {
                pane_id: "pane-1".into(),
            },
            os(&["agent", "focus", "pane-1"]),
            RequestClass::Mutation,
        ),
        (
            HerdrOperation::AgentPrompt {
                pane_id: "pane-1".into(),
                text: "keep  prompt spacing".into(),
            },
            os(&["agent", "prompt", "pane-1", "keep  prompt spacing"]),
            RequestClass::Mutation,
        ),
        (
            HerdrOperation::AgentRename {
                pane_id: "pane-1".into(),
                name: Some("review pair #2".into()),
            },
            os(&["agent", "rename", "pane-1", "review pair #2"]),
            RequestClass::Mutation,
        ),
        (
            HerdrOperation::AgentRename {
                pane_id: "pane-1".into(),
                name: None,
            },
            os(&["agent", "rename", "pane-1", "--clear"]),
            RequestClass::Mutation,
        ),
        (
            HerdrOperation::AgentStart {
                name: "reviewer one".into(),
                kind: "claude".into(),
                pane_id: "pane-1".into(),
            },
            os(&[
                "agent",
                "start",
                "reviewer one",
                "--kind",
                "claude",
                "--pane",
                "pane-1",
                "--timeout",
                "30000",
            ]),
            RequestClass::Mutation,
        ),
        (
            HerdrOperation::PluginActionInvoke {
                action_id: "demo.test.run".into(),
            },
            os(&["plugin", "action", "invoke", "demo.test.run"]),
            RequestClass::OpenOrAction,
        ),
    ];

    for (operation, expected_argv, expected_class) in cases {
        assert_eq!(operation.argv(), expected_argv, "{operation:?}");
        assert_eq!(operation.request_class(), expected_class, "{operation:?}");
    }

    assert_eq!(
        RequestClass::BootstrapRead.default_timeout(),
        Duration::from_secs(3)
    );
    assert_eq!(
        RequestClass::OpenOrAction.default_timeout(),
        Duration::from_secs(5)
    );
    assert_eq!(
        RequestClass::Mutation.default_timeout(),
        Duration::from_secs(30)
    );
}

struct FakeEnvironment {
    _lock: MutexGuard<'static, ()>,
    temp: TempDir,
}

impl FakeEnvironment {
    fn new(mode: &str) -> Self {
        let lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().expect("tempdir");
        std::env::set_var("HERDR_BIN_PATH", fixture("fake-herdr.sh"));
        std::env::set_var("FAKE_HERDR_MODE", mode);
        std::env::remove_var("FAKE_HERDR_STDOUT_FILE");
        std::env::remove_var("FAKE_HERDR_PID_FILE");
        std::env::set_var("FAKE_HERDR_ARGV_LOG", temp.path().join("argv"));
        Self { _lock: lock, temp }
    }

    fn response(&self, contents: &str) {
        let path = self.temp.path().join("response.json");
        fs::write(&path, contents).expect("write fake response");
        std::env::set_var("FAKE_HERDR_STDOUT_FILE", path);
    }

    fn argv(&self) -> Vec<Vec<u8>> {
        fs::read(self.temp.path().join("argv"))
            .expect("read argv log")
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .map(<[u8]>::to_vec)
            .collect()
    }

    fn client(&self) -> CliHerdrClient {
        self.client_with_timeout(Duration::from_secs(2))
    }

    fn client_with_timeout(&self, timeout: Duration) -> CliHerdrClient {
        CliHerdrClient::from_env_with_timeouts(ClientTimeouts {
            bootstrap_read: timeout,
            open_or_action: timeout,
            mutation: timeout,
        })
        .expect("fake client")
    }

    fn recorded_pids(&self) -> Vec<String> {
        fs::read_to_string(self.temp.path().join("pid"))
            .expect("read fake pids")
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

#[cfg(unix)]
fn assert_pids_are_gone(pids: &[String]) {
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        if pids.iter().all(|pid| {
            !std::process::Command::new("kill")
                .args([OsStr::new("-0"), OsStr::new(pid)])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .expect("probe process")
                .success()
        }) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("processes remain alive: {pids:?}");
}

impl Drop for FakeEnvironment {
    fn drop(&mut self) {
        std::env::remove_var("HERDR_BIN_PATH");
        std::env::remove_var("FAKE_HERDR_MODE");
        std::env::remove_var("FAKE_HERDR_STDOUT_FILE");
        std::env::remove_var("FAKE_HERDR_PID_FILE");
        std::env::remove_var("FAKE_HERDR_ARGV_LOG");
    }
}

#[test]
fn read_methods_use_exact_argv_and_typed_response_envelopes() {
    let fake = FakeEnvironment::new("fixture");
    let mut client = fake.client();

    fake.response(include_str!("fixtures/api-schema-0.7.4.json"));
    let capabilities = client.api_capabilities().expect("capabilities");
    assert!(!capabilities.agent_prompt);
    assert_eq!(fake.argv(), [b"api".as_slice(), b"schema", b"--json"]);

    fake.response(include_str!("fixtures/session-snapshot.json"));
    let snapshot = client.session_snapshot().expect("snapshot");
    assert_eq!(snapshot.workspaces.len(), 2);
    assert_eq!(fake.argv(), [b"api".as_slice(), b"snapshot"]);

    fake.response(include_str!("fixtures/worktree-list.json"));
    let worktrees = client.worktree_list("ws one").expect("worktrees");
    assert_eq!(worktrees.len(), 2);
    assert_eq!(
        fake.argv(),
        [
            b"worktree".as_slice(),
            b"list",
            b"--workspace",
            b"ws one",
            b"--json"
        ]
    );

    fake.response(include_str!("fixtures/plugin-list.json"));
    let plugins = client.plugin_list().expect("plugins");
    assert!(!plugins.is_empty());
    assert_eq!(fake.argv(), [b"plugin".as_slice(), b"list", b"--json"]);

    fake.response(include_str!("fixtures/plugin-actions.json"));
    let actions = client.plugin_action_list().expect("actions");
    assert!(!actions.is_empty());
    assert_eq!(fake.argv(), [b"plugin".as_slice(), b"action", b"list"]);
}

#[test]
fn direct_invocation_preserves_spaced_user_values_as_one_argument() {
    let fake = FakeEnvironment::new("fixture");
    fake.response(r#"{"id":"fake","result":{"type":"workspace_created"}}"#);
    let mut client = fake.client();

    let result = client.dispatch(&HerdrOperation::WorkspaceCreate {
        label: "My Work".into(),
        cwd: Some(PathBuf::from("/tmp/a b")),
    });

    assert_eq!(result.expect("dispatch"), OperationResponse::Empty);
    assert_eq!(
        fake.argv(),
        [
            b"workspace".as_slice(),
            b"create",
            b"--label",
            b"My Work",
            b"--cwd",
            b"/tmp/a b",
            b"--focus"
        ]
    );
}

#[test]
fn response_codecs_distinguish_json_pane_creation_and_empty_success() {
    let fake = FakeEnvironment::new("fixture");
    let mut client = fake.client();

    fake.response(
        r#"{"id":"fake","result":{"type":"pane_info","pane":{"pane_id":"new-pane","terminal_id":"term-new","workspace_id":"ws-1","tab_id":"tab-1","focused":true,"agent_status":"idle","revision":1}}}"#,
    );
    let response = client
        .dispatch(&HerdrOperation::PaneSplit {
            pane_id: "pane-1".into(),
            direction: SplitDirection::Right,
            cwd: None,
        })
        .expect("pane split");
    assert_eq!(
        response,
        OperationResponse::PaneCreated {
            pane_id: "new-pane".into()
        }
    );

    std::env::set_var("FAKE_HERDR_MODE", "malformed-json");
    let response = client
        .dispatch(&HerdrOperation::PaneRun {
            pane_id: "new-pane".into(),
            command: "echo one two".into(),
        })
        .expect("pane run uses empty codec");
    assert_eq!(response, OperationResponse::Empty);

    std::env::set_var("FAKE_HERDR_MODE", "fixture");
    fake.response(r#"{"id":"fake","result":{"type":"agent_renamed"}}"#);
    assert_eq!(
        client
            .dispatch(&HerdrOperation::AgentRename {
                pane_id: "pane-1".into(),
                name: Some("review pair #2".into()),
            })
            .expect("agent rename"),
        OperationResponse::Empty
    );
    assert_eq!(
        fake.argv(),
        [b"agent".as_slice(), b"rename", b"pane-1", b"review pair #2"]
    );
}

#[test]
fn open_palette_uses_exact_argv() {
    let fake = FakeEnvironment::new("fixture");
    fake.response(r#"{"id":"fake","result":{"type":"plugin_pane_opened"}}"#);
    let mut client = fake.client();

    client.open_palette().expect("open palette");

    assert_eq!(
        fake.argv(),
        [
            b"plugin".as_slice(),
            b"pane",
            b"open",
            b"--plugin",
            b"herdr.command-palette",
            b"--entrypoint",
            b"palette"
        ]
    );
}

#[test]
fn nonzero_api_envelopes_preserve_typed_codes() {
    let fake = FakeEnvironment::new("api-error");
    let mut client = fake.client();

    let error = client
        .dispatch(&HerdrOperation::PaneClose {
            pane_id: "gone".into(),
        })
        .expect_err("API failure");

    assert_eq!(error.code(), Some("pane_not_found"));
    assert!(error.to_string().contains("pane vanished"));
}

#[test]
fn nonzero_plain_stderr_does_not_parse_json_looking_stdout_as_an_api_error() {
    let fake = FakeEnvironment::new("json-stdout-plain-stderr-error");
    let mut client = fake.client();

    let error = client
        .dispatch(&HerdrOperation::PaneClose {
            pane_id: "gone".into(),
        })
        .expect_err("nonzero exit");

    assert!(matches!(error, ClientError::Exit { .. }));
    assert_eq!(error.code(), None);
    assert!(error.to_string().contains("public stderr failure"));
    assert!(!error.to_string().contains("stdout_only_code"));
}

#[test]
fn malformed_json_is_rejected_only_for_json_codecs() {
    let fake = FakeEnvironment::new("malformed-json");
    let mut client = fake.client();

    let error = client.session_snapshot().expect_err("malformed response");
    assert!(error.to_string().contains("invalid JSON"));
}

#[test]
fn stderr_is_stripped_of_ansi_osc_and_raw_controls() {
    let fake = FakeEnvironment::new("ansi-error");
    let mut client = fake.client();

    let message = client.plugin_list().expect_err("failure").to_string();

    assert!(message.contains("failed"));
    assert!(message.contains("link"));
    assert!(!message.contains('\u{1b}'));
    assert!(!message.chars().any(|character| character == '\u{1}'));
    assert!(!message.contains("https://example.invalid"));
}

#[test]
fn stdout_and_stderr_have_separate_hard_caps() {
    let fake = FakeEnvironment::new("over-json-stdout");
    let mut client = fake.client();
    let stdout_error = client.session_snapshot().expect_err("oversized stdout");
    assert!(matches!(
        stdout_error,
        ClientError::OutputLimit {
            stream: StreamKind::Stdout,
            limit: 4_194_304,
        }
    ));
    assert!(stdout_error
        .to_string()
        .contains("4194304-byte capture limit"));

    std::env::set_var("FAKE_HERDR_MODE", "large-stderr");
    let stderr_error = client.session_snapshot().expect_err("oversized stderr");
    assert!(matches!(
        stderr_error,
        ClientError::OutputLimit {
            stream: StreamKind::Stderr,
            limit: 262_144,
        }
    ));
    assert!(stderr_error
        .to_string()
        .contains("262144-byte capture limit"));
}

#[test]
fn simultaneous_oversized_stdout_and_stderr_are_drained_and_capped() {
    let fake = FakeEnvironment::new("large-both");
    let mut client = fake.client();

    let error = client
        .session_snapshot()
        .expect_err("simultaneous oversized streams");

    assert!(matches!(
        error.stream_kind(),
        Some(StreamKind::Stdout | StreamKind::Stderr)
    ));
}

#[test]
fn plugin_action_list_accepts_json_larger_than_stderr_diagnostic_cap() {
    let fake = FakeEnvironment::new("fixture");
    fake.response(include_str!("fixtures/plugin-actions-large.json"));
    let mut client = fake.client();

    let actions = client.plugin_action_list().expect("large action list");
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].action_id, "large");
}

#[test]
fn over_four_mib_json_stdout_fails_promptly() {
    let fake = FakeEnvironment::new("over-json-stdout");
    let mut client = fake.client_with_timeout(Duration::from_secs(5));
    let started = Instant::now();

    let error = client
        .session_snapshot()
        .expect_err("oversized JSON stdout");

    assert_eq!(error.stream_kind(), Some(StreamKind::Stdout));
    // Draining more than 4 MiB can be delayed by a loaded test scheduler; two
    // seconds remains well below this request's five-second deadline.
    assert!(started.elapsed() < Duration::from_secs(2), "{error}");
}

#[cfg(unix)]
#[test]
fn timeout_kills_process_group_and_returns_promptly_when_descendant_keeps_pipes_open() {
    let fake = FakeEnvironment::new("fork-retain-pipes");
    let pid_file = fake.temp.path().join("pid");
    std::env::set_var("FAKE_HERDR_PID_FILE", &pid_file);
    let mut client = fake.client_with_timeout(Duration::from_millis(40));
    let started = Instant::now();

    let error = client.plugin_list().expect_err("timeout");

    assert_eq!(error.request_class(), Some(RequestClass::BootstrapRead));
    assert!(started.elapsed() < Duration::from_millis(500), "{error}");
    let pids = fake.recorded_pids();
    assert_eq!(pids.len(), 2);
    assert_pids_are_gone(&pids);
}

#[cfg(unix)]
#[test]
fn timeout_kills_descendant_when_direct_child_has_already_exited() {
    let fake = FakeEnvironment::new("leader-exits-retain-pipes");
    let pid_file = fake.temp.path().join("pid");
    std::env::set_var("FAKE_HERDR_PID_FILE", &pid_file);
    let mut client = fake.client_with_timeout(Duration::from_millis(40));
    let started = Instant::now();

    let error = client.plugin_list().expect_err("timeout");

    assert_eq!(error.request_class(), Some(RequestClass::BootstrapRead));
    assert!(started.elapsed() < Duration::from_millis(500), "{error}");
    assert_pids_are_gone(&fake.recorded_pids());
}

#[cfg(unix)]
#[test]
fn infinite_output_terminates_process_group_before_request_deadline() {
    let fake = FakeEnvironment::new("infinite-output");
    let pid_file = fake.temp.path().join("pid");
    std::env::set_var("FAKE_HERDR_PID_FILE", &pid_file);
    let mut client = fake.client_with_timeout(Duration::from_secs(5));
    let started = Instant::now();

    let error = client.plugin_list().expect_err("output limit");

    assert_eq!(error.stream_kind(), Some(StreamKind::Stdout));
    // Keep a meaningful promptness check while allowing scheduler contention
    // below the configured five-second request deadline.
    assert!(started.elapsed() < Duration::from_secs(2), "{error}");
    let pids = fake.recorded_pids();
    assert_eq!(pids.len(), 2);
    assert_pids_are_gone(&pids);
}

#[test]
fn timeout_kills_and_reaps_the_child() {
    let fake = FakeEnvironment::new("hang");
    let pid_file = fake.temp.path().join("pid");
    std::env::set_var("FAKE_HERDR_PID_FILE", &pid_file);
    let mut client = CliHerdrClient::from_env_with_timeouts(ClientTimeouts {
        bootstrap_read: Duration::from_millis(40),
        open_or_action: Duration::from_millis(40),
        mutation: Duration::from_millis(40),
    })
    .expect("fake client");

    let error = client.plugin_list().expect_err("timeout");
    assert_eq!(error.request_class(), Some(RequestClass::BootstrapRead));

    let pid = fs::read_to_string(pid_file)
        .expect("fake pid")
        .trim()
        .to_owned();
    let status = std::process::Command::new("kill")
        .args([OsStr::new("-0"), OsStr::new(&pid)])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("probe child");
    assert!(!status.success(), "timed-out child {pid} is still alive");
}

#[test]
fn missing_or_empty_herdr_bin_path_is_a_typed_error() {
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::remove_var("HERDR_BIN_PATH");
    assert!(CliHerdrClient::from_env().is_err());
    std::env::set_var("HERDR_BIN_PATH", "");
    assert!(CliHerdrClient::from_env().is_err());
    std::env::remove_var("HERDR_BIN_PATH");
}
