# Rename Focused Pane Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a command-palette option that assigns or clears the custom label of the exact pane captured as focused.

**Architecture:** Introduce a first-class `CoreCommand::RenameFocusedPane` backed by a typed `HerdrOperation::PaneRename`. Reuse the existing optional trimmed text form and exact-ID stale retry machinery, with `pane_not_found` as the only refreshable error.

**Tech Stack:** Rust 1.88, serde/serde_json, crossterm, ratatui, Herdr 0.7.4 CLI, built-in Rust tests.

## Global Constraints

- Work only in `/Users/sendhil/src/herdr-command-palette-rename-pane` on branch `feat/rename-focused-pane`.
- Keep pane-label rename separate from focused-agent rename.
- Prefill only a nonempty trimmed `PaneInfo.label`; never fall back to terminal titles, agent metadata, or pane ID.
- Trim submitted labels; blank or whitespace-only input clears the custom label.
- Always target the exact pane ID captured in the runtime snapshot.
- Refresh once only for `pane_not_found`; retry only if that exact pane ID survives.
- Add no dependencies, keybindings, plugin actions, Herdr changes, or minimum-version change.

---

### Task 1: Add the pane rename command and form contract

**Files:**
- Modify: `src/command.rs:6-53`
- Modify: `src/command.rs:278-750`
- Test: `src/command.rs:785-1164`

**Interfaces:**
- Consumes: `RuntimeSnapshot::focused_pane() -> Option<&PaneInfo>` and `PaneInfo.label: Option<String>`.
- Produces: `CoreCommand::RenameFocusedPane`, stable ID `pane.rename`, and one optional trimmed `ArgumentKey::Label` step.

- [ ] **Step 1: Write failing catalog and form tests**

Rename `core_catalog_is_the_exact_twenty_three_item_contract` to `core_catalog_is_the_exact_twenty_four_item_contract` and insert this expected row after `pane.zoom-toggle`:

```rust
(
    "pane.rename",
    "Rename focused pane",
    "Rename the focused pane",
    CommandCategory::Pane,
    &["pane name", "rename current pane"][..],
    20,
    &[(ArgumentKey::Label, ArgumentKind::Text, true)][..],
),
```

Add this focused test next to the focused-agent rename test:

```rust
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
```

Also add to `dynamic_steps_contain_exact_choices_prompts_and_defaults`:

```rust
assert_eq!(
    CoreCommand::RenameFocusedPane.spec(&runtime).unwrap().steps[0].default(),
    Some(&ArgumentDefault::Value("agent".into()))
);
```

- [ ] **Step 2: Run the focused command tests and verify RED**

Run:

```bash
cargo test --locked command::tests::core_catalog_is_the_exact_twenty_four_item_contract
cargo test --locked command::tests::rename_focused_pane_requires_a_pane_and_uses_only_its_custom_label
```

Expected: compilation fails because `CoreCommand::RenameFocusedPane` does not exist.

- [ ] **Step 3: Implement the command variant and exact metadata**

Add `RenameFocusedPane` after `TogglePaneZoom` in `CoreCommand`, and in the same position in `CORE_COMMANDS`; change its array length from `23` to `24`.

Add these match arms:

```rust
// stable_id
Self::RenameFocusedPane => "pane.rename",

// metadata
Self::RenameFocusedPane => (
    "Rename focused pane",
    "Rename the focused pane",
    CommandCategory::Pane,
    &["pane name", "rename current pane"],
    20,
),

// is_available
Self::RenameFocusedPane => focused_pane.is_some(),

// steps
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
```

Add `CoreCommand::RenameFocusedPane` to the sparse-runtime availability assertion that becomes unavailable when `focused_pane_id` is absent. Extend the text-normalization test with:

```rust
let rename_pane = &CoreCommand::RenameFocusedPane.spec(&runtime).unwrap().steps[0];
assert_eq!(rename_pane.normalize_text("  build logs  ").unwrap(), Some("build logs".into()));
assert_eq!(rename_pane.normalize_text(" \t ").unwrap(), None);
```

- [ ] **Step 4: Run the command tests and verify GREEN**

Run:

```bash
cargo test --locked command::tests
```

Expected: all `command::tests` pass.

- [ ] **Step 5: Commit the command contract**

```bash
git add src/command.rs
git commit -m "feat: add focused pane rename command"
```

---

### Task 2: Add the typed Herdr pane rename operation

