use std::collections::BTreeMap;

use unicode_segmentation::UnicodeSegmentation;

use crate::command::{
    ArgumentDefault, ArgumentKey, ArgumentKind, ArgumentStep, CommandId, CommandSpec,
};
use crate::registry::{PaletteItemId, RankedPaletteItem};

/// The largest text value accepted from interactive input. Defaults are intentionally exempt:
/// they are retained exactly so submitting an untouched form never changes its argument.
pub const MAX_INTERACTIVE_TEXT_BYTES: usize = 4 * 1024;
pub const MAX_INTERACTIVE_TEXT_GRAPHEMES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextBuffer {
    text: String,
    // Always a UTF-8 and extended-grapheme boundary. Keeping this as a byte offset makes
    // construction at the end of an externally supplied default constant-time.
    cursor: usize,
}

impl TextBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_text(text: impl Into<String>) -> Self {
        let text = text.into();
        let cursor = text.len();
        Self { text, cursor }
    }

    pub(crate) fn with_text_at_cursor(text: impl Into<String>, cursor: usize) -> Self {
        let text = text.into();
        let cursor = cursor.min(text.len());
        let cursor = if cursor == text.len() {
            cursor
        } else {
            UnicodeSegmentation::grapheme_indices(text.as_str(), true)
                .take_while(|(index, _)| *index <= cursor)
                .map(|(index, _)| index)
                .last()
                .unwrap_or(0)
        };
        Self { text, cursor }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the UTF-8 byte offset of the cursor, which is always a grapheme boundary.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn insert(&mut self, input: &str) -> bool {
        if input.is_empty() {
            return false;
        }
        self.text.insert_str(self.cursor, input);
        self.cursor = self.next_boundary_at_or_after(self.cursor + input.len());
        true
    }

    /// Inserts only when the complete resulting value fits the interactive input cap.
    pub fn insert_interactive(&mut self, input: &str) -> bool {
        if input.is_empty()
            || self
                .text
                .len()
                .checked_add(input.len())
                .is_none_or(|bytes| bytes > MAX_INTERACTIVE_TEXT_BYTES)
        {
            return false;
        }
        // The byte cap bounds this validation scan. Count the resulting string rather than
        // adding separate counts because an inserted combining mark can merge clusters.
        let cursor = self.cursor;
        self.text.insert_str(cursor, input);
        if UnicodeSegmentation::graphemes(self.text.as_str(), true).count()
            > MAX_INTERACTIVE_TEXT_GRAPHEMES
        {
            self.text.replace_range(cursor..cursor + input.len(), "");
            return false;
        }
        self.cursor = self.next_boundary_at_or_after(cursor + input.len());
        true
    }

    pub fn backspace(&mut self) -> bool {
        let Some(start) = self.previous_boundary() else {
            return false;
        };
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
        true
    }

    pub fn delete(&mut self) -> bool {
        let Some(end) = self.next_boundary() else {
            return false;
        };
        self.text.replace_range(self.cursor..end, "");
        true
    }

    pub fn move_left(&mut self) -> bool {
        let Some(start) = self.previous_boundary() else {
            return false;
        };
        self.cursor = start;
        true
    }

    pub fn move_right(&mut self) -> bool {
        let Some(end) = self.next_boundary() else {
            return false;
        };
        self.cursor = end;
        true
    }

    pub fn move_start(&mut self) {
        self.cursor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.text.len();
    }

    fn previous_boundary(&self) -> Option<usize> {
        (self.cursor > 0).then(|| {
            UnicodeSegmentation::grapheme_indices(&self.text[..self.cursor], true)
                .next_back()
                .map_or(0, |(index, _)| index)
        })
    }

    fn next_boundary(&self) -> Option<usize> {
        UnicodeSegmentation::graphemes(&self.text[self.cursor..], true)
            .next()
            .map(|grapheme| self.cursor + grapheme.len())
    }

    fn next_boundary_at_or_after(&self, byte: usize) -> usize {
        UnicodeSegmentation::grapheme_indices(self.text.as_str(), true)
            .find_map(|(index, grapheme)| {
                let end = index + grapheme.len();
                (end >= byte).then_some(end)
            })
            .unwrap_or(self.text.len())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmation {
    Yes,
    No,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgumentValue {
    Text(String),
    Choice(String),
    Confirmation(Confirmation),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompletedArguments {
    values: BTreeMap<ArgumentKey, ArgumentValue>,
}

impl CompletedArguments {
    #[cfg(test)]
    pub(crate) fn from_values(values: BTreeMap<ArgumentKey, ArgumentValue>) -> Self {
        Self { values }
    }

    pub fn get(&self, key: ArgumentKey) -> Option<&ArgumentValue> {
        self.values.get(&key)
    }

    pub fn text(&self, key: ArgumentKey) -> Option<&str> {
        match self.values.get(&key) {
            Some(ArgumentValue::Text(value)) => Some(value),
            Some(ArgumentValue::Choice(_)) | Some(ArgumentValue::Confirmation(_)) | None => None,
        }
    }

    pub fn choice(&self, key: ArgumentKey) -> Option<&str> {
        match self.values.get(&key) {
            Some(ArgumentValue::Choice(value)) => Some(value),
            Some(ArgumentValue::Text(_)) | Some(ArgumentValue::Confirmation(_)) | None => None,
        }
    }

    pub fn confirmation(&self, key: ArgumentKey) -> Option<Confirmation> {
        match self.values.get(&key) {
            Some(ArgumentValue::Confirmation(value)) => Some(*value),
            Some(ArgumentValue::Text(_)) | Some(ArgumentValue::Choice(_)) | None => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LoadingState {
    Loading,
    #[default]
    Ready,
    Failed(String),
    Fatal(String),
}

impl LoadingState {
    pub fn error(&self) -> Option<&str> {
        match self {
            Self::Failed(error) | Self::Fatal(error) => Some(error),
            Self::Loading | Self::Ready => None,
        }
    }

    pub fn is_fatal(&self) -> bool {
        matches!(self, Self::Fatal(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormState {
    command: CommandSpec,
    step: usize,
    input: FormInput,
}

impl FormState {
    pub fn command(&self) -> &CommandSpec {
        &self.command
    }

    pub fn step(&self) -> usize {
        self.step
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FormInput {
    Text(TextBuffer),
    Choice { selected: usize },
    Confirmation { selected: Confirmation },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Interaction {
    Search,
    Form(FormState),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateAction {
    None,
    Redraw,
    Close,
    Execute {
        command: CommandId,
        arguments: CompletedArguments,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteState {
    pub query: TextBuffer,
    pub ranked: Vec<RankedPaletteItem>,
    pub selected: usize,
    pub scroll_offset: usize,
    pub interaction: Interaction,
    pub values: BTreeMap<ArgumentKey, ArgumentValue>,
    pub error: Option<String>,
    pub loading: LoadingState,
    /// Ranked-result counts per palette section, maintained alongside `ranked` so headers
    /// never rescan the full ranking during layout or render.
    pub live_results: usize,
    pub command_results: usize,
}

fn count_sources(ranked: &[RankedPaletteItem]) -> (usize, usize) {
    ranked
        .iter()
        .fold((0, 0), |(live, commands), item| match item.id {
            PaletteItemId::Command(_) => (live, commands + 1),
            PaletteItemId::Workspace(_) | PaletteItemId::Tab(_) | PaletteItemId::Agent(_) => {
                (live + 1, commands)
            }
        })
}

impl PaletteState {
    pub fn new(ranked: Vec<RankedPaletteItem>) -> Self {
        let (live_results, command_results) = count_sources(&ranked);
        Self {
            query: TextBuffer::new(),
            ranked,
            selected: 0,
            scroll_offset: 0,
            interaction: Interaction::Search,
            values: BTreeMap::new(),
            error: None,
            loading: LoadingState::Ready,
            live_results,
            command_results,
        }
    }

    pub fn ready(query: impl Into<String>, ranked: Vec<RankedPaletteItem>) -> Self {
        let mut state = Self::new(ranked);
        state.query = TextBuffer::with_text(query);
        state
    }

    pub fn set_ranked(&mut self, ranked: Vec<RankedPaletteItem>) {
        let (live_results, command_results) = count_sources(&ranked);
        self.live_results = live_results;
        self.command_results = command_results;
        self.ranked = ranked;
        self.selected = 0;
        self.scroll_offset = 0;
    }

    /// Mutates the query before ranking so callers can rank this borrowed final value.
    pub fn insert_query(&mut self, input: &str) -> bool {
        if !self.query.insert_interactive(input) {
            return false;
        }
        self.error = None;
        true
    }

    pub fn selected_item(&self) -> Option<&crate::registry::PaletteItemId> {
        self.ranked.get(self.selected).map(|ranked| &ranked.id)
    }

    pub fn completed_arguments(&self) -> CompletedArguments {
        CompletedArguments {
            values: self.values.clone(),
        }
    }

    pub fn set_selection(&mut self, selection: usize, visible_rows: usize) {
        self.selected = selection.min(self.ranked.len().saturating_sub(1));
        self.ensure_selected_visible(visible_rows);
    }

    pub fn move_selection(&mut self, amount: isize, visible_rows: usize) {
        let limit = self.ranked.len().saturating_sub(1);
        self.selected = if amount.is_negative() {
            self.selected.saturating_sub(amount.unsigned_abs())
        } else {
            self.selected.saturating_add(amount as usize).min(limit)
        };
        self.ensure_selected_visible(visible_rows);
    }

    pub fn scroll_by(&mut self, amount: isize, visible_rows: usize) {
        let limit = self.ranked.len().saturating_sub(visible_rows.max(1));
        self.scroll_offset = if amount.is_negative() {
            self.scroll_offset.saturating_sub(amount.unsigned_abs())
        } else {
            self.scroll_offset.saturating_add(amount as usize)
        }
        .min(limit);
    }

    pub fn begin_command(&mut self, command: CommandSpec) -> StateAction {
        self.error = None;
        self.values.clear();
        if command.steps.is_empty() {
            return StateAction::Execute {
                command: command.id,
                arguments: CompletedArguments {
                    values: self.values.clone(),
                },
            };
        }
        let input = self.input_for_step(&command, 0);
        self.interaction = Interaction::Form(FormState {
            command,
            step: 0,
            input,
        });
        StateAction::Redraw
    }

    pub fn active_step(&self) -> Option<&ArgumentStep> {
        match &self.interaction {
            Interaction::Search => None,
            Interaction::Form(form) => form.command.steps.get(form.step),
        }
    }

    pub fn active_text(&self) -> Option<&TextBuffer> {
        match &self.interaction {
            Interaction::Form(FormState {
                input: FormInput::Text(text),
                ..
            }) => Some(text),
            Interaction::Search
            | Interaction::Form(FormState {
                input: FormInput::Choice { .. } | FormInput::Confirmation { .. },
                ..
            }) => None,
        }
    }

    pub fn selected_confirmation(&self) -> Option<Confirmation> {
        match &self.interaction {
            Interaction::Form(FormState {
                input: FormInput::Confirmation { selected },
                ..
            }) => Some(*selected),
            Interaction::Search
            | Interaction::Form(FormState {
                input: FormInput::Text(_) | FormInput::Choice { .. },
                ..
            }) => None,
        }
    }

    pub fn selected_choice(&self) -> Option<usize> {
        match &self.interaction {
            Interaction::Form(FormState {
                input: FormInput::Choice { selected },
                ..
            }) => Some(*selected),
            Interaction::Search
            | Interaction::Form(FormState {
                input: FormInput::Text(_) | FormInput::Confirmation { .. },
                ..
            }) => None,
        }
    }

    pub fn insert_text(&mut self, input: &str) -> bool {
        self.edit_active_text(|text| text.insert_interactive(input))
    }

    pub fn backspace(&mut self) -> bool {
        self.edit_active_text(TextBuffer::backspace)
    }

    pub fn delete(&mut self) -> bool {
        self.edit_active_text(TextBuffer::delete)
    }

    pub fn move_cursor_left(&mut self) -> bool {
        self.edit_active_text(TextBuffer::move_left)
    }

    pub fn move_cursor_right(&mut self) -> bool {
        self.edit_active_text(TextBuffer::move_right)
    }

    pub fn move_form_selection(&mut self, amount: isize) {
        let Some(step) = self.active_step().cloned() else {
            return;
        };
        let Some(form) = self.form_mut() else {
            return;
        };
        match &mut form.input {
            FormInput::Choice { selected } => {
                let limit = step.choices().len().saturating_sub(1);
                *selected = if amount.is_negative() {
                    selected.saturating_sub(amount.unsigned_abs())
                } else {
                    selected.saturating_add(amount as usize).min(limit)
                };
            }
            FormInput::Confirmation { selected } => {
                if amount != 0 {
                    *selected = match *selected {
                        Confirmation::Yes => Confirmation::No,
                        Confirmation::No => Confirmation::Yes,
                    };
                }
            }
            FormInput::Text(_) => {}
        }
        self.error = None;
    }

    pub fn submit(&mut self) -> StateAction {
        let Some(step) = self.active_step().cloned() else {
            return StateAction::None;
        };
        let value = match self.active_value(&step) {
            Ok(value) => value,
            Err(()) => {
                self.error = Some("a value is required".into());
                return StateAction::Redraw;
            }
        };
        if matches!(value, Some(ArgumentValue::Confirmation(Confirmation::No))) {
            self.leave_form();
            return StateAction::Redraw;
        }
        if let Some(value) = value {
            self.values.insert(step.key(), value);
        } else {
            self.values.remove(&step.key());
        }
        self.error = None;

        let (command, step_index) = match &self.interaction {
            Interaction::Form(form) if form.step + 1 < form.command.steps.len() => {
                (Some(form.command.clone()), form.step + 1)
            }
            Interaction::Form(form) => (None, form.step),
            Interaction::Search => return StateAction::None,
        };
        if let Some(command) = command {
            let input = self.input_for_step(&command, step_index);
            if let Some(form) = self.form_mut() {
                form.step = step_index;
                form.input = input;
                return StateAction::Redraw;
            }
            return StateAction::None;
        }
        let command = match &self.interaction {
            Interaction::Form(form) => form.command.id.clone(),
            Interaction::Search => return StateAction::None,
        };
        StateAction::Execute {
            command,
            arguments: CompletedArguments {
                values: self.values.clone(),
            },
        }
    }

    pub fn cancel_or_close(&mut self) -> StateAction {
        let Interaction::Form(form) = &self.interaction else {
            return StateAction::Close;
        };
        if form.step == 0 {
            self.leave_form();
            return StateAction::Redraw;
        }
        let (command, step_index) = match &self.interaction {
            Interaction::Form(form) => (form.command.clone(), form.step - 1),
            Interaction::Search => return StateAction::Close,
        };
        self.save_active_draft();
        let input = self.input_for_step(&command, step_index);
        if let Some(form) = self.form_mut() {
            form.step = step_index;
            form.input = input;
            self.error = None;
            return StateAction::Redraw;
        }
        StateAction::None
    }

    pub fn set_error(&mut self, error: impl Into<String>) {
        self.error = Some(error.into());
    }

    pub fn clear_error(&mut self) {
        self.error = None;
    }

    fn ensure_selected_visible(&mut self, visible_rows: usize) {
        let rows = visible_rows.max(1);
        let max_scroll = self.ranked.len().saturating_sub(rows);
        self.scroll_offset = self.scroll_offset.min(max_scroll);
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset.saturating_add(rows) {
            self.scroll_offset = self.selected + 1 - rows;
        }
    }

    fn form_mut(&mut self) -> Option<&mut FormState> {
        match &mut self.interaction {
            Interaction::Form(form) => Some(form),
            Interaction::Search => None,
        }
    }

    fn edit_active_text(&mut self, edit: impl FnOnce(&mut TextBuffer) -> bool) -> bool {
        let key = self.active_step().map(ArgumentStep::key);
        let draft = {
            let Some(form) = self.form_mut() else {
                return false;
            };
            let FormInput::Text(text) = &mut form.input else {
                return false;
            };
            if !edit(text) {
                return false;
            }
            text.text().to_owned()
        };
        if let Some(key) = key {
            self.values.insert(key, ArgumentValue::Text(draft));
        }
        self.error = None;
        true
    }

    fn active_value(&self, step: &ArgumentStep) -> Result<Option<ArgumentValue>, ()> {
        let Interaction::Form(form) = &self.interaction else {
            return Err(());
        };
        match (&form.input, step.kind()) {
            (FormInput::Text(text), ArgumentKind::Text) => step
                .normalize_text(text.text())
                .map(|value| value.map(ArgumentValue::Text))
                .map_err(|_| ()),
            (FormInput::Choice { selected }, ArgumentKind::Choice) => step
                .choices()
                .get(*selected)
                .map(|choice| Some(ArgumentValue::Choice(choice.value.clone())))
                .ok_or(()),
            (FormInput::Confirmation { selected }, ArgumentKind::Confirm) => {
                Ok(Some(ArgumentValue::Confirmation(*selected)))
            }
            (FormInput::Text(_), ArgumentKind::Choice | ArgumentKind::Confirm)
            | (FormInput::Choice { .. }, ArgumentKind::Text | ArgumentKind::Confirm)
            | (FormInput::Confirmation { .. }, ArgumentKind::Text | ArgumentKind::Choice) => {
                Err(())
            }
        }
    }

    fn input_for_step(&self, command: &CommandSpec, step_index: usize) -> FormInput {
        let Some(step) = command.steps.get(step_index) else {
            return FormInput::Text(TextBuffer::new());
        };
        match step.kind() {
            ArgumentKind::Text => FormInput::Text(TextBuffer::with_text(
                self.text_value_for(step).unwrap_or_default(),
            )),
            ArgumentKind::Choice => FormInput::Choice {
                selected: self.choice_index_for(step),
            },
            ArgumentKind::Confirm => FormInput::Confirmation {
                selected: self.confirmation_for(step),
            },
        }
    }

    fn text_value_for(&self, step: &ArgumentStep) -> Option<String> {
        Self::text_value(&self.values, step.key())
            .map(str::to_owned)
            .or_else(|| match step.default() {
                Some(ArgumentDefault::Value(value)) => Some(value.clone()),
                Some(ArgumentDefault::From(key)) => {
                    Self::text_value(&self.values, *key).map(str::to_owned)
                }
                None => None,
            })
    }

    fn choice_index_for(&self, step: &ArgumentStep) -> usize {
        let value = Self::choice_value(&self.values, step.key())
            .map(str::to_owned)
            .or_else(|| match step.default() {
                Some(ArgumentDefault::Value(value)) => Some(value.clone()),
                Some(ArgumentDefault::From(key)) => {
                    Self::text_value(&self.values, *key).map(str::to_owned)
                }
                None => None,
            });
        value
            .and_then(|value| {
                step.choices()
                    .iter()
                    .position(|choice| choice.value == value)
            })
            .unwrap_or(0)
    }

    fn text_value(values: &BTreeMap<ArgumentKey, ArgumentValue>, key: ArgumentKey) -> Option<&str> {
        match values.get(&key) {
            Some(ArgumentValue::Text(value)) => Some(value),
            Some(ArgumentValue::Choice(_)) | Some(ArgumentValue::Confirmation(_)) | None => None,
        }
    }

    fn choice_value(
        values: &BTreeMap<ArgumentKey, ArgumentValue>,
        key: ArgumentKey,
    ) -> Option<&str> {
        match values.get(&key) {
            Some(ArgumentValue::Choice(value)) => Some(value),
            Some(ArgumentValue::Text(_)) | Some(ArgumentValue::Confirmation(_)) | None => None,
        }
    }

    fn confirmation_for(&self, step: &ArgumentStep) -> Confirmation {
        match self.values.get(&step.key()) {
            Some(ArgumentValue::Confirmation(value)) => *value,
            Some(ArgumentValue::Text(_)) | Some(ArgumentValue::Choice(_)) | None => {
                Confirmation::No
            }
        }
    }

    fn save_active_draft(&mut self) {
        let Some(step) = self.active_step().cloned() else {
            return;
        };
        let value = match &self.interaction {
            Interaction::Form(form) => match &form.input {
                FormInput::Text(text) => Some(ArgumentValue::Text(text.text().to_owned())),
                FormInput::Choice { selected } => step
                    .choices()
                    .get(*selected)
                    .map(|choice| ArgumentValue::Choice(choice.value.clone())),
                FormInput::Confirmation { selected } => {
                    Some(ArgumentValue::Confirmation(*selected))
                }
            },
            Interaction::Search => None,
        };
        if let Some(value) = value {
            self.values.insert(step.key(), value);
        }
    }

    fn leave_form(&mut self) {
        self.interaction = Interaction::Search;
        self.values.clear();
        self.error = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{CommandCategory, CommandId, CommandSpec, CoreCommand};
    use crate::context::parse_invocation_context;
    use crate::model::{
        ApiCapabilities, RuntimeSnapshot, SessionSnapshotResult, SuccessEnvelope,
        WorktreeListResult,
    };
    use crate::registry::{PaletteItemId, RankedPaletteItem};

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

    fn ranked(ids: &[&str]) -> Vec<RankedPaletteItem> {
        ids.iter()
            .map(|id| {
                RankedPaletteItem::stale(PaletteItemId::Command(CommandId::Plugin((*id).into())))
            })
            .collect()
    }

    fn empty_command(id: &str) -> CommandSpec {
        CommandSpec {
            id: CommandId::Plugin(id.into()),
            title: id.into(),
            description: None,
            category: CommandCategory::Plugin,
            aliases: Vec::new(),
            priority: 20,
            steps: Vec::new(),
            plugin_name: None,
        }
    }

    fn prompt_command() -> CommandSpec {
        CoreCommand::PromptFocusedAgent
            .spec(&runtime(true))
            .unwrap()
    }

    fn confirmation_command() -> CommandSpec {
        CoreCommand::ClosePane.spec(&runtime(true)).unwrap()
    }

    #[test]
    fn text_buffer_edits_by_unicode_grapheme() {
        let mut text = TextBuffer::with_text("a🇺🇳e\u{301}");
        assert_eq!(text.cursor(), "a🇺🇳e\u{301}".len());
        text.move_left();
        text.backspace();
        assert_eq!(text.text(), "ae\u{301}");
        assert_eq!(text.cursor(), "a".len());
        text.delete();
        assert_eq!(text.text(), "a");
        assert_eq!(text.cursor(), "a".len());
        text.insert("🙂");
        assert_eq!(text.text(), "a🙂");
        assert_eq!(text.cursor(), "a🙂".len());
    }

    #[test]
    fn text_buffer_uses_byte_boundaries_for_unicode_navigation_and_edits() {
        let mut text = TextBuffer::with_text("a👩\u{200d}💻界e\u{301}");
        assert_eq!(text.cursor(), text.text().len());
        assert!(text.move_left());
        assert_eq!(text.cursor(), "a👩\u{200d}💻界".len());
        assert!(text.backspace());
        assert_eq!(text.text(), "a👩\u{200d}💻e\u{301}");
        assert_eq!(text.cursor(), "a👩\u{200d}💻".len());
        assert!(text.delete());
        assert_eq!(text.text(), "a👩\u{200d}💻");
        text.move_start();
        assert!(!text.move_left());
        assert!(text.move_right());
        assert_eq!(text.cursor(), 1);
        assert!(text.move_right());
        assert_eq!(text.cursor(), text.text().len());
        assert!(!text.move_right());
        assert!(text.backspace());
        assert_eq!(text.text(), "a");
        assert_eq!(text.cursor(), 1);
    }

    #[test]
    fn interactive_text_caps_cover_ascii_multibyte_combining_and_paste_like_input() {
        let mut byte_limited = TextBuffer::new();
        let almost_four_kib_grapheme = format!("e{}", "\u{301}".repeat(2_047));
        assert_eq!(
            almost_four_kib_grapheme.len(),
            MAX_INTERACTIVE_TEXT_BYTES - 1
        );
        assert!(byte_limited.insert_interactive(&almost_four_kib_grapheme));
        assert!(byte_limited.insert_interactive("x"));
        assert!(!byte_limited.insert_interactive("x"));

        let mut multibyte = TextBuffer::new();
        assert!(multibyte.insert_interactive(&"\u{200b}".repeat(MAX_INTERACTIVE_TEXT_GRAPHEMES)));
        assert!(!multibyte.insert_interactive("\u{200b}"));

        let mut combining = TextBuffer::new();
        assert!(combining.insert_interactive(&("e\u{301}").repeat(MAX_INTERACTIVE_TEXT_GRAPHEMES)));
        assert!(!combining.insert_interactive("x"));

        let mut pasted_in_chunks = TextBuffer::new();
        for _ in 0..(MAX_INTERACTIVE_TEXT_GRAPHEMES / 2) {
            assert!(pasted_in_chunks.insert_interactive("ab"));
        }
        let before = pasted_in_chunks.clone();
        assert!(!pasted_in_chunks.insert_interactive("paste"));
        assert_eq!(pasted_in_chunks, before);
    }

    #[test]
    fn raw_scope_text_remains_in_the_query_buffer() {
        let mut state = PaletteState::new(ranked(&[]));
        assert!(state.insert_query("tab: dep"));
        assert_eq!(state.query.text(), "tab: dep");
    }

    #[test]
    fn reranking_resets_selection_and_clamps_scroll() {
        let mut state = PaletteState::new(ranked(&["one", "two", "three", "four"]));
        state.set_selection(3, 2);
        assert_eq!((state.selected, state.scroll_offset), (3, 2));
        state.set_ranked(ranked(&["one"]));
        assert_eq!((state.selected, state.scroll_offset), (0, 0));
        state.set_ranked(Vec::new());
        state.move_selection(1, 3);
        state.scroll_by(9, 3);
        assert_eq!((state.selected, state.scroll_offset), (0, 0));
    }

    #[test]
    fn escape_preserves_query_when_leaving_first_form() {
        let mut state = PaletteState::ready("prompt", ranked(&["prompt"]));
        state.begin_command(prompt_command());
        assert_eq!(state.cancel_or_close(), StateAction::Redraw);
        assert_eq!(state.query.text(), "prompt");
        assert!(matches!(state.interaction, Interaction::Search));
    }

    #[test]
    fn form_back_preserves_values_across_multiple_steps() {
        let mut state = PaletteState::new(ranked(&["worktree"]));
        let form = CoreCommand::CreateWorktree.spec(&runtime(true)).unwrap();
        state.begin_command(form);
        state.insert_text("  branch  ");
        assert_eq!(state.submit(), StateAction::Redraw);
        state.insert_text("-main");
        assert_eq!(state.submit(), StateAction::Redraw);
        state.insert_text("linked");
        assert_eq!(state.cancel_or_close(), StateAction::Redraw);
        assert_eq!(state.active_text().map(TextBuffer::text), Some("HEAD-main"));
        assert_eq!(state.cancel_or_close(), StateAction::Redraw);
        assert_eq!(state.active_text().map(TextBuffer::text), Some("branch"));
        assert!(matches!(
            state.values.get(&ArgumentKey::Branch),
            Some(ArgumentValue::Text(value)) if value == "branch"
        ));
    }

    #[test]
    fn text_defaults_and_from_defaults_are_editable_and_preserved() {
        let mut runtime = runtime(true);
        runtime
            .session
            .agents
            .retain(|agent| agent.pane_id != "pane-a");
        let mut state = PaletteState::new(ranked(&["agent"]));
        state.begin_command(CoreCommand::StartAgent.spec(&runtime).unwrap());
        state.insert_text("builder");
        assert_eq!(state.submit(), StateAction::Redraw);
        assert_eq!(state.active_text().map(TextBuffer::text), Some("builder"));
        state.insert_text("-2");
        let StateAction::Execute { arguments, .. } = state.submit() else {
            panic!("the completed form must execute");
        };
        assert_eq!(arguments.text(ArgumentKey::Kind), Some("builder"));
        assert_eq!(arguments.text(ArgumentKey::Name), Some("builder-2"));
    }

    #[test]
    fn visible_text_validation_preserves_input_and_clears_after_editing() {
        let mut state = PaletteState::new(ranked(&["prompt"]));
        state.begin_command(prompt_command());
        state.insert_text(" \t\n ");
        assert_eq!(state.submit(), StateAction::Redraw);
        assert_eq!(state.error.as_deref(), Some("a value is required"));
        assert_eq!(state.active_text().map(TextBuffer::text), Some(" \t\n "));
        state.insert_text("hello");
        assert_eq!(state.error, None);
        let StateAction::Execute { arguments, .. } = state.submit() else {
            panic!("valid visible text must execute");
        };
        assert_eq!(arguments.text(ArgumentKey::Prompt), Some(" \t\n hello"));
    }

    #[test]
    fn choice_defaults_and_selection_are_clamped() {
        let mut state = PaletteState::new(ranked(&["direction"]));
        state.begin_command(
            CoreCommand::RunCommandInNewPane
                .spec(&runtime(true))
                .unwrap(),
        );
        state.insert_text("echo hi");
        assert_eq!(state.submit(), StateAction::Redraw);
        assert_eq!(state.selected_choice(), Some(0));
        state.move_form_selection(9);
        assert_eq!(state.selected_choice(), Some(1));
        state.move_form_selection(-9);
        assert_eq!(state.selected_choice(), Some(0));
    }

    #[test]
    fn destructive_confirmation_defaults_to_no_and_enter_does_not_execute() {
        let mut state = PaletteState::new(ranked(&["close"]));
        state.begin_command(confirmation_command());
        assert_eq!(state.selected_confirmation(), Some(Confirmation::No));
        assert_eq!(state.submit(), StateAction::Redraw);
        assert!(matches!(state.interaction, Interaction::Search));
        state.begin_command(confirmation_command());
        state.move_form_selection(-1);
        assert_eq!(state.selected_confirmation(), Some(Confirmation::Yes));
        assert!(matches!(state.submit(), StateAction::Execute { .. }));
    }

    #[test]
    fn optional_blank_text_advances_and_omits_the_argument() {
        let mut state = PaletteState::new(ranked(&["workspace"]));
        state.begin_command(CoreCommand::CreateWorkspace.spec(&runtime(true)).unwrap());
        state.insert_text("workspace");
        assert_eq!(state.submit(), StateAction::Redraw);
        while state
            .active_text()
            .is_some_and(|text| !text.text().is_empty())
        {
            state.backspace();
        }

        let StateAction::Execute { arguments, .. } = state.submit() else {
            panic!("a blank optional field must complete the form");
        };
        assert_eq!(arguments.text(ArgumentKey::Label), Some("workspace"));
        assert_eq!(arguments.text(ArgumentKey::Cwd), None);
    }

    #[test]
    fn inserting_combining_mark_keeps_cursor_on_a_resulting_boundary() {
        let mut text = TextBuffer::with_text("e");
        text.insert("\u{301}");
        assert_eq!(text.cursor(), "e\u{301}".len());
        text.backspace();
        assert_eq!(text.text(), "");
        assert_eq!(text.cursor(), 0);
    }

    #[test]
    fn backing_up_preserves_choice_draft() {
        let mut state = PaletteState::new(ranked(&["run"]));
        state.begin_command(
            CoreCommand::RunCommandInNewPane
                .spec(&runtime(true))
                .unwrap(),
        );
        state.insert_text("echo hi");
        assert_eq!(state.submit(), StateAction::Redraw);
        state.move_form_selection(1);
        assert_eq!(state.selected_choice(), Some(1));
        assert_eq!(state.cancel_or_close(), StateAction::Redraw);
        assert_eq!(state.submit(), StateAction::Redraw);
        assert_eq!(state.selected_choice(), Some(1));
    }

    #[test]
    fn scrolling_up_clamps_offset_after_viewport_grows() {
        let mut state = PaletteState::new(ranked(&["one", "two", "three", "four", "five", "six"]));
        state.scroll_by(4, 2);
        assert_eq!(state.scroll_offset, 4);
        state.scroll_by(-1, 5);
        assert_eq!(state.scroll_offset, 1);
    }

    #[test]
    fn state_transitions_clear_errors_without_runtime_dependencies() {
        let mut state = PaletteState::new(ranked(&["prompt"]));
        state.set_error("failed");
        assert!(state.insert_query("p"));
        state.set_ranked(ranked(&["prompt"]));
        assert_eq!(state.error, None);
        state.loading = LoadingState::Failed("bootstrap failed".into());
        assert_eq!(state.loading.error(), Some("bootstrap failed"));
        state.loading = LoadingState::Fatal("context is invalid".into());
        assert_eq!(state.loading.error(), Some("context is invalid"));
        assert!(matches!(
            state.begin_command(empty_command("plugin")),
            StateAction::Execute { .. }
        ));
    }
}
