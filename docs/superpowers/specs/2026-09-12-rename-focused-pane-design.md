# Rename Focused Pane Design

## Goal

Add a first-class command-palette option that renames the pane that was focused when the palette captured its runtime snapshot. The command also allows the user to clear that pane's custom label.

## User experience

The palette includes a Pane-category command titled **Rename focused pane** with the description **Rename the focused pane** and search aliases **pane name** and **rename current pane**.

The command is available only when the runtime snapshot contains a valid focused pane. Selecting it opens the palette's existing bounded text form:

- The field is prefilled with the focused pane's custom `PaneInfo.label` when that label is present and nonempty after trimming.
- Terminal titles, agent names, agent display names, and pane IDs are not used as fallback field values because they are not the custom pane label being edited.
- Submitting nonempty text trims surrounding whitespace and assigns the resulting custom label.
- Submitting an empty or whitespace-only field clears the custom label.

The existing **Rename focused agent** command remains available independently when the focused pane has an agent. Pane labels and agent names are separate Herdr fields and neither command modifies the other.

## Architecture

### Command catalog and form

Add `CoreCommand::RenameFocusedPane` to the fixed core command list in `src/command.rs`. Its stable ID is `pane.rename`, category is `CommandCategory::Pane`, and its priority is `20`, placing it with other non-destructive pane operations.

Its single argument uses the existing optional, trimmed `ArgumentKey::Label` text step. The default is derived only from the focused `PaneInfo.label`. Reusing the optional text rule naturally maps blank input to a clear operation while preserving the palette's current bounded-input and rendering behavior.

### Typed operation and CLI boundary

Add this operation to `HerdrOperation` in `src/client.rs`:

```rust
PaneRename {
    pane_id: String,
    label: Option<String>,
}
```

CLI argument generation maps it exactly as follows:

- `Some(label)` → `herdr pane rename <pane_id> <label>`
- `None` → `herdr pane rename <pane_id> --clear`

The operation is a mutation and expects Herdr's standard JSON acknowledgement, matching workspace, tab, and agent rename operations. Herdr 0.7.4 already exposes both `pane rename` and the nullable `pane.rename` API label, so no capability gate or minimum-version change is required.

### Execution and target safety

`core_operations` in `src/executor.rs` resolves the target from `RuntimeSnapshot::focused_pane()` and never from a display label or a fresh focus lookup. It constructs `PaneRename` with the exact captured `pane_id` and the optional trimmed label.

Pane rename follows the same stale-target policy as focused-agent rename:

1. If Herdr returns `pane_not_found`, refresh the runtime snapshot once.
2. Retry only if the exact original pane ID still exists in the refreshed snapshot.
3. Never retarget a replacement pane with the same label.
4. Do not retry transport errors, unrelated API errors, or a second failure.

A failed rename keeps the argument form open, preserves the submitted text, and displays the existing bounded error treatment.

## Files and responsibilities

- `src/command.rs`: declare the command, catalog metadata, availability rule, and label-prefilled argument form.
- `src/client.rs`: declare `PaneRename`, generate exact CLI arguments, and classify its request and response.
- `src/executor.rs`: map form values to the captured focused pane and integrate exact-ID stale retry.
- `tests/client_cli.rs`: verify label and clear CLI forms plus mutation classification.
- `tests/palette_flow.rs`: verify palette prefill, rename, clear, and visible failure behavior end to end.
- `README.md`: document focused-pane rename behavior and distinguish it from focused-agent rename.
- `CHANGELOG.md`: record the user-visible addition under a new Unreleased section.

Existing unit tests in `src/command.rs`, `src/client.rs`, and `src/executor.rs` are updated where they enforce exhaustive command and operation contracts.

## Testing strategy

Implementation follows test-driven development:

1. Extend command-contract tests to require the new catalog identity, focused-pane availability, and custom-label-only prefill.
2. Extend client tests to require exact argv for assigning and clearing a pane label.
3. Extend executor tests to require exact captured-pane targeting and the bounded `pane_not_found` refresh/retry policy.
4. Add palette-flow coverage proving the form prefills, renames, clears, and retains submitted text on failure.
5. Run formatting, Clippy with warnings denied, and the full locked test suite.

The implementation is complete when:

- A focused pane exposes **Rename focused pane**.
- A missing focused pane hides it.
- Existing custom labels are prefilled without fallback substitutions.
- Nonempty input renames the captured pane and blank input clears it.
- Stale refresh never changes the stable pane target.
- Agent rename behavior remains unchanged.
- `cargo fmt --check`, `cargo clippy --locked --all-targets --all-features -- -D warnings`, and `cargo test --locked` pass.

## Out of scope

- Renaming an arbitrary pane selected from a list.
- Combining pane-label and agent-name editing.
- Adding a new keybinding or plugin manifest action.
- Refactoring workspace, tab, pane, and agent rename commands into a generic abstraction.
- Changing Herdr itself or supporting versions older than 0.7.4.
