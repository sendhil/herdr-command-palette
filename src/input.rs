use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};

use crate::{
    command::CommandId,
    registry::{PaletteCatalog, PaletteItem, PaletteItemId},
    state::{Interaction, LoadingState, PaletteState, StateAction},
    view::compute_layout,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputAction {
    None,
    Redraw,
    ExecuteCommand(CommandId),
    FocusEntity(PaletteItemId),
    RetryBootstrap,
    Close,
}

pub fn handle_key(
    state: &mut PaletteState,
    registry: &PaletteCatalog,
    key: KeyEvent,
    area: Rect,
) -> InputAction {
    if key.kind == KeyEventKind::Release {
        return InputAction::None;
    }

    match state.loading {
        LoadingState::Loading => {
            return if key.code == KeyCode::Esc {
                InputAction::Close
            } else {
                InputAction::None
            };
        }
        LoadingState::Failed(_) => {
            return match (key.code, key.modifiers) {
                (KeyCode::Esc, _) => InputAction::Close,
                (KeyCode::Char('r'), KeyModifiers::NONE) => InputAction::RetryBootstrap,
                _ => InputAction::None,
            };
        }
        LoadingState::Fatal(_) => {
            return if key.code == KeyCode::Esc {
                InputAction::Close
            } else {
                InputAction::None
            };
        }
        LoadingState::Ready => {}
    }

    if key.modifiers.contains(KeyModifiers::SUPER) {
        if !key.modifiers.contains(KeyModifiers::CONTROL) {
            if let KeyCode::Char(digit @ '1'..='9') = key.code {
                return activate_visible_shortcut(state, registry, area, digit);
            }
        }
        return InputAction::None;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('p') | KeyCode::Char('P') => move_up(state, registry, area),
            KeyCode::Char('n') | KeyCode::Char('N') => move_down(state, registry, area),
            _ => InputAction::None,
        };
    }

    match key.code {
        KeyCode::Backspace => edit_backspace(state, registry),
        KeyCode::Delete => edit_delete(state, registry),
        KeyCode::Left => move_cursor(state, TextBufferDirection::Left),
        KeyCode::Right => move_cursor(state, TextBufferDirection::Right),
        KeyCode::Up => move_up(state, registry, area),
        KeyCode::Down => move_down(state, registry, area),
        KeyCode::Enter => match state.interaction {
            Interaction::Search => activate_selected(state, registry, area),
            Interaction::Form(_) => map_state_action(state.submit()),
        },
        KeyCode::Esc => map_state_action(state.cancel_or_close()),
        KeyCode::Char(character) if !character.is_control() => {
            let input = character.to_string();
            if matches!(state.interaction, Interaction::Search) {
                if !state.insert_query(&input) {
                    return InputAction::None;
                }
                // Rank the one mutated query directly; do not clone and segment a speculative
                // full-query insertion before updating state.
                state.set_ranked(registry.rank(state.query.text()));
            } else if !state.insert_text(&input) {
                return InputAction::None;
            }
            InputAction::Redraw
        }
        _ => InputAction::None,
    }
}

pub fn handle_mouse(
    state: &mut PaletteState,
    registry: &PaletteCatalog,
    event: MouseEvent,
    area: Rect,
) -> InputAction {
    if !matches!(state.loading, LoadingState::Ready) {
        return InputAction::None;
    }
    let layout = compute_layout(area, state, registry);
    let action_rows: Vec<_> = layout.action_rows().collect();
    let action_count = action_rows.len();
    match event.kind {
        MouseEventKind::Moved => row_at(&action_rows, event).map_or(InputAction::None, |index| {
            let before = selection_state(state);
            select_row(state, index, action_count);
            if selection_state(state) == before {
                InputAction::None
            } else {
                InputAction::Redraw
            }
        }),
        MouseEventKind::Down(MouseButton::Left) => {
            row_at(&action_rows, event).map_or(InputAction::None, |index| {
                select_row(state, index, action_count);
                match state.interaction {
                    Interaction::Search => activate_selected(state, registry, area),
                    Interaction::Form(_) => map_state_action(state.submit()),
                }
            })
        }
        MouseEventKind::ScrollUp => move_selection(state, registry, area, -1),
        MouseEventKind::ScrollDown => move_selection(state, registry, area, 1),
        MouseEventKind::Down(MouseButton::Right)
        | MouseEventKind::Down(MouseButton::Middle)
        | MouseEventKind::Up(_)
        | MouseEventKind::Drag(_)
        | MouseEventKind::ScrollLeft
        | MouseEventKind::ScrollRight => InputAction::None,
    }
}

fn edit_backspace(state: &mut PaletteState, registry: &PaletteCatalog) -> InputAction {
    let changed = if matches!(state.interaction, Interaction::Search) {
        let changed = state.query.backspace();
        if changed {
            state.clear_error();
        }
        changed
    } else {
        state.backspace()
    };
    if !changed {
        return InputAction::None;
    }
    rerank_search(state, registry);
    InputAction::Redraw
}