**Files:**
- Modify: `src/client.rs:109-378`
- Test: `tests/client_cli.rs:17-315`

**Interfaces:**
- Consumes: an exact pane ID and an optional normalized label.
- Produces: `HerdrOperation::PaneRename { pane_id: String, label: Option<String> }`, classified as `RequestClass::Mutation` with JSON acknowledgement decoding.

- [ ] **Step 1: Write failing exact-argv tests**

Insert these cases after `PaneZoomToggle` in `every_operation_has_exact_public_cli_argv_and_request_class`:

```rust
(
    HerdrOperation::PaneRename {
        pane_id: "pane-1".into(),
        label: Some("build logs".into()),
    },
    os(&["pane", "rename", "pane-1", "build logs"]),
    RequestClass::Mutation,
),
(
    HerdrOperation::PaneRename {
        pane_id: "pane-1".into(),
        label: None,
    },
    os(&["pane", "rename", "pane-1", "--clear"]),
    RequestClass::Mutation,
),
```

- [ ] **Step 2: Run the client test and verify RED**

Run:

```bash
cargo test --locked --test client_cli every_operation_has_exact_public_cli_argv_and_request_class
```

Expected: compilation fails because `HerdrOperation::PaneRename` does not exist.

- [ ] **Step 3: Implement the operation and codecs**

Add this enum variant after `PaneZoomToggle`:

```rust
PaneRename {
    pane_id: String,
    label: Option<String>,
},
```

Add this `argv` arm after pane zoom:

```rust
Self::PaneRename { pane_id, label } => match label {
    Some(label) => with_values(["pane", "rename"], [pane_id.as_str(), label.as_str()]),
    None => with_values(["pane", "rename"], [pane_id.as_str(), "--clear"]),
},
```

Include `Self::PaneRename { .. }` in the mutation group in `request_class` and the JSON-acknowledgement group in `response_codec`.

- [ ] **Step 4: Run client tests and verify GREEN**

Run:

```bash
cargo test --locked --test client_cli
```

Expected: all `client_cli` tests pass.

- [ ] **Step 5: Commit the client boundary**

```bash
git add src/client.rs tests/client_cli.rs
git commit -m "feat: dispatch pane rename operations"
```

---

### Task 3: Map form values and enforce exact-ID stale retry

**Files:**
- Modify: `src/executor.rs:369-770`
- Test: `src/executor.rs:823-2041`

**Interfaces:**
- Consumes: `CoreCommand::RenameFocusedPane`, `ArgumentKey::Label`, and `RuntimeSnapshot::focused_pane()`.
- Produces: one `PaneRename` operation and a retry policy keyed only by `StableTarget::Pane(original_pane_id)`.

- [ ] **Step 1: Write failing operation-mapping tests**

Add this case after `TogglePaneZoom` in `every_core_identity_maps_to_the_exact_typed_operation`:

```rust
(
    CoreCommand::RenameFocusedPane,
    args(&[(ArgumentKey::Label, text("  build logs  "))]),
    vec![HerdrOperation::PaneRename {
        pane_id: "pane-a".into(),
        label: Some("build logs".into()),
    }],
),
```

Add this test beside the focused-agent blank mapping test:

```rust
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
```

- [ ] **Step 2: Run mapping tests and verify RED**

Run:

```bash
cargo test --locked executor::tests::every_core_identity_maps_to_the_exact_typed_operation
cargo test --locked executor::tests::rename_focused_pane_maps_blank_to_clear_and_requires_the_focused_pane
```

Expected: exhaustive matches fail until the executor handles the new command and operation.

- [ ] **Step 3: Implement operation mapping**

Add this `core_operations` arm after pane zoom:

```rust
CoreCommand::RenameFocusedPane => Ok(vec![HerdrOperation::PaneRename {
    pane_id: pane_id()?,
    label: optional_trimmed(values, ArgumentKey::Label, "pane label")?,
}]),
```

Add `HerdrOperation::PaneRename { pane_id, .. }` to the pane arm in `stable_target`, and map `CoreCommand::RenameFocusedPane` to `StableTargetKind::Pane`.

Add `CoreCommand::RenameFocusedPane` to these retry matches:

```rust
// is_refreshable_stale
CoreCommand::RenameFocusedPane => code == "pane_not_found",

// retryable
CoreCommand::RenameFocusedPane

// retry_target_still_valid
(CoreCommand::RenameFocusedPane, StableTarget::Pane(id)) => refreshed
    .session
    .panes
    .iter()
    .any(|pane| pane.pane_id == *id),
```