fn edit_delete(state: &mut PaletteState, registry: &PaletteCatalog) -> InputAction {
    let changed = if matches!(state.interaction, Interaction::Search) {
        let changed = state.query.delete();
        if changed {
            state.clear_error();
        }
        changed
    } else {
        state.delete()
    };
    if !changed {
        return InputAction::None;
    }
    rerank_search(state, registry);
    InputAction::Redraw
}

fn rerank_search(state: &mut PaletteState, registry: &PaletteCatalog) {
    if matches!(state.interaction, Interaction::Search) {
        let ranked = registry.rank(state.query.text());
        state.set_ranked(ranked);
    }
}

#[derive(Clone, Copy)]
enum TextBufferDirection {
    Left,
    Right,
}

fn move_cursor(state: &mut PaletteState, direction: TextBufferDirection) -> InputAction {
    let changed = match direction {
        TextBufferDirection::Left if matches!(&state.interaction, Interaction::Search) => {
            state.query.move_left()
        }
        TextBufferDirection::Right if matches!(&state.interaction, Interaction::Search) => {
            state.query.move_right()
        }
        TextBufferDirection::Left => state.move_cursor_left(),
        TextBufferDirection::Right => state.move_cursor_right(),
    };
    if changed {
        InputAction::Redraw
    } else {
        InputAction::None
    }
}

fn move_up(state: &mut PaletteState, registry: &PaletteCatalog, area: Rect) -> InputAction {
    move_selection(state, registry, area, -1)
}

fn move_down(state: &mut PaletteState, registry: &PaletteCatalog, area: Rect) -> InputAction {
    move_selection(state, registry, area, 1)
}

fn move_selection(
    state: &mut PaletteState,
    registry: &PaletteCatalog,
    area: Rect,
    amount: isize,
) -> InputAction {
    let layout = compute_layout(area, state, registry);
    let visible_rows = layout.action_rows().count();
    if matches!(state.interaction, Interaction::Search) && visible_rows == 0 {
        return InputAction::None;
    }
    let before = selection_state(state);
    match state.interaction {
        Interaction::Search => {
            state.move_selection(amount, visible_rows);
            if !compute_layout(area, state, registry)
                .action_rows()
                .any(|(index, _)| index == state.selected)
            {
                state.scroll_offset = if amount.is_negative() {
                    state.scroll_offset.saturating_sub(1)
                } else {
                    state
                        .scroll_offset
                        .saturating_add(1)
                        .min(state.ranked.len().saturating_sub(1))
                };
                if !compute_layout(area, state, registry)
                    .action_rows()
                    .any(|(index, _)| index == state.selected)
                {
                    state.selected = before.0;
                    state.scroll_offset = before.1;
                }
            }
        }
        Interaction::Form(_) => state.move_form_selection(amount),
    }
    if selection_state(state) == before {
        InputAction::None
    } else {
        InputAction::Redraw
    }
}

fn activate_visible_shortcut(
    state: &mut PaletteState,
    registry: &PaletteCatalog,
    area: Rect,
    digit: char,
) -> InputAction {
    if !matches!(state.interaction, Interaction::Search) {
        return InputAction::None;
    }
    let index = match digit.to_digit(10).and_then(|number| number.checked_sub(1)) {
        Some(index) => index as usize,
        None => return InputAction::None,
    };
    let layout = compute_layout(area, state, registry);
    let Some((ranked_index, _)) = layout.action_rows().nth(index) else {
        return InputAction::None;
    };
    state.selected = ranked_index;
    activate_selected(state, registry, area)
}

fn activate_selected(
    state: &mut PaletteState,
    catalog: &PaletteCatalog,
    area: Rect,
) -> InputAction {
    if !compute_layout(area, state, catalog)
        .action_rows()
        .any(|(index, _)| index == state.selected)
    {
        return InputAction::None;
    }
    let Some(item) = state
        .ranked
        .get(state.selected)
        .and_then(|ranked| catalog.get_ranked(ranked))
        .cloned()
    else {
        return InputAction::None;
    };
    match item {
        PaletteItem::Command(command) => map_state_action(state.begin_command(command)),
        PaletteItem::Entity(entity) => InputAction::FocusEntity(entity.id),
    }
}

fn select_row(state: &mut PaletteState, index: usize, _visible_rows: usize) {
    match state.interaction {
        Interaction::Search => state.selected = index,
        Interaction::Form(_) => select_form_row(state, index),
    }
}

fn select_form_row(state: &mut PaletteState, index: usize) {
    let Some(current) = state.selected_choice().or_else(|| {
        state
            .selected_confirmation()
            .map(|selection| match selection {
                crate::state::Confirmation::Yes => 0,
                crate::state::Confirmation::No => 1,
            })
    }) else {
        return;
    };
    let (Ok(target), Ok(current)) = (isize::try_from(index), isize::try_from(current)) else {
        return;
    };
    state.move_form_selection(target.saturating_sub(current));
}

fn selection_state(
    state: &PaletteState,
) -> (
    usize,
    usize,
    Option<usize>,
    Option<crate::state::Confirmation>,
    bool,
) {
    (
        state.selected,
        state.scroll_offset,
        state.selected_choice(),
        state.selected_confirmation(),
        state.error.is_some(),
    )
}

fn row_at(rows: &[(usize, Rect)], event: MouseEvent) -> Option<usize> {
    let position = Position::new(event.column, event.row);
    rows.iter()
        .find(|(_, row)| row.width > 0 && row.height > 0 && row.contains(position))
        .map(|(index, _)| *index)
}

fn map_state_action(action: StateAction) -> InputAction {
    match action {
        StateAction::None => InputAction::None,
        StateAction::Redraw => InputAction::Redraw,
        StateAction::Close => InputAction::Close,
        StateAction::Execute { command, .. } => InputAction::ExecuteCommand(command),
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::layout::Rect;

    use super::{handle_key, handle_mouse, InputAction};
    use crate::command::{CommandId, CoreCommand};
    use crate::context::parse_invocation_context;
    use crate::model::{
        ApiCapabilities, PluginActionListResult, PluginListResult, RuntimeSnapshot,
        SessionSnapshotResult, SuccessEnvelope, WorktreeListResult,
    };
    use crate::registry::{PaletteCatalog, PaletteItem, PaletteItemId};
    use crate::state::{Interaction, LoadingState, PaletteState};
    use crate::view::compute_layout;

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

    fn registry() -> PaletteCatalog {
        PaletteCatalog::new(&runtime(), &[], &[])
    }

    fn registry_with_plugins() -> PaletteCatalog {
        let plugins: SuccessEnvelope<PluginListResult> =
            serde_json::from_str(include_str!("../tests/fixtures/plugin-list.json")).unwrap();
        let actions: SuccessEnvelope<PluginActionListResult> =
            serde_json::from_str(include_str!("../tests/fixtures/plugin-actions.json")).unwrap();
        PaletteCatalog::new(&runtime(), &plugins.result.plugins, &actions.result.actions)
    }

    fn registry_with_overflow_choices() -> PaletteCatalog {
        let mut runtime = runtime();
        for number in 3..=8 {
            let mut workspace = runtime.session.workspaces[1].clone();
            workspace.workspace_id = format!("ws-{number}");
            workspace.number = number;
            workspace.label = format!("Workspace {number}");
            runtime.session.workspaces.push(workspace);
        }
        PaletteCatalog::new(&runtime, &[], &[])
    }

    fn registry_with_eight_live_items() -> PaletteCatalog {
        let mut runtime = runtime();
        for index in 0..8 {
            let pane_id = format!("extra-live-pane-{index}");
            let mut pane = runtime.session.panes[0].clone();
            pane.pane_id = pane_id.clone();
            runtime.session.panes.push(pane);
            let mut agent = runtime.session.agents[0].clone();
            agent.pane_id = pane_id;
            runtime.session.agents.push(agent);
        }
        PaletteCatalog::new(&runtime, &[], &[])
    }

    fn state(registry: &PaletteCatalog, query: &str) -> PaletteState {
        PaletteState::ready(query, registry.rank(query))
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn enter_on_entity_emits_focus_entity_without_entering_a_form() {
        let catalog = PaletteCatalog::new(&runtime(), &[], &[]);
        let mut state = PaletteState::ready("reviewer", catalog.rank("reviewer"));
        let action = handle_key(
            &mut state,
            &catalog,
            key(KeyCode::Enter),
            Rect::new(0, 0, 64, 16),
        );

        assert_eq!(
            action,
            InputAction::FocusEntity(PaletteItemId::Agent("pane-a".into()))
        );
        assert!(matches!(state.interaction, Interaction::Search));
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn row_center(row: Rect) -> (u16, u16) {
        (row.x.saturating_add(row.width / 2), row.y)
    }

    fn action(layout: &crate::view::ViewLayout, position: usize) -> (usize, Rect) {
        layout.action_rows().nth(position).expect("action row")
    }

    #[test]
    fn printable_keys_edit_search_and_forms_but_control_and_super_do_not_insert() {
        let registry = registry();
        let area = Rect::new(0, 0, 64, 16);
        let mut search = state(&registry, "");

        assert_eq!(
            handle_key(&mut search, &registry, key(KeyCode::Char('p')), area),
            InputAction::Redraw
        );
        assert_eq!(search.query.text(), "p");
        assert_eq!(
            handle_key(
                &mut search,
                &registry,
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
                area,
            ),
            InputAction::None
        );
        assert_eq!(
            handle_key(
                &mut search,
                &registry,
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::SUPER),
                area,
            ),
            InputAction::None
        );
        assert_eq!(search.query.text(), "p");

        let mut grapheme = state(&registry, "");
        for character in ['👩', '\u{200d}', '💻'] {
            assert_eq!(
                handle_key(
                    &mut grapheme,
                    &registry,
                    key(KeyCode::Char(character)),
                    area
                ),
                InputAction::Redraw
            );
        }
        assert_eq!(grapheme.query.text(), "👩\u{200d}💻");
        assert_eq!(
            handle_key(&mut grapheme, &registry, key(KeyCode::Backspace), area),
            InputAction::Redraw
        );
        assert_eq!(grapheme.query.text(), "");

        search.set_ranked(registry.rank("prompt"));
        assert_eq!(
            handle_key(&mut search, &registry, key(KeyCode::Enter), area),
            InputAction::Redraw
        );
        assert!(matches!(search.interaction, Interaction::Form(_)));
        assert_eq!(
            handle_key(&mut search, &registry, key(KeyCode::Char('é')), area),
            InputAction::Redraw
        );
        assert_eq!(search.active_text().map(|text| text.text()), Some("é"));
        assert_eq!(
            handle_key(&mut search, &registry, key(KeyCode::Char('x')), area),
            InputAction::Redraw
        );
        assert_eq!(
            handle_key(&mut search, &registry, key(KeyCode::Left), area),
            InputAction::Redraw
        );
        assert_eq!(
            handle_key(&mut search, &registry, key(KeyCode::Backspace), area),
            InputAction::Redraw
        );
        assert_eq!(search.active_text().map(|text| text.text()), Some("x"));
        assert_eq!(
            handle_key(&mut search, &registry, key(KeyCode::Delete), area),
            InputAction::Redraw
        );
        assert_eq!(search.active_text().map(|text| text.text()), Some(""));
    }

    #[test]
    fn capped_insertions_are_inert_for_search_and_forms() {
        let registry = registry();
        let area = Rect::new(0, 0, 64, 16);
        let capped = "x".repeat(crate::state::MAX_INTERACTIVE_TEXT_GRAPHEMES);
        let mut search = state(&registry, &capped);
        let ranked = search.ranked.clone();
        assert_eq!(
            handle_key(&mut search, &registry, key(KeyCode::Char('y')), area),
            InputAction::None
        );
        assert_eq!(search.query.text(), capped);
        assert_eq!(search.ranked, ranked, "a rejected key must not rerank");
        assert_eq!(
            handle_key(&mut search, &registry, key(KeyCode::Backspace), area),
            InputAction::Redraw
        );
        assert_eq!(
            handle_key(&mut search, &registry, key(KeyCode::Char('y')), area),
            InputAction::Redraw
        );

        let mut form = state(&registry, "prompt");
        assert_eq!(
            handle_key(&mut form, &registry, key(KeyCode::Enter), area),
            InputAction::Redraw
        );
        assert!(form.insert_text(&"x".repeat(crate::state::MAX_INTERACTIVE_TEXT_GRAPHEMES)));
        let before = form.active_text().unwrap().text().to_owned();
        assert_eq!(
            handle_key(&mut form, &registry, key(KeyCode::Char('y')), area),
            InputAction::None
        );
        assert_eq!(
            form.active_text().map(|text| text.text()),
            Some(before.as_str())
        );
        assert_eq!(
            handle_key(&mut form, &registry, key(KeyCode::Backspace), area),
            InputAction::Redraw
        );
    }

    #[test]
    fn editing_navigation_submission_and_escape_follow_the_active_state() {
        let registry = registry();
        let area = Rect::new(0, 0, 64, 16);
        let mut search = state(&registry, "abc");
        assert_eq!(
            handle_key(&mut search, &registry, key(KeyCode::Left), area),
            InputAction::Redraw
        );
        assert_eq!(
            handle_key(&mut search, &registry, key(KeyCode::Backspace), area),
            InputAction::Redraw
        );
        assert_eq!(search.query.text(), "ac");
        assert_eq!(
            handle_key(&mut search, &registry, key(KeyCode::Delete), area),
            InputAction::Redraw
        );
        assert_eq!(search.query.text(), "a");

        let mut choices = state(&registry, "pane");
        let layout = compute_layout(area, &choices, &registry);
        assert!(layout.action_rows().count() > 2);
        assert_eq!(
            handle_key(&mut choices, &registry, key(KeyCode::Down), area),
            InputAction::Redraw
        );
        assert_eq!(choices.selected, action(&layout, 1).0);
        assert_eq!(
            handle_key(
                &mut choices,
                &registry,
                KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
                area
            ),
            InputAction::Redraw
        );
        assert_eq!(choices.selected, action(&layout, 2).0);
        assert_eq!(
            handle_key(
                &mut choices,
                &registry,
                KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
                area
            ),
            InputAction::Redraw
        );
        assert_eq!(choices.selected, action(&layout, 1).0);
        assert_eq!(
            handle_key(&mut choices, &registry, key(KeyCode::Up), area),
            InputAction::Redraw
        );
        assert_eq!(choices.selected, action(&layout, 0).0);

        let mut form = state(&registry, "switch workspace");
        assert_eq!(
            handle_key(&mut form, &registry, key(KeyCode::Enter), area),
            InputAction::Redraw
        );
        assert_eq!(
            handle_key(&mut form, &registry, key(KeyCode::Enter), area),
            InputAction::ExecuteCommand(CommandId::Core(CoreCommand::SwitchWorkspace))
        );
        assert_eq!(
            handle_key(&mut form, &registry, key(KeyCode::Esc), area),
            InputAction::Redraw
        );
        assert!(matches!(form.interaction, Interaction::Search));
        assert_eq!(
            handle_key(&mut form, &registry, key(KeyCode::Esc), area),
            InputAction::Close
        );

        let mut create = state(&registry, "create workspace");
        assert_eq!(
            handle_key(&mut create, &registry, key(KeyCode::Enter), area),
            InputAction::Redraw
        );
        assert_eq!(
            handle_key(&mut create, &registry, key(KeyCode::Char('n')), area),
            InputAction::Redraw
        );
        assert_eq!(
            handle_key(&mut create, &registry, key(KeyCode::Enter), area),
            InputAction::Redraw
        );
        assert_eq!(
            handle_key(&mut create, &registry, key(KeyCode::Enter), area),
            InputAction::ExecuteCommand(CommandId::Core(CoreCommand::CreateWorkspace))
        );
    }

    #[test]
    fn enter_executes_immediate_commands_and_escape_handles_bootstrap_states() {
        let registry = registry();
        let area = Rect::new(0, 0, 64, 16);
        let mut immediate = state(&registry, "zoom");
        assert_eq!(
            handle_key(&mut immediate, &registry, key(KeyCode::Enter), area),
            InputAction::ExecuteCommand(CommandId::Core(CoreCommand::TogglePaneZoom))
        );

        let mut failed = state(&registry, "");
        failed.loading = LoadingState::Failed("offline".into());
        assert_eq!(
            handle_key(&mut failed, &registry, key(KeyCode::Char('r')), area),
            InputAction::RetryBootstrap
        );
        assert_eq!(
            handle_key(&mut failed, &registry, key(KeyCode::Esc), area),
            InputAction::Close
        );
        failed.loading = LoadingState::Fatal("invalid context".into());
        assert_eq!(
            handle_key(&mut failed, &registry, key(KeyCode::Char('r')), area),
            InputAction::None
        );
        assert_eq!(
            handle_key(&mut failed, &registry, key(KeyCode::Enter), area),
            InputAction::None
        );
        assert_eq!(
            handle_key(&mut failed, &registry, key(KeyCode::Esc), area),
            InputAction::Close
        );
        failed.loading = LoadingState::Loading;
        assert_eq!(
            handle_key(&mut failed, &registry, key(KeyCode::Esc), area),
            InputAction::Close
        );
        assert_eq!(
            handle_key(&mut failed, &registry, key(KeyCode::Char('r')), area),
            InputAction::None
        );
    }

    #[test]
    fn super_number_activates_only_currently_rendered_search_rows() {
        let registry = registry();
        let area = Rect::new(0, 0, 64, 12);
        let mut search = state(&registry, "zoom");
        assert_eq!(
            handle_key(
                &mut search,
                &registry,
                KeyEvent::new(KeyCode::Char('1'), KeyModifiers::SUPER),
                area,
            ),
            InputAction::ExecuteCommand(CommandId::Core(CoreCommand::TogglePaneZoom))
        );

        let mut hidden = state(&registry, "zoom");
        assert_eq!(
            handle_key(
                &mut hidden,
                &registry,
                KeyEvent::new(KeyCode::Char('2'), KeyModifiers::SUPER),
                area,
            ),
            InputAction::None
        );
        assert_eq!(
            handle_key(
                &mut hidden,
                &registry,
                KeyEvent::new(KeyCode::Char('1'), KeyModifiers::SUPER),
                Rect::new(0, 0, 2, 12),
            ),
            InputAction::None
        );
    }

    #[test]
    fn selection_crossing_live_to_commands_stays_visible_and_tiny_views_cannot_activate() {
        let catalog = registry_with_eight_live_items();
        let area = Rect::new(0, 0, 100, 12);
        let mut search = state(&catalog, "");
        assert!(search
            .ranked
            .iter()
            .take(8)
            .all(|ranked| matches!(catalog.get_ranked(ranked), Some(PaletteItem::Entity(_)))));
        assert!(matches!(
            search
                .ranked
                .get(8)
                .and_then(|ranked| catalog.get_ranked(ranked)),
            Some(PaletteItem::Command(_))
        ));
        search.set_selection(
            7,
            compute_layout(area, &search, &catalog)
                .action_rows()
                .count(),
        );
        assert_eq!(
            handle_key(&mut search, &catalog, key(KeyCode::Down), area),
            InputAction::Redraw
        );
        assert_eq!(search.selected, 8);
        assert!(compute_layout(area, &search, &catalog)
            .action_rows()
            .any(|(index, _)| index == 8));
        assert_ne!(
            handle_key(&mut search, &catalog, key(KeyCode::Enter), area),
            InputAction::None
        );

        search.set_selection(
            8,
            compute_layout(area, &search, &catalog)
                .action_rows()
                .count(),
        );
        assert_eq!(
            handle_key(&mut search, &catalog, key(KeyCode::Up), area),
            InputAction::Redraw
        );
        assert_eq!(search.selected, 7);
        assert!(compute_layout(area, &search, &catalog)
            .action_rows()
            .any(|(index, _)| index == 7));

        let tiny = Rect::new(0, 0, 100, 6);
        let mut tiny_search = state(&catalog, "");
        assert!(compute_layout(tiny, &tiny_search, &catalog)
            .action_rows()
            .next()
            .is_none());
        assert_eq!(
            handle_key(&mut tiny_search, &catalog, key(KeyCode::Down), tiny),
            InputAction::None
        );
        assert_eq!(
            handle_key(&mut tiny_search, &catalog, key(KeyCode::Enter), tiny),
            InputAction::None
        );
    }

    #[test]
    fn default_live_entities_activate_only_from_their_rendered_ranked_rows() {
        let catalog = registry();
        let area = Rect::new(0, 0, 64, 12);
        let search = state(&catalog, "");
        let layout = compute_layout(area, &search, &catalog);
        let (visible_position, (ranked_index, row), expected) = layout
            .action_rows()
            .enumerate()
            .find_map(|(position, (index, row))| {
                match search
                    .ranked
                    .get(index)
                    .and_then(|ranked| catalog.get_ranked(ranked))
                {
                    Some(PaletteItem::Entity(entity)) => {
                        Some((position, (index, row), entity.id.clone()))
                    }
                    Some(PaletteItem::Command(_)) | None => None,
                }
            })
            .expect("the default layout exposes a live entity row");
        let shortcut = char::from_digit((visible_position + 1) as u32, 10)
            .expect("visible shortcut position is one digit");

        let mut command_number = search.clone();
        assert_eq!(
            handle_key(
                &mut command_number,
                &catalog,
                KeyEvent::new(KeyCode::Char(shortcut), KeyModifiers::SUPER),
                area,
            ),
            InputAction::FocusEntity(expected.clone())
        );
        assert_eq!(command_number.selected, ranked_index);

        let mut clicked = search;
        let (column, row_y) = row_center(row);
        assert_eq!(
            handle_mouse(
                &mut clicked,
                &catalog,
                mouse(MouseEventKind::Down(MouseButton::Left), column, row_y),
                area,
            ),
            InputAction::FocusEntity(expected)
        );
        assert_eq!(clicked.selected, ranked_index);
    }

    #[test]
    fn headers_and_hints_are_inert_for_mouse_and_shortcuts_use_only_actions() {
        let catalog = registry();
        let area = Rect::new(0, 0, 100, 30);
        let mut search = state(&catalog, "");
        let layout = compute_layout(area, &search, &catalog);
        for visual in &layout.rows {
            let area = match visual {
                crate::view::PaletteRenderRow::Header { area, .. }
                | crate::view::PaletteRenderRow::Hint { area } => *area,
                crate::view::PaletteRenderRow::Action { .. } => continue,
            };
            assert_eq!(
                handle_mouse(
                    &mut search,
                    &catalog,
                    mouse(MouseEventKind::Down(MouseButton::Left), area.x, area.y),
                    Rect::new(0, 0, 100, 30),
                ),
                InputAction::None
            );
        }
        let (first, _) = layout.action_rows().next().expect("visible action");
        let expected = match search
            .ranked
            .get(first)
            .and_then(|ranked| catalog.get_ranked(ranked))
        {
            Some(PaletteItem::Entity(entity)) => InputAction::FocusEntity(entity.id.clone()),
            Some(PaletteItem::Command(command)) => InputAction::ExecuteCommand(command.id.clone()),
            None => panic!("visible action is current"),
        };
        assert_eq!(
            handle_key(
                &mut search,
                &catalog,
                KeyEvent::new(KeyCode::Char('1'), KeyModifiers::SUPER),
                Rect::new(0, 0, 100, 30),
            ),
            expected
        );
    }

    #[test]
    fn mouse_hover_click_wheel_and_outside_use_rendered_layout_rows() {
        let registry = registry();
        let area = Rect::new(0, 0, 64, 12);
        let mut search = state(&registry, "pane");
        let layout = compute_layout(area, &search, &registry);
        let (_, second) = action(&layout, 1);
        let (column, row) = row_center(second);
        assert_eq!(
            handle_mouse(
                &mut search,
                &registry,
                mouse(MouseEventKind::Moved, column, row),
                area
            ),
            InputAction::Redraw
        );
        assert_eq!(search.selected, action(&layout, 1).0);
        let expected = match search.selected_item() {
            Some(PaletteItemId::Command(id)) => id.clone(),
            Some(_) | None => panic!("pane search should select a command"),
        };
        let click = handle_mouse(
            &mut search,
            &registry,
            mouse(MouseEventKind::Down(MouseButton::Left), column, row),
            area,
        );
        assert!(click == InputAction::Redraw || click == InputAction::ExecuteCommand(expected));

        let mut scrolled = state(&registry, ">");
        let mut no_op_wheels = 0;
        for _ in 0..20 {
            let before = (scrolled.selected, scrolled.scroll_offset);
            let action = handle_mouse(
                &mut scrolled,
                &registry,
                mouse(MouseEventKind::ScrollDown, 1, 1),
                area,
            );
            if (scrolled.selected, scrolled.scroll_offset) == before {
                assert_eq!(action, InputAction::None);
                no_op_wheels += 1;
            } else {
                assert_eq!(action, InputAction::Redraw);
            }
        }
        assert!(
            no_op_wheels > 0,
            "the twenty-wheel regression reaches the boundary"
        );
        assert_eq!(
            scrolled.scroll_offset,
            scrolled.ranked.len().saturating_sub(
                compute_layout(area, &scrolled, &registry)
                    .action_rows()
                    .count()
                    .max(1)
            )
        );
        assert_eq!(
            handle_mouse(
                &mut scrolled,
                &registry,
                mouse(MouseEventKind::ScrollUp, 1, 1),
                area
            ),
            InputAction::Redraw
        );
        assert_eq!(
            handle_mouse(
                &mut scrolled,
                &registry,
                mouse(MouseEventKind::Moved, 0, 0),
                area
            ),
            InputAction::None
        );
        assert_eq!(
            handle_mouse(
                &mut scrolled,
                &registry,
                mouse(MouseEventKind::Down(MouseButton::Left), 0, 0),
                area
            ),
            InputAction::None
        );
        let mut zero_width = state(&registry, "zoom");
        assert_eq!(
            handle_mouse(
                &mut zero_width,
                &registry,
                mouse(MouseEventKind::Down(MouseButton::Left), 1, 2),
                Rect::new(0, 0, 2, 12),
            ),
            InputAction::None
        );
    }

    #[test]
    fn duplicate_hover_and_navigation_boundaries_do_not_request_redraws() {
        let registry = registry();
        let area = Rect::new(0, 0, 64, 12);
        let mut search = state(&registry, "pane");
        let (_, row) = action(&compute_layout(area, &search, &registry), 1);
        let (column, row) = row_center(row);

        assert_eq!(
            handle_mouse(
                &mut search,
                &registry,
                mouse(MouseEventKind::Moved, column, row),
                area
            ),
            InputAction::Redraw
        );
        assert_eq!(
            handle_mouse(
                &mut search,
                &registry,
                mouse(MouseEventKind::Moved, column, row),
                area
            ),
            InputAction::None
        );
        assert_eq!(
            handle_key(&mut search, &registry, key(KeyCode::Right), area),
            InputAction::None
        );
        search.query.move_start();
        assert_eq!(
            handle_key(&mut search, &registry, key(KeyCode::Left), area),
            InputAction::None
        );
        assert_eq!(
            handle_key(&mut search, &registry, key(KeyCode::Up), area),
            InputAction::Redraw
        );
        assert_eq!(
            handle_key(&mut search, &registry, key(KeyCode::Up), area),
            InputAction::None
        );
        assert_eq!(
            handle_mouse(
                &mut search,
                &registry,
                mouse(MouseEventKind::ScrollUp, 1, 1),
                area
            ),
            InputAction::None
        );

        let mut choice = state(&registry, "switch workspace");
        assert_eq!(
            handle_key(&mut choice, &registry, key(KeyCode::Enter), area),
            InputAction::Redraw
        );
        assert_eq!(
            handle_key(&mut choice, &registry, key(KeyCode::Up), area),
            InputAction::None
        );
    }

    #[test]
    fn search_wheel_moves_the_rendered_window_with_its_selection() {
        let registry = registry();
        let area = Rect::new(0, 0, 64, 8);
        let mut search = state(&registry, "");
        let before = compute_layout(area, &search, &registry);
        let first_before = action(&before, 0).0;

        assert_eq!(
            handle_mouse(
                &mut search,
                &registry,
                mouse(MouseEventKind::ScrollDown, 1, 1),
                area,
            ),
            InputAction::Redraw
        );
        let after = compute_layout(area, &search, &registry);
        assert_eq!(action(&after, 0).0, first_before + 1);
        assert_eq!(search.selected, first_before + 1);
        assert!(after
            .action_rows()
            .any(|(index, _)| index == search.selected));

        assert_eq!(
            handle_mouse(
                &mut search,
                &registry,
                mouse(MouseEventKind::ScrollUp, 1, 1),
                area,
            ),
            InputAction::Redraw
        );
        assert_eq!(
            action(&compute_layout(area, &search, &registry), 0).0,
            first_before
        );
        assert_eq!(search.selected, first_before);
    }

    #[test]
    fn super_shortcut_uses_a_nonzero_visible_search_window() {
        let registry = registry();
        let area = Rect::new(0, 0, 64, 8);
        let mut search = state(&registry, ">");
        search.set_selection(
            4,
            compute_layout(area, &search, &registry)
                .action_rows()
                .count(),
        );
        let layout = compute_layout(area, &search, &registry);
        assert_eq!(action(&layout, 0).0, 3);
        assert_eq!(action(&layout, 1).0, 4);

        assert_eq!(
            handle_key(
                &mut search,
                &registry,
                KeyEvent::new(KeyCode::Char('2'), KeyModifiers::SUPER),
                area,
            ),
            InputAction::ExecuteCommand(CommandId::Core(CoreCommand::TogglePaneZoom))
        );
        assert_eq!(search.selected, 4);
    }

    #[test]
    fn stale_rank_from_replacement_catalog_with_same_id_cannot_render_or_execute() {
        let old_catalog = registry();
        let replacement_catalog = registry();
        let area = Rect::new(0, 0, 64, 16);
        let mut search = state(&old_catalog, "zoom");
        assert_eq!(
            search.selected_item(),
            Some(&PaletteItemId::Command(CommandId::Core(
                CoreCommand::TogglePaneZoom
            )))
        );
        assert!(replacement_catalog
            .get(&PaletteItemId::Command(CommandId::Core(
                CoreCommand::TogglePaneZoom
            )))
            .is_some());
        assert!(compute_layout(area, &search, &replacement_catalog)
            .rows
            .is_empty());
        assert_eq!(
            handle_key(&mut search, &replacement_catalog, key(KeyCode::Enter), area),
            InputAction::None
        );
        assert!(matches!(search.interaction, Interaction::Search));
    }

    #[test]
    fn plugin_commands_preserve_their_qualified_identity_on_execution() {
        let registry = registry_with_plugins();
        let area = Rect::new(0, 0, 64, 16);
        let mut search = state(&registry, "Run demo");

        assert_eq!(
            handle_key(&mut search, &registry, key(KeyCode::Enter), area),
            InputAction::ExecuteCommand(CommandId::Plugin("demo.tools.run".into()))
        );
    }

    #[test]
    fn mouse_is_inert_during_bootstrap_and_search_errors_reserve_row_space() {
        let registry = registry();
        let area = Rect::new(0, 0, 64, 12);
        for loading in [
            LoadingState::Loading,
            LoadingState::Failed("bootstrap unavailable".into()),
        ] {
            let mut search = state(&registry, "");
            search.loading = loading;
            let before = search.clone();
            for event in [
                mouse(MouseEventKind::Moved, 2, 3),
                mouse(MouseEventKind::Down(MouseButton::Left), 2, 3),
                mouse(MouseEventKind::ScrollDown, 2, 3),
            ] {
                assert_eq!(
                    handle_mouse(&mut search, &registry, event, area),
                    InputAction::None
                );
                assert_eq!(search, before);
            }
        }

        let mut search = state(&registry, "");
        search.set_error("command failed");
        let layout = compute_layout(area, &search, &registry);
        assert!(layout
            .action_rows()
            .last()
            .is_some_and(|(_, row)| row.bottom() < layout.footer.y));
    }

    #[test]
    fn overflow_choice_wheel_selects_and_clamps_the_visible_choice() {
        let registry = registry_with_overflow_choices();
        let area = Rect::new(0, 0, 36, 9);
        let mut choice = state(&registry, "switch workspace");
        assert_eq!(
            handle_key(&mut choice, &registry, key(KeyCode::Enter), area),
            InputAction::Redraw
        );
        assert_eq!(action(&compute_layout(area, &choice, &registry), 0).0, 0);

        assert_eq!(
            handle_mouse(
                &mut choice,
                &registry,
                mouse(MouseEventKind::ScrollDown, 1, 1),
                area,
            ),
            InputAction::Redraw
        );
        assert_eq!(choice.selected_choice(), Some(1));
        assert_eq!(action(&compute_layout(area, &choice, &registry), 0).0, 1);

        for _ in 0..20 {
            let before = choice.selected_choice();
            let action = handle_mouse(
                &mut choice,
                &registry,
                mouse(MouseEventKind::ScrollDown, 1, 1),
                area,
            );
            assert_eq!(
                action,
                if choice.selected_choice() == before {
                    InputAction::None
                } else {
                    InputAction::Redraw
                }
            );
        }
        assert_eq!(choice.selected_choice(), Some(6));
        assert_eq!(action(&compute_layout(area, &choice, &registry), 0).0, 6);
    }

    #[test]
    fn mouse_choice_and_confirmation_clicks_preserve_destructive_safety() {
        let registry = registry();
        let area = Rect::new(0, 0, 64, 16);
        let mut choice = state(&registry, "run command");
        assert_eq!(
            handle_key(&mut choice, &registry, key(KeyCode::Enter), area),
            InputAction::Redraw
        );
        assert_eq!(
            handle_key(&mut choice, &registry, key(KeyCode::Char('x')), area),
            InputAction::Redraw
        );
        assert_eq!(
            handle_key(&mut choice, &registry, key(KeyCode::Enter), area),
            InputAction::Redraw
        );
        let choice_row = action(&compute_layout(area, &choice, &registry), 1).1;
        let (column, row) = row_center(choice_row);
        assert_eq!(
            handle_mouse(
                &mut choice,
                &registry,
                mouse(MouseEventKind::Down(MouseButton::Left), column, row),
                area
            ),
            InputAction::ExecuteCommand(CommandId::Core(CoreCommand::RunCommandInNewPane))
        );
        assert_eq!(choice.selected_choice(), Some(1));

        let mut confirm = state(&registry, "close pane");
        assert_eq!(
            handle_key(&mut confirm, &registry, key(KeyCode::Enter), area),
            InputAction::Redraw
        );
        let rows = compute_layout(area, &confirm, &registry);
        let (no_column, no_row) = row_center(action(&rows, 1).1);
        assert_eq!(
            handle_mouse(
                &mut confirm,
                &registry,
                mouse(MouseEventKind::Down(MouseButton::Left), no_column, no_row),
                area
            ),
            InputAction::Redraw
        );
        assert!(matches!(confirm.interaction, Interaction::Search));

        assert_eq!(
            handle_key(&mut confirm, &registry, key(KeyCode::Enter), area),
            InputAction::Redraw
        );
        let yes_row = action(&compute_layout(area, &confirm, &registry), 0).1;
        let (yes_column, yes_y) = row_center(yes_row);
        assert_eq!(
            handle_mouse(
                &mut confirm,
                &registry,
                mouse(MouseEventKind::Down(MouseButton::Left), yes_column, yes_y),
                area
            ),
            InputAction::ExecuteCommand(CommandId::Core(CoreCommand::ClosePane))
        );
    }
}