Every exhaustive nonretryable/fallback match in `src/executor.rs` must place `RenameFocusedPane` in the pane-retryable family, not in a generic nonretryable family.

- [ ] **Step 4: Write failing stale-target tests**

Add these three tests:

```rust
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
    refreshed.session.panes.retain(|pane| pane.pane_id != "pane-a");
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
        ExecutionOutcome::Failed { refreshed: Some(ref value), .. } if value == &refreshed
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
        ExecutionOutcome::Failed { refreshed: None, .. }
    ));
    assert_eq!(executor.client().operations.len(), 1);
    assert!(executor.client().refresh_calls.is_empty());
}
```

Insert this tuple in every table-driven retryable-family case array:

```rust
(
    CoreCommand::RenameFocusedPane,
    args(&[(ArgumentKey::Label, text("build logs"))]),
    "pane_not_found",
),
```

Place `RenameFocusedPane` beside `TogglePaneZoom` in every exhaustive retryable command arm; it must not appear in a nonretryable arm.

- [ ] **Step 5: Run executor tests and verify GREEN**

Run:

```bash
cargo test --locked executor::tests
```

Expected: all `executor::tests` pass, including exact-ID retry, changed-target refusal, wrong-code refusal, and second-failure refusal.

- [ ] **Step 6: Commit executor behavior**

```bash
git add src/executor.rs
git commit -m "feat: execute focused pane rename safely"
```

---

### Task 4: Prove palette behavior and document the feature

**Files:**
- Modify: `tests/palette_flow.rs:1-546`
- Modify: `README.md:5-130`
- Modify: `CHANGELOG.md:1-5`

**Interfaces:**
- Consumes: the command, form, operation, and retry contracts from Tasks 1-3.
- Produces: end-to-end user behavior coverage and user-facing documentation.

- [ ] **Step 1: Write failing palette-flow rename and clear test**

Add this test before the focused-agent rename flow test:

```rust
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
```

- [ ] **Step 2: Write failing visible-failure test**

Add this second test:

```rust
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
```

- [ ] **Step 3: Run palette flow and verify RED or incomplete coverage**

Run:

```bash
cargo test --locked --test palette_flow rename_focused_pane
```

Expected before all implementation is present: tests fail to compile or fail their operation assertions. After Tasks 1-3, they may already pass; in that case this step proves the end-to-end behavior before documentation changes.

- [ ] **Step 4: Update user documentation**

Change the Features bullet to:

```markdown
- Rename the focused pane or the agent in it from the palette.
```

Insert before `## Focused-agent rename behavior`:

```markdown
## Focused-pane rename behavior

`Rename focused pane` appears whenever the runtime snapshot has a focused pane.
Its field starts with that pane's current custom label. Submit a nonempty value
to rename that exact pane, or submit an empty field to clear its custom label.
This is separate from the agent name managed by `Rename focused agent`.
```

Prepend to `CHANGELOG.md`:

```markdown
## [Unreleased]

### Added

- A focused-pane rename command that can assign or clear the pane's custom label
  without changing its agent name.

```

- [ ] **Step 5: Run focused tests and verify GREEN**

Run:

```bash
cargo test --locked --test palette_flow rename_focused_pane
cargo test --locked --test client_cli every_operation_has_exact_public_cli_argv_and_request_class
cargo test --locked command::tests
cargo test --locked executor::tests
```

Expected: all commands pass.

- [ ] **Step 6: Run complete validation**

Run:

```bash
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
```

Expected: formatting is clean, Clippy emits no warnings, and every test passes.

- [ ] **Step 7: Verify diagnostics and scope**

Run Pi diagnostics over the edited Rust files, then inspect:

```bash
git diff --check
git status --short
git diff --stat master...HEAD
git log --oneline master..HEAD
```

Expected: no blocking diagnostics, no whitespace errors, only the planned source/test/docs files changed, and no staged files remain.

- [ ] **Step 8: Commit integration tests and docs**

```bash
git add tests/palette_flow.rs README.md CHANGELOG.md
git commit -m "test: cover focused pane rename flow"
```

- [ ] **Step 9: Re-run final validation after the commit**

```bash
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
git status --short
```

Expected: all commands succeed and the worktree is clean.
