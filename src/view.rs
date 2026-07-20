use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    command::{ArgumentKey, ArgumentKind},
    model::RuntimeSnapshot,
    registry::{
        parse_query, EntityItem, EntityKind, PaletteCatalog, PaletteItem, PaletteScope,
        PaletteSource,
    },
    state::{Confirmation, Interaction, LoadingState, PaletteState, TextBuffer},
};

const SHELL: Color = Color::Rgb(11, 16, 18);
const PANEL: Color = Color::Rgb(17, 25, 27);
const RULE: Color = Color::Rgb(41, 64, 68);
const TEXT: Color = Color::Rgb(216, 227, 228);
const MUTED: Color = Color::Rgb(113, 130, 134);
const TEAL: Color = Color::Rgb(70, 217, 194);
const SELECTED: Color = Color::Rgb(18, 97, 92);
const WARNING: Color = Color::Rgb(255, 198, 109);
const ERROR: Color = Color::Rgb(255, 123, 114);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteRenderRow {
    Header { source: PaletteSource, area: Rect },
    Hint { area: Rect },
    Action { ranked_index: usize, area: Rect },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewLayout {
    pub search: Rect,
    pub rows: Vec<PaletteRenderRow>,
    pub footer: Rect,
}

impl ViewLayout {
    pub fn action_rows(&self) -> impl Iterator<Item = (usize, Rect)> + '_ {
        self.rows.iter().filter_map(|row| match row {
            PaletteRenderRow::Action { ranked_index, area } => Some((*ranked_index, *area)),
            PaletteRenderRow::Header { .. } | PaletteRenderRow::Hint { .. } => None,
        })
    }
}

/// Deterministic layout work counters used by the benchmark harness.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LayoutStats {
    pub ranked_entries_examined: usize,
}

pub fn compute_layout(area: Rect, state: &PaletteState, catalog: &PaletteCatalog) -> ViewLayout {
    compute_layout_with_stats(area, state, catalog).0
}

#[doc(hidden)]
pub fn compute_layout_with_stats(
    area: Rect,
    state: &PaletteState,
    catalog: &PaletteCatalog,
) -> (ViewLayout, LayoutStats) {
    let inner_x = area.x.saturating_add(u16::from(area.width > 1));
    let inner_width = area.width.saturating_sub(2.min(area.width));
    let inner_y = area.y.saturating_add(u16::from(area.height > 1));
    let inner_height = area.height.saturating_sub(2.min(area.height));
    let search = Rect::new(inner_x, inner_y, inner_width, u16::from(inner_height > 0));
    let footer_y = inner_y.saturating_add(inner_height.saturating_sub(1));
    let footer = Rect::new(inner_x, footer_y, inner_width, u16::from(inner_height > 0));
    let body_y = search.y.saturating_add(search.height.saturating_add(1));
    let body_height = footer.y.saturating_sub(body_y);
    let body = Rect::new(inner_x, body_y, inner_width, body_height);

    if inner_width == 0 || !matches!(state.loading, LoadingState::Ready) {
        return (
            ViewLayout {
                search,
                rows: Vec::new(),
                footer,
            },
            LayoutStats::default(),
        );
    }

    let (rows, stats) = match &state.interaction {
        Interaction::Search => search_rows(state, catalog, body, area.width >= 72),
        Interaction::Form(_) => form_rows(body, state),
    };
    (
        ViewLayout {
            search,
            rows,
            footer,
        },
        stats,
    )
}

const MAX_STALE_RANKED_SKIPS: usize = 16;

fn search_rows(
    state: &PaletteState,
    catalog: &PaletteCatalog,
    body: Rect,
    detailed: bool,
) -> (Vec<PaletteRenderRow>, LayoutStats) {
    let error_height = u16::from(state.error.is_some());
    let bottom = body.bottom().saturating_sub(error_height);
    if body.width == 0 || body.y >= bottom {
        return (Vec::new(), LayoutStats::default());
    }
    let row_height = if detailed { 2 } else { 1 };
    let mut rows = Vec::new();
    let mut y = body.y;
    let parsed = parse_query(state.query.text());
    if parsed.scope == PaletteScope::Default && parsed.fuzzy_query.is_empty() && y < bottom {
        rows.push(PaletteRenderRow::Hint {
            area: Rect::new(body.x, y, body.width, 1),
        });
        y = y.saturating_add(1);
    }

    let mut stats = LayoutStats::default();
    let mut stale = 0;
    let first = state.scroll_offset.min(state.ranked.len());
    let mut last_source = None;
    for (ranked_index, ranked) in state.ranked.iter().enumerate().skip(first) {
        stats.ranked_entries_examined += 1;
        let Some(item) = catalog.get_ranked(ranked) else {
            stale += 1;
            if stale >= MAX_STALE_RANKED_SKIPS {
                break;
            }
            continue;
        };
        let source = item.source();
        if last_source != Some(source) {
            if y >= bottom {
                break;
            }
            rows.push(PaletteRenderRow::Header {
                source,
                area: Rect::new(body.x, y, body.width, 1),
            });
            y = y.saturating_add(1);
            last_source = Some(source);
        }
        if y.saturating_add(row_height) > bottom {
            break;
        }
        rows.push(PaletteRenderRow::Action {
            ranked_index,
            area: Rect::new(body.x, y, body.width, row_height),
        });
        y = y.saturating_add(row_height);
        // No later ranked entry can fit once an actionable row reaches the viewport edge.
        // This keeps redraw work bounded by the rows that are actually visible.
        if y >= bottom {
            break;
        }
    }
    (rows, stats)
}

fn form_rows(body: Rect, state: &PaletteState) -> (Vec<PaletteRenderRow>, LayoutStats) {
    let (row_count, selected, rows_y) = match state.active_step().map(|step| step.kind()) {
        Some(ArgumentKind::Choice) => (
            state.active_step().map_or(0, |step| step.choices().len()),
            state.selected_choice(),
            body.y.saturating_add(3),
        ),
        Some(ArgumentKind::Confirm) => (
            2,
            state
                .selected_confirmation()
                .map(|selected| match selected {
                    Confirmation::Yes => 0,
                    Confirmation::No => 1,
                }),
            body.y.saturating_add(3),
        ),
        Some(ArgumentKind::Text) | None => (0, None, body.y),
    };
    let bottom = body
        .bottom()
        .saturating_sub(u16::from(state.error.is_some()));
    let visible = usize::from(bottom.saturating_sub(rows_y));
    let usable = visible.max(1);
    let first = selected
        .filter(|selected| *selected >= usable)
        .map_or(0, |selected| selected + 1 - usable)
        .min(row_count.saturating_sub(usable));
    let rows = (first..row_count)
        .take(visible)
        .enumerate()
        .map(|(offset, index)| PaletteRenderRow::Action {
            ranked_index: index,
            area: Rect::new(
                body.x,
                rows_y.saturating_add(u16::try_from(offset).unwrap_or(u16::MAX)),
                body.width,
                1,
            ),
        })
        .collect();
    (rows, LayoutStats::default())
}

pub fn render(
    frame: &mut Frame<'_>,
    state: &PaletteState,
    registry: &PaletteCatalog,
    runtime: Option<&RuntimeSnapshot>,
) {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }
    let layout = compute_layout(area, state, registry);
    // Build the border title directly from bounded pieces: a workspace label is external
    // metadata and must never be formatted into an unbounded intermediate string.
    let title_width = usize::from(area.width.saturating_sub(2));
    let title = runtime
        .and_then(|snapshot| snapshot.focused_workspace())
        .map_or_else(
            || truncate_parts(&[" COMMAND PALETTE "], title_width),
            |workspace| {
                truncate_parts(
                    &[" COMMAND PALETTE · ", workspace.label.as_str(), " "],
                    title_width,
                )
            },
        );
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .style(Style::default().bg(SHELL).fg(RULE))
            .border_style(Style::default().fg(RULE)),
        area,
    );

    render_search(frame, layout.search, state);
    match &state.loading {
        LoadingState::Loading => {
            render_message(frame, content_area(&layout), "Loading commands…", MUTED)
        }
        LoadingState::Failed(message) => {
            render_failure(frame, content_area(&layout), message, true)
        }
        LoadingState::Fatal(message) => {
            render_failure(frame, content_area(&layout), message, false)
        }
        LoadingState::Ready => match &state.interaction {
            Interaction::Search => render_results(frame, &layout, state, registry),
            Interaction::Form(form) => render_form(
                frame,
                &layout,
                state,
                form.command().title.as_str(),
                form.command().category.as_str(),
                form.command().description.as_deref(),
            ),
        },
    }
    render_footer(frame, layout.footer, state);
}

fn content_area(layout: &ViewLayout) -> Rect {
    let y = layout
        .search
        .y
        .saturating_add(layout.search.height.saturating_add(1));
    Rect::new(
        layout.search.x,
        y,
        layout.search.width,
        layout.footer.y.saturating_sub(y),
    )
}

fn render_search(frame: &mut Frame<'_>, area: Rect, state: &PaletteState) {
    let parsed = parse_query(state.query.text());
    let badge = match parsed.scope {
        PaletteScope::Default => None,
        PaletteScope::Commands => Some("COMMANDS"),
        PaletteScope::Workspaces => Some("WORKSPACES"),
        PaletteScope::Tabs => Some("ALL TABS"),
        PaletteScope::Agents => Some("AGENTS"),
    };
    let prefix = badge.map_or_else(|| "› ".to_owned(), |badge| format!("› [{badge}] "));
    let (display_query, display_cursor) = scoped_display_query(&state.query, parsed.scope);
    let display = TextBuffer::with_text_at_cursor(display_query, display_cursor);
    let text = if matches!(state.interaction, Interaction::Search) {
        cursor_line(&prefix, &display, area.width as usize)
    } else {
        truncate_parts(&[prefix.as_str(), display.text()], area.width as usize)
    };
    render_line(frame, area, &text, Style::default().fg(TEAL).bg(PANEL));
}

fn scoped_display_query(query: &TextBuffer, scope: PaletteScope) -> (&str, usize) {
    if scope == PaletteScope::Default {
        return (query.text(), query.cursor());
    }
    let raw = query.text();
    let trimmed = raw.trim_start();
    let leading = raw.len().saturating_sub(trimmed.len());
    let after_prefix = match scope {
        PaletteScope::Commands => leading.saturating_add(1),
        PaletteScope::Workspaces => leading.saturating_add("space:".len()),
        PaletteScope::Tabs => leading.saturating_add("tab:".len()),
        PaletteScope::Agents => leading.saturating_add("agent:".len()),
        PaletteScope::Default => unreachable!("the default query is returned above"),
    };
    let fuzzy = raw.get(after_prefix..).unwrap_or_default().trim_start();
    let fuzzy_start = raw.len().saturating_sub(fuzzy.len());
    (fuzzy, query.cursor().saturating_sub(fuzzy_start))
}

fn render_results(
    frame: &mut Frame<'_>,
    layout: &ViewLayout,
    state: &PaletteState,
    catalog: &PaletteCatalog,
) {
    let has_actions = layout.action_rows().next().is_some();
    for row in &layout.rows {
        match row {
            PaletteRenderRow::Header { source, area } => render_parts(
                frame, *area, &[match source { PaletteSource::Live => "LIVE", PaletteSource::Commands => "COMMANDS" }],
                Style::default().fg(TEAL).bg(PANEL).add_modifier(Modifier::BOLD),
            ),
            PaletteRenderRow::Hint { area } => render_parts(
                frame, *area,
                &["Search everything   > commands   space: workspaces   tab: all tabs   agent: agents"],
                Style::default().fg(MUTED).bg(PANEL),
            ),
            PaletteRenderRow::Action { ranked_index, area } => {
                let Some(ranked) = state.ranked.get(*ranked_index) else { continue; };
                let Some(item) = catalog.get_ranked(ranked) else { continue; };
                let selected = *ranked_index == state.selected;
                let background = if selected { SELECTED } else { PANEL };
                frame.render_widget(Block::default().style(Style::default().bg(background)), *area);
                let title_width = area.width as usize;
                let (title, detail) = match item {
                    PaletteItem::Command(command) => {
                        let category = category_label(command.category.as_str());
                        (truncate_parts(&["[", &category, "] ", command.title.as_str()], title_width), command.description.as_deref().map(|description| truncate_line(description, title_width.saturating_sub(2))))
                    }
                    PaletteItem::Entity(entity) => {
                        let detail = if entity.breadcrumb.is_empty() {
                            truncate_parts(&[entity.status.as_str()], title_width.saturating_sub(2))
                        } else {
                            truncate_parts(&[&entity.breadcrumb, " · ", entity.status.as_str()], title_width.saturating_sub(2))
                        };
                        (entity_title(entity, title_width), Some(detail))
                    }
                };
                let foreground = if selected { Color::Rgb(239, 255, 252) } else { TEXT };
                let mut lines = vec![Line::from(Span::styled(title, Style::default().fg(foreground).bg(background).add_modifier(if selected { Modifier::BOLD } else { Modifier::empty() })) )];
                if area.height > 1 {
                    if let Some(detail) = detail { lines.push(Line::from(Span::styled(detail, Style::default().fg(if selected { Color::Rgb(239, 255, 252) } else { MUTED }).bg(background)))); }
                }
                frame.render_widget(Paragraph::new(lines), *area);
            }
        }
    }
    if !has_actions {
        let body = content_area(layout);
        let available_bottom = body
            .bottom()
            .saturating_sub(u16::from(state.error.is_some()));
        let occupied_bottom = layout
            .rows
            .iter()
            .map(|row| match row {
                PaletteRenderRow::Header { area, .. }
                | PaletteRenderRow::Hint { area }
                | PaletteRenderRow::Action { area, .. } => area.bottom(),
            })
            .max()
            .unwrap_or(body.y);
        if occupied_bottom < available_bottom {
            render_message(
                frame,
                Rect::new(body.x, occupied_bottom, body.width, 1),
                &empty_message(state),
                MUTED,
            );
        }
    }
    if let Some(error) = state
        .error
        .as_deref()
        .filter(|_| content_area(layout).height > 0)
    {
        let body = content_area(layout);
        render_line(
            frame,
            Rect::new(body.x, body.bottom().saturating_sub(1), body.width, 1),
            error,
            Style::default().fg(ERROR).bg(PANEL),
        );
    }
}

const MAX_ENTITY_SUFFIX_WIDTH: usize = 16;

fn entity_title(entity: &EntityItem, width: usize) -> String {
    let kind = entity_kind_label(entity.kind);
    let prefix = truncate_parts(&["[", kind, "] "], width);
    let Some(stable_suffix) = entity.stable_suffix.as_deref() else {
        return truncate_parts(&[prefix.as_str(), &entity.label], width);
    };
    let prefix_width = display_width(&prefix);
    let remaining = width.saturating_sub(prefix_width);
    if remaining == 0 {
        return prefix;
    }
    let suffix = truncate_line(stable_suffix, MAX_ENTITY_SUFFIX_WIDTH.min(remaining));
    let suffix_width = display_width(&suffix);
    let separator = if suffix_width < remaining { " · " } else { "" };
    let label_width = remaining
        .saturating_sub(display_width(separator))
        .saturating_sub(suffix_width);
    let label = truncate_line(&entity.label, label_width);
    truncate_parts(
        &[prefix.as_str(), label.as_str(), separator, suffix.as_str()],
        width,
    )
}

fn empty_message(state: &PaletteState) -> String {
    let parsed = parse_query(state.query.text());
    match parsed.scope {
        PaletteScope::Commands => format!(
            "No commands match “{}” · clear or change >",
            parsed.fuzzy_query
        ),
        PaletteScope::Workspaces => format!(
            "No workspaces match “{}” · clear or change space:",
            parsed.fuzzy_query
        ),
        PaletteScope::Tabs => format!(
            "No tabs match “{}” · clear or change tab:",
            parsed.fuzzy_query
        ),
        PaletteScope::Agents => format!(
            "No agents match “{}” · clear or change agent:",
            parsed.fuzzy_query
        ),
        PaletteScope::Default => "No matching results".into(),
    }
}

fn render_form(
    frame: &mut Frame<'_>,
    layout: &ViewLayout,
    state: &PaletteState,
    title: &str,
    category: &str,
    description: Option<&str>,
) {
    let body = content_area(layout);
    let category = category_label(category);
    render_parts(
        frame,
        Rect::new(body.x, body.y, body.width, u16::from(body.height > 0)),
        &["[", &category, "] ", title],
        Style::default()
            .fg(TEXT)
            .bg(PANEL)
            .add_modifier(Modifier::BOLD),
    );
    if let Some(description) = description.filter(|_| body.height > 1) {
        render_line(
            frame,
            Rect::new(body.x, body.y.saturating_add(1), body.width, 1),
            description,
            Style::default().fg(MUTED).bg(PANEL),
        );
    }
    let field_y = body.y.saturating_add(2.min(body.height));
    let field_bottom = body
        .bottom()
        .saturating_sub(u16::from(state.error.is_some()));
    let field = Rect::new(
        body.x,
        field_y,
        body.width,
        field_bottom.saturating_sub(field_y),
    );
    match state.active_step().map(|step| step.kind()) {
        Some(ArgumentKind::Text) => {
            let label = state
                .active_step()
                .map_or("Value", |step| argument_label(step.key()));
            render_line(frame, field, label, Style::default().fg(TEAL).bg(PANEL));
            if field.height > 1 {
                let text = state.active_text().map_or_else(
                    || "› │".to_owned(),
                    |text| cursor_line("› ", text, field.width as usize),
                );
                render_line(
                    frame,
                    Rect::new(field.x, field.y.saturating_add(1), field.width, 1),
                    &text,
                    Style::default().fg(TEXT).bg(PANEL),
                );
            }
        }
        Some(ArgumentKind::Choice) => {
            render_parts(
                frame,
                field,
                &[
                    "Choose ",
                    state
                        .active_step()
                        .map_or("value", |step| argument_label(step.key())),
                ],
                Style::default().fg(TEAL).bg(PANEL),
            );
            for (index, row) in layout.action_rows() {
                if row.y < field.y {
                    continue;
                }
                let label = state
                    .active_step()
                    .and_then(|step| step.choices().get(index))
                    .map_or("", |choice| choice.label.as_str());
                let selected = state.selected_choice() == Some(index);
                render_choice(frame, row, label, selected);
            }
        }
        Some(ArgumentKind::Confirm) => {
            let prompt = state
                .active_step()
                .and_then(|step| step.prompt())
                .unwrap_or("Confirm this action?");
            render_line(frame, field, prompt, Style::default().fg(WARNING).bg(PANEL));
            for (index, row) in layout.action_rows() {
                if row.y < field.y {
                    continue;
                }
                let (label, selected) = match index {
                    0 => (
                        "Yes",
                        state.selected_confirmation() == Some(Confirmation::Yes),
                    ),
                    1 => (
                        "No",
                        state.selected_confirmation() == Some(Confirmation::No),
                    ),
                    _ => continue,
                };
                render_choice(frame, row, label, selected);
            }
        }
        None => {}
    }
    if let Some(error) = state.error.as_deref().filter(|_| body.height > 0) {
        let error_y = body.bottom().saturating_sub(1);
        render_line(
            frame,
            Rect::new(body.x, error_y, body.width, 1),
            error,
            Style::default().fg(ERROR).bg(PANEL),
        );
    }
}

fn render_choice(frame: &mut Frame<'_>, area: Rect, label: &str, selected: bool) {
    let background = if selected { SELECTED } else { PANEL };
    frame.render_widget(
        Block::default().style(Style::default().bg(background)),
        area,
    );
    render_parts(
        frame,
        area,
        &["[", if selected { "x" } else { " " }, "] ", label],
        Style::default()
            .fg(if selected {
                Color::Rgb(239, 255, 252)
            } else {
                TEXT
            })
            .bg(background),
    );
}

fn render_failure(frame: &mut Frame<'_>, area: Rect, message: &str, retryable: bool) {
    render_message(frame, area, message, ERROR);
    if area.height > 1 {
        render_line(
            frame,
            Rect::new(area.x, area.y.saturating_add(1), area.width, 1),
            if retryable {
                "r Retry · Esc Close"
            } else {
                "Esc Close"
            },
            Style::default().fg(WARNING).bg(PANEL),
        );
    }
}

fn render_message(frame: &mut Frame<'_>, area: Rect, message: &str, color: Color) {
    render_line(frame, area, message, Style::default().fg(color).bg(PANEL));
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, state: &PaletteState) {
    let text = match state.loading {
        LoadingState::Failed(_) => "r Retry · Esc Close",
        LoadingState::Fatal(_) | LoadingState::Loading => "Esc close",
        LoadingState::Ready => match state.interaction {
            Interaction::Search
                if parse_query(state.query.text()).scope == PaletteScope::Default
                    && !parse_query(state.query.text()).fuzzy_query.is_empty() =>
            {
                match area.width {
                    47.. => "⌘1–9 activate · scopes: >  space:  tab:  agent:",
                    31.. => "scopes: >  space:  tab:  agent:",
                    9.. => "scopes: >",
                    _ => "Enter run · Esc close",
                }
            }
            Interaction::Search if area.width < 60 => "Enter run · Esc close",
            Interaction::Search => "↑↓ move · Enter select · Esc close",
            Interaction::Form(_) if state.active_text().is_some() => "Enter continue · Esc back",
            Interaction::Form(_) if area.width < 60 => "Enter continue · Esc back",
            Interaction::Form(_) => "↑↓ choose · Enter continue · Esc back",
        },
    };
    render_line(frame, area, text, Style::default().fg(MUTED).bg(SHELL));
}

fn render_line(frame: &mut Frame<'_>, area: Rect, text: &str, style: Style) {
    render_parts(frame, area, &[text], style);
}

fn render_parts(frame: &mut Frame<'_>, area: Rect, parts: &[&str], style: Style) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            truncate_parts(parts, area.width as usize),
            style,
        )))
        .style(style),
        Rect::new(area.x, area.y, area.width, 1),
    );
}

fn cursor_line(prefix: &str, text: &TextBuffer, width: usize) -> String {
    cursor_line_with_inspections(prefix, text, width).0
}

// Cursor offsets are UTF-8 boundaries, so this only segments a bounded window adjacent to the
// cursor rather than first walking an arbitrary prefix. A window that starts or ends inside a
// grapheme is represented by an ellipsis; normal values (within the preview policy) remain
// grapheme-exact.
fn cursor_line_with_inspections(prefix: &str, text: &TextBuffer, width: usize) -> (String, usize) {
    let prefix_width = display_width(prefix);
    let available = width.saturating_sub(prefix_width);
    if available == 0 {
        return (truncate_line(prefix, width), 0);
    }
    if available == 1 {
        return (format!("{prefix}│"), 0);
    }

    let source = text.text();
    let cursor = text.cursor();
    debug_assert!(source.is_char_boundary(cursor));
    let (left_start, right_end) = cursor_preview_window(source, cursor);
    let left_omitted = left_start > 0;
    let right_omitted = right_end < source.len();
    let mut inspected = 0;
    let left: Box<dyn Iterator<Item = &str>> =
        Box::new(UnicodeSegmentation::graphemes(&source[left_start..cursor], true).rev());
    let right: Box<dyn Iterator<Item = &str>> = Box::new(UnicodeSegmentation::graphemes(
        &source[cursor..right_end],
        true,
    ));
    let mut left = left.peekable();
    let mut right = right.peekable();
    let mut left_incomplete = cursor > left_start || left_omitted;
    let mut right_incomplete = right_end > cursor || right_omitted;
    let mut left_pending: Option<(&str, bool)> = None;
    let mut right_pending: Option<(&str, bool)> = None;
    let mut left_selected = Vec::new();
    let mut right_selected = Vec::new();
    let mut content_width = 0;
    let budget = available - 1;
    let mut prefer_left = true;

    loop {
        let mut extended = false;
        for try_left in [prefer_left, !prefer_left] {
            let other_incomplete = if try_left {
                right_incomplete
            } else {
                left_incomplete
            };
            let (pending, incomplete, iterator, omitted) = if try_left {
                (
                    &mut left_pending,
                    &mut left_incomplete,
                    &mut left,
                    left_omitted,
                )
            } else {
                (
                    &mut right_pending,
                    &mut right_incomplete,
                    &mut right,
                    right_omitted,
                )
            };
            if pending.is_none() && *incomplete {
                if inspected == MAX_RENDER_GRAPHEMES {
                    continue;
                }
                let Some(grapheme) = iterator.next() else {
                    *incomplete = omitted;
                    continue;
                };
                inspected += 1;
                // The source-window edge may have cut a cluster, so it also requires an
                // omission marker even if this iterator has no local lookahead.
                *pending = Some((grapheme, iterator.peek().is_some() || omitted));
            }
            let Some((grapheme, has_more)) = *pending else {
                continue;
            };
            let candidate_width = content_width
                + display_width(grapheme)
                + usize::from(has_more)
                + usize::from(other_incomplete);
            if candidate_width > budget {
                continue;
            }
            content_width += display_width(grapheme);
            *incomplete = has_more;
            *pending = None;
            if try_left {
                left_selected.push(grapheme);
            } else {
                right_selected.push(grapheme);
            }
            extended = true;
            break;
        }
        if !extended {
            break;
        }
        prefer_left = !prefer_left;
    }

    let mut output = String::with_capacity(prefix.len() + width.saturating_mul(4));
    output.push_str(prefix);
    if left_incomplete {
        output.push('…');
    }
    for grapheme in left_selected.iter().rev() {
        output.push_str(grapheme);
    }
    output.push('│');
    for grapheme in right_selected {
        output.push_str(grapheme);
    }
    if right_incomplete {
        output.push('…');
    }
    (output, inspected)
}

#[cfg(test)]
fn cursor_line_with_inspection_bounds(
    prefix: &str,
    text: &TextBuffer,
    width: usize,
) -> (String, usize, usize) {
    let (line, graphemes) = cursor_line_with_inspections(prefix, text, width);
    let (start, end) = cursor_preview_window(text.text(), text.cursor());
    (line, graphemes, end - start)
}

fn cursor_preview_window(source: &str, cursor: usize) -> (usize, usize) {
    const SIDE_BYTES: usize = MAX_RENDER_SOURCE_BYTES / 2;
    let left_bytes = cursor.min(SIDE_BYTES);
    let right_bytes = source.len().saturating_sub(cursor).min(SIDE_BYTES);
    // Give an unused half to the opposite side near either end, without exceeding 4 KiB total.
    let left_bytes = if right_bytes < SIDE_BYTES {
        cursor.min(MAX_RENDER_SOURCE_BYTES.saturating_sub(right_bytes))
    } else {
        left_bytes
    };
    let right_bytes = if left_bytes < SIDE_BYTES {
        source
            .len()
            .saturating_sub(cursor)
            .min(MAX_RENDER_SOURCE_BYTES.saturating_sub(left_bytes))
    } else {
        right_bytes
    };
    let mut left_start = cursor - left_bytes;
    while left_start < cursor && !source.is_char_boundary(left_start) {
        left_start += 1;
    }
    let mut right_end = cursor + right_bytes;
    while right_end > cursor && !source.is_char_boundary(right_end) {
        right_end -= 1;
    }
    (left_start, right_end)
}

fn category_label(category: &str) -> String {
    category.to_uppercase()
}

const fn entity_kind_label(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Workspace => "WORKSPACE",
        EntityKind::Tab => "TAB",
        EntityKind::Agent => "AGENT",
    }
}

fn argument_label(key: ArgumentKey) -> &'static str {
    match key {
        ArgumentKey::Workspace => "workspace",
        ArgumentKey::Label => "label",
        ArgumentKey::Cwd => "working directory",
        ArgumentKey::Worktree => "worktree",
        ArgumentKey::Branch => "branch",
        ArgumentKey::Base => "base revision",
        ArgumentKey::Tab => "tab",
        ArgumentKey::Direction => "direction",
        ArgumentKey::Command => "command",
        ArgumentKey::Agent => "agent",
        ArgumentKey::Prompt => "prompt",
        ArgumentKey::Kind => "agent kind",
        ArgumentKey::Name => "agent name",
        ArgumentKey::Confirm => "confirmation",
    }
}

// Metadata comes from plugins and runtime snapshots.  Rendering examines at most a small
// viewport-proportional number of graphemes and 4 KiB of source bytes in total.  If that budget
// ends in a grapheme, that grapheme is omitted rather than emitting a partial cluster.
const MAX_RENDER_SOURCE_BYTES: usize = 4 * 1024;
const MAX_RENDER_GRAPHEMES: usize = 256;
const RENDER_GRAPHEME_SLACK: usize = 8;

fn truncate_line(text: &str, width: usize) -> String {
    truncate_parts_with_inspections(&[text], width).0
}

#[cfg(test)]
fn truncate_line_with_inspections(text: &str, width: usize) -> (String, usize) {
    truncate_parts_with_inspections(&[text], width)
}

fn truncate_parts(parts: &[&str], width: usize) -> String {
    truncate_parts_with_inspections(parts, width).0
}

#[cfg(test)]
fn truncate_parts_with_inspections_for_test(parts: &[&str], width: usize) -> (String, usize) {
    truncate_parts_with_inspections(parts, width)
}

fn truncate_parts_with_inspections(parts: &[&str], width: usize) -> (String, usize) {
    if width == 0 {
        return (String::new(), 0);
    }

    let mut output = String::new();
    let mut boundaries = Vec::new();
    let mut output_width = 0;
    let mut inspected = 0;
    let mut bytes_remaining = MAX_RENDER_SOURCE_BYTES;
    let mut graphemes_remaining = width
        .saturating_add(RENDER_GRAPHEME_SLACK)
        .min(MAX_RENDER_GRAPHEMES);
    let mut omitted = false;

    'parts: for (part_index, part) in parts.iter().enumerate() {
        if bytes_remaining == 0 || graphemes_remaining == 0 {
            omitted =
                !part.is_empty() || parts[part_index + 1..].iter().any(|part| !part.is_empty());
            break;
        }
        let byte_limit = part.len().min(bytes_remaining);
        let mut end = byte_limit;
        while end > 0 && !part.is_char_boundary(end) {
            end -= 1;
        }
        let prefix = &part[..end];
        let source_cut = end < part.len();
        bytes_remaining -= end;
        let mut graphemes = UnicodeSegmentation::grapheme_indices(prefix, true).peekable();
        while let Some((start, grapheme)) = graphemes.next() {
            inspected += 1;
            graphemes_remaining = graphemes_remaining.saturating_sub(1);
            // The segmenter cannot know whether a prefix ending at the byte ceiling completes
            // this cluster.  Never copy that potentially partial final grapheme.
            if source_cut && start + grapheme.len() == prefix.len() {
                omitted = true;
                break 'parts;
            }
            let grapheme_width = display_width(grapheme);
            if output_width + grapheme_width > width {
                omitted = true;
                break 'parts;
            }
            boundaries.push((output.len(), grapheme_width));
            output.push_str(grapheme);
            output_width += grapheme_width;
            if graphemes_remaining == 0 {
                omitted = graphemes.peek().is_some()
                    || source_cut
                    || parts[part_index + 1..].iter().any(|part| !part.is_empty());
                break 'parts;
            }
        }
        if source_cut {
            omitted = true;
            break;
        }
    }

    if omitted {
        // Reserve one terminal cell for the omission marker, removing complete graphemes only.
        while output_width + 1 > width {
            let Some((byte_index, removed_width)) = boundaries.pop() else {
                break;
            };
            output.truncate(byte_index);
            output_width -= removed_width;
        }
        output.push('…');
    }
    (output, inspected)
}

fn display_width(text: &str) -> usize {
    Line::from(text).width()
}

#[cfg(test)]
mod tests {
    use ratatui::{backend::TestBackend, style::Color, Terminal};

    use super::{
        compute_layout, compute_layout_with_stats, cursor_line_with_inspection_bounds,
        cursor_line_with_inspections, render, truncate_line_with_inspections,
        truncate_parts_with_inspections_for_test, PaletteRenderRow,
    };
    use crate::command::{CommandId, CoreCommand};
    use crate::context::parse_invocation_context;
    use crate::model::{
        ApiCapabilities, RuntimeSnapshot, SessionSnapshotResult, SuccessEnvelope,
        WorktreeListResult,
    };
    use crate::registry::{PaletteCatalog, PaletteItem, PaletteItemId, RankedPaletteItem};
    use crate::state::{LoadingState, PaletteState};

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

    fn registry(runtime: &RuntimeSnapshot) -> PaletteCatalog {
        PaletteCatalog::new(runtime, &[], &[])
    }

    fn search_state(registry: &PaletteCatalog, query: &str) -> PaletteState {
        PaletteState::ready(query, registry.rank(query))
    }

    fn catalog_command(catalog: &PaletteCatalog, id: CommandId) -> crate::command::CommandSpec {
        match catalog.get(&PaletteItemId::Command(id)) {
            Some(PaletteItem::Command(command)) => command.clone(),
            Some(PaletteItem::Entity(_)) | None => panic!("expected command item"),
        }
    }

    fn draw(
        width: u16,
        height: u16,
        state: &PaletteState,
        registry: &PaletteCatalog,
        runtime: &RuntimeSnapshot,
    ) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, state, registry, Some(runtime)))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn text(buffer: &ratatui::buffer::Buffer) -> String {
        buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    fn row_text(buffer: &ratatui::buffer::Buffer, row: ratatui::layout::Rect) -> String {
        (row.x..row.right())
            .map(|x| buffer[(x, row.y)].symbol())
            .collect()
    }

    #[test]
    fn empty_query_renders_nonactionable_scope_hint_and_live_before_commands() {
        let snapshot = runtime();
        let catalog = registry(&snapshot);
        let state = search_state(&catalog, "");
        let output = text(&draw(100, 30, &state, &catalog, &snapshot));
        assert!(output.contains("Search everything"));
        assert!(output.contains("space:"));
        assert!(output.find("LIVE").unwrap() < output.find("COMMANDS").unwrap());
    }

    #[test]
    fn unscoped_query_renders_compact_scope_footer_and_scoped_query_renders_badge() {
        let snapshot = runtime();
        let catalog = registry(&snapshot);
        let unscoped = search_state(&catalog, "pane");
        let scoped = search_state(&catalog, "tab: deploy");
        assert!(text(&draw(100, 30, &unscoped, &catalog, &snapshot)).contains("scopes: >"));
        assert!(text(&draw(100, 30, &scoped, &catalog, &snapshot)).contains("ALL TABS"));
    }

    #[test]
    fn scoped_search_displays_only_the_fuzzy_query_with_raw_cursor_semantics() {
        let snapshot = runtime();
        let catalog = registry(&snapshot);
        let mut state = search_state(&catalog, "tab: deploy");
        let output = text(&draw(100, 12, &state, &catalog, &snapshot));
        assert!(output.contains("› [ALL TABS] deploy│"));
        assert!(!output.contains("tab: deploy"));

        state.query.move_start();
        for _ in 0..2 {
            state.query.move_right();
        }
        assert!(text(&draw(100, 12, &state, &catalog, &snapshot)).contains("› [ALL TABS] │deploy"));
        for _ in 0..2 {
            state.query.move_right();
        }
        assert!(text(&draw(100, 12, &state, &catalog, &snapshot)).contains("› [ALL TABS] │deploy"));
        assert!(state.query.backspace());
        assert_eq!(state.query.text(), "tab deploy");

        state.query = crate::state::TextBuffer::with_text("tab: 界🙂");
        let unicode = text(&draw(100, 12, &state, &catalog, &snapshot));
        assert!(unicode.contains("› [ALL TABS] 界"));
        assert!(unicode.contains('🙂'));
        assert!(unicode.contains('│'));
        assert!(!unicode.contains("tab: 界"));
        assert!(state.query.backspace());
        assert_eq!(state.query.text(), "tab: 界");
        let after_unicode_backspace = text(&draw(100, 12, &state, &catalog, &snapshot));
        assert!(after_unicode_backspace.contains("› [ALL TABS] 界"));
        assert!(after_unicode_backspace.contains('│'));
        assert!(!after_unicode_backspace.contains('🙂'));
    }

    #[test]
    fn unscoped_search_footer_uses_exact_inner_width_thresholds() {
        let snapshot = runtime();
        let catalog = registry(&snapshot);
        let state = search_state(&catalog, "pane");
        let minimal_scopes = "scopes: >";
        let full_scopes = "scopes: >  space:  tab:  agent:";
        let command_numbers = "⌘1–9 activate · scopes: >  space:  tab:  agent:";
        let cases = [
            (8, "Enter r…"),
            (9, minimal_scopes),
            (30, minimal_scopes),
            (31, full_scopes),
            (46, full_scopes),
            (47, command_numbers),
        ];

        for (inner_width, expected) in cases {
            let outer_width = inner_width + 2;
            let area = ratatui::layout::Rect::new(0, 0, outer_width as u16, 12);
            let layout = compute_layout(area, &state, &catalog);
            assert_eq!(usize::from(layout.footer.width), inner_width);
            let buffer = draw(outer_width as u16, 12, &state, &catalog, &snapshot);
            assert_eq!(
                row_text(&buffer, layout.footer),
                format!("{expected:<inner_width$}"),
                "inner width {inner_width} (outer width {outer_width})"
            );
        }
    }

    #[test]
    fn unscoped_search_footer_is_exact_for_every_inner_width_28_through_35() {
        let snapshot = runtime();
        let catalog = registry(&snapshot);
        let state = search_state(&catalog, "pane");
        for inner_width in 28..=35 {
            let outer_width = inner_width + 2;
            let expected = if inner_width < 31 {
                "scopes: >"
            } else {
                "scopes: >  space:  tab:  agent:"
            };
            let area = ratatui::layout::Rect::new(0, 0, outer_width as u16, 12);
            let layout = compute_layout(area, &state, &catalog);
            assert_eq!(usize::from(layout.footer.width), inner_width);
            let buffer = draw(outer_width as u16, 12, &state, &catalog, &snapshot);
            assert_eq!(
                row_text(&buffer, layout.footer),
                format!("{expected:<inner_width$}"),
                "inner width {inner_width} (outer width {outer_width})"
            );
        }
    }

    #[test]
    fn one_line_duplicate_entities_keep_their_stable_suffixes_at_36_columns() {
        let mut snapshot = runtime();
        let mut pane = snapshot.session.panes[0].clone();
        pane.pane_id = "duplicate-pane".into();
        snapshot.session.panes.push(pane);
        let mut agent = snapshot.session.agents[0].clone();
        agent.pane_id = "duplicate-pane".into();
        snapshot.session.agents.push(agent);
        let catalog = registry(&snapshot);
        let query = catalog
            .rank("")
            .iter()
            .find_map(|ranked| match catalog.get_ranked(ranked) {
                Some(PaletteItem::Entity(entity)) if entity.stable_suffix.is_some() => {
                    Some(entity.label.clone())
                }
                Some(PaletteItem::Command(_)) | Some(PaletteItem::Entity(_)) | None => None,
            })
            .expect("duplicate entity");
        let state = search_state(&catalog, &query);
        let layout = compute_layout(ratatui::layout::Rect::new(0, 0, 36, 12), &state, &catalog);
        let buffer = draw(36, 12, &state, &catalog, &snapshot);
        let duplicate_rows = layout
            .action_rows()
            .filter_map(|(index, area)| {
                match state
                    .ranked
                    .get(index)
                    .and_then(|ranked| catalog.get_ranked(ranked))
                {
                    Some(PaletteItem::Entity(entity)) if entity.stable_suffix.is_some() => {
                        Some(row_text(&buffer, area))
                    }
                    Some(PaletteItem::Command(_)) | Some(PaletteItem::Entity(_)) | None => None,
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(duplicate_rows.len(), 2);
        assert_ne!(duplicate_rows[0], duplicate_rows[1]);
        assert!(duplicate_rows.iter().all(|row| row.contains(" · ")));
    }

    #[test]
    fn empty_query_hint_is_not_overwritten_when_no_action_row_fits() {
        let snapshot = runtime();
        let catalog = registry(&snapshot);
        let state = search_state(&catalog, "");
        let output = text(&draw(100, 6, &state, &catalog, &snapshot));
        assert!(output.contains("Search everything"));
        assert!(!output.contains("No matching results"));
    }

    #[test]
    fn scoped_badges_empty_messages_and_sticky_headers_are_explicit() {
        let snapshot = runtime();
        let catalog = registry(&snapshot);
        for (query, badge) in [
            ("> pane", "COMMANDS"),
            ("space: docs", "WORKSPACES"),
            ("tab: deploy", "ALL TABS"),
            ("agent: reviewer", "AGENTS"),
        ] {
            let state = search_state(&catalog, query);
            assert!(text(&draw(100, 30, &state, &catalog, &snapshot)).contains(badge));
        }
        let empty = search_state(&catalog, "agent: nobody");
        assert!(text(&draw(100, 30, &empty, &catalog, &snapshot))
            .contains("No agents match “nobody” · clear or change agent:"));

        let mut scrolled = search_state(&catalog, ">");
        scrolled.scroll_offset = 1;
        let layout = compute_layout(
            ratatui::layout::Rect::new(0, 0, 100, 30),
            &scrolled,
            &catalog,
        );
        assert!(matches!(
            layout.rows.first(),
            Some(PaletteRenderRow::Header {
                source: crate::registry::PaletteSource::Commands,
                ..
            })
        ));
    }

    #[test]
    fn headers_and_hints_have_no_actionable_hit_rows_or_shortcut_numbers() {
        let snapshot = runtime();
        let catalog = registry(&snapshot);
        let state = search_state(&catalog, "");
        let layout = compute_layout(ratatui::layout::Rect::new(0, 0, 100, 30), &state, &catalog);
        assert!(layout
            .rows
            .iter()
            .any(|row| matches!(row, PaletteRenderRow::Header { .. })));
        assert!(layout
            .rows
            .iter()
            .any(|row| matches!(row, PaletteRenderRow::Hint { .. })));
        assert!(layout
            .action_rows()
            .all(|(index, _)| index < state.ranked.len()));
    }

    #[test]
    fn tiny_terminal_returns_no_rows_without_examining_ranked_entries() {
        let runtime = runtime();
        let registry = registry(&runtime);
        let state = search_state(&registry, "pane");
        let (layout, stats) =
            compute_layout_with_stats(ratatui::layout::Rect::new(0, 0, 64, 3), &state, &registry);
        assert!(layout.rows.is_empty());
        assert_eq!(stats.ranked_entries_examined, 0);
    }

    #[test]
    fn narrow_unicode_rendering_stops_before_huge_suffixes() {
        let ascii = "x".repeat(1024 * 1024);
        let (truncated, inspected) = truncate_line_with_inspections(&ascii, 8);
        assert_eq!(truncated, "xxxxxxx…");
        assert_eq!(inspected, 9);

        let unicode = "界e\u{301}".repeat(512 * 1024);
        let (truncated, inspected) = truncate_line_with_inspections(&unicode, 8);
        assert!(truncated.ends_with('…'));
        assert!(ratatui::text::Line::from(truncated.as_str()).width() <= 8);
        assert!(inspected <= 6, "inspected {inspected} graphemes");

        let zero_width = "\u{200b}".repeat(350_000);
        let (truncated, inspected) = truncate_line_with_inspections(&zero_width, 8);
        assert!(truncated.ends_with('…'));
        assert_eq!(inspected, 16);
        assert!(truncated.len() <= 16 * '\u{200b}'.len_utf8() + '…'.len_utf8());

        let combining = format!("e{}", "\u{301}".repeat(600_000));
        let (truncated, inspected) = truncate_line_with_inspections(&combining, 8);
        assert_eq!(truncated, "…");
        assert_eq!(inspected, 1);

        let (truncated, inspected) = truncate_parts_with_inspections_for_test(
            &["[", "\u{200b}".repeat(350_000).as_str(), "] "],
            8,
        );
        assert!(truncated.ends_with('…'));
        assert_eq!(inspected, 16);

        let mut cursor = crate::state::TextBuffer::with_text(ascii);
        cursor.move_start();
        cursor.move_right();
        cursor.move_right();
        cursor.move_right();
        let (rendered, inspected) = cursor_line_with_inspections("› ", &cursor, 16);
        assert!(rendered.ends_with('…'));
        assert!(inspected <= 32, "inspected {inspected} graphemes");

        // A grapheme-indexed cursor requires its prefix to locate a byte boundary,
        // but does not inspect the remaining half-megabyte suffix.
        let mut cursor = crate::state::TextBuffer::with_text(unicode);
        cursor.move_start();
        for _ in 0..4 {
            cursor.move_right();
        }
        let (rendered, inspected) = cursor_line_with_inspections("› ", &cursor, 16);
        assert!(rendered.ends_with('…'));
        assert!(inspected <= 32, "inspected {inspected} graphemes");
    }

    #[test]
    fn truncation_and_cursor_preserve_normal_unicode_exactly() {
        assert_eq!(truncate_line_with_inspections("abcdef", 0).0, "");
        assert_eq!(truncate_line_with_inspections("abcdef", 1).0, "…");
        assert_eq!(truncate_line_with_inspections("abcdef", 2).0, "a…");
        assert_eq!(truncate_line_with_inspections("🙂", 2).0, "🙂");
        assert_eq!(truncate_line_with_inspections("🙂xy", 3).0, "🙂…");
        assert_eq!(
            truncate_line_with_inspections("e\u{301}xy", 2).0,
            "e\u{301}…"
        );
        assert_eq!(
            truncate_line_with_inspections("👩\u{200d}💻xy", 3).0,
            "👩\u{200d}💻…"
        );
        assert_eq!(truncate_line_with_inspections("界ab", 3).0, "界…");

        let mut cursor = crate::state::TextBuffer::with_text("a🙂e\u{301}界");
        cursor.move_start();
        assert_eq!(
            cursor_line_with_inspections("› ", &cursor, 20).0,
            "› │a🙂e\u{301}界"
        );
        cursor.move_right();
        cursor.move_right();
        assert_eq!(
            cursor_line_with_inspections("› ", &cursor, 20).0,
            "› a🙂│e\u{301}界"
        );
        let mut zwj_cursor = crate::state::TextBuffer::with_text("👩\u{200d}💻x");
        zwj_cursor.move_left();
        assert_eq!(
            cursor_line_with_inspections("› ", &zwj_cursor, 8).0,
            "› 👩\u{200d}💻│x"
        );
    }

    #[test]
    fn rename_agent_prefill_is_visible_in_the_existing_bounded_form() {
        let snapshot = runtime();
        let catalog = registry(&snapshot);
        let mut state = search_state(&catalog, "rename focused agent");
        state.begin_command(
            CoreCommand::RenameFocusedAgent
                .spec(&snapshot)
                .expect("focused agent makes rename available"),
        );

        let rendered = text(&draw(36, 12, &state, &catalog, &snapshot));
        assert!(rendered.contains("reviewer"));
        assert!(rendered.contains("agent name"));
    }

    #[test]
    fn hostile_workspace_form_and_choice_metadata_are_bounded_on_render() {
        let mut workspace_runtime = runtime();
        workspace_runtime.session.workspaces[0].label = "w".repeat(1024 * 1024);
        let workspace_registry = registry(&workspace_runtime);
        let state = search_state(&workspace_registry, "pane");
        assert!(text(&draw(
            20,
            12,
            &state,
            &workspace_registry,
            &workspace_runtime
        ))
        .contains('…'));

        let mut form = search_state(&workspace_registry, "prompt");
        let mut command = catalog_command(
            &workspace_registry,
            CommandId::Core(CoreCommand::PromptFocusedAgent),
        );
        command.title = "t".repeat(1024 * 1024);
        form.begin_command(command);
        assert!(text(&draw(
            20,
            12,
            &form,
            &workspace_registry,
            &workspace_runtime
        ))
        .contains('…'));

        let mut prompt_runtime = runtime();
        prompt_runtime.session.panes[0].label = Some("p".repeat(1024 * 1024));
        let prompt_registry = registry(&prompt_runtime);
        let mut prompt = search_state(&prompt_registry, "close pane");
        prompt.begin_command(catalog_command(
            &prompt_registry,
            CommandId::Core(CoreCommand::ClosePane),
        ));
        assert!(text(&draw(20, 12, &prompt, &prompt_registry, &prompt_runtime)).contains('…'));

        let mut choice_runtime = runtime();
        choice_runtime.session.workspaces[1].label = "c".repeat(1024 * 1024);
        let choice_registry = registry(&choice_runtime);
        let mut choice = search_state(&choice_registry, "workspace");
        choice.begin_command(catalog_command(
            &choice_registry,
            CommandId::Core(CoreCommand::SwitchWorkspace),
        ));
        assert!(text(&draw(20, 12, &choice, &choice_registry, &choice_runtime)).contains('…'));
    }

    #[test]
    fn huge_text_defaults_render_from_a_bounded_window_and_submit_exactly() {
        for default in [
            "\u{200b}".repeat(1024 * 1024),
            format!("e{}", "\u{301}".repeat(1024 * 1024)),
        ] {
            let mut runtime = runtime();
            runtime.session.workspaces[0].label = default.clone();
            let registry = registry(&runtime);
            let mut state = search_state(&registry, "rename workspace");
            state.begin_command(catalog_command(
                &registry,
                CommandId::Core(CoreCommand::RenameWorkspace),
            ));
            let input = state.active_text().expect("rename has a text default");
            assert_eq!(input.text(), default);
            let (line, graphemes, bytes) = cursor_line_with_inspection_bounds("› ", input, 32);
            assert!(line.contains('…'));
            assert!(
                bytes <= super::MAX_RENDER_SOURCE_BYTES,
                "inspected {bytes} bytes"
            );
            assert!(
                graphemes <= super::MAX_RENDER_GRAPHEMES,
                "inspected {graphemes} graphemes"
            );
            assert!(text(&draw(32, 12, &state, &registry, &runtime)).contains('…'));
            let crate::state::StateAction::Execute { arguments, .. } = state.submit() else {
                panic!("untouched default must submit");
            };
            assert_eq!(
                arguments.text(crate::command::ArgumentKey::Label),
                Some(default.as_str())
            );
        }
    }

    #[test]
    fn default_live_entities_render_visible_bounded_rows() {
        let snapshot = runtime();
        let catalog = registry(&snapshot);
        let state = search_state(&catalog, "");
        let area = ratatui::layout::Rect::new(0, 0, 36, 12);
        let layout = compute_layout(area, &state, &catalog);
        let buffer = draw(36, 12, &state, &catalog, &snapshot);

        for (index, row) in layout.action_rows() {
            let Some(PaletteItem::Entity(entity)) = state
                .ranked
                .get(index)
                .and_then(|ranked| catalog.get_ranked(ranked))
            else {
                continue;
            };
            let rendered = (row.x..row.right())
                .map(|x| buffer[(x, row.y)].symbol())
                .collect::<String>();
            let kind = match entity.kind {
                crate::registry::EntityKind::Workspace => "WORKSPACE",
                crate::registry::EntityKind::Tab => "TAB",
                crate::registry::EntityKind::Agent => "AGENT",
            };
            assert!(rendered.contains(kind), "row {index}: {rendered:?}");
            assert!(
                rendered.contains(&*entity.label),
                "row {index}: {rendered:?}"
            );
            assert!(!rendered.trim().is_empty(), "row {index}");
        }
        let selected = layout
            .action_rows()
            .find(|(index, _)| *index == state.selected)
            .map(|(_, row)| row)
            .expect("the selected entity is rendered");
        assert_eq!(buffer[(selected.x, selected.y)].bg, Color::Rgb(18, 97, 92));
        let detailed = text(&draw(100, 30, &state, &catalog, &snapshot));
        assert!(detailed.contains("Palette / Code · WORKING"));

        let mut hostile_runtime = runtime();
        hostile_runtime.session.workspaces[0].label = "x".repeat(1024 * 1024);
        let hostile_registry = registry(&hostile_runtime);
        let hostile_state = search_state(&hostile_registry, "");
        let hostile = text(&draw(
            20,
            12,
            &hostile_state,
            &hostile_registry,
            &hostile_runtime,
        ));
        assert!(hostile.contains("[WORKSPACE]"));
        assert!(hostile.contains('…'));
    }

    #[test]
    fn normal_results_use_terminal_shell_selected_row_badges_and_descriptions_at_100x30() {
        let runtime = runtime();
        let registry = registry(&runtime);
        let mut state = search_state(&registry, "pane");
        state.set_selection(1, 8);
        let buffer = draw(100, 30, &state, &registry, &runtime);
        let output = text(&buffer);

        assert!(output.contains("COMMAND PALETTE"));
        assert!(output.contains("Focus pane"));
        assert!(output.contains("Focus the pane"));
        assert!(output.contains("[PANE]"));
        let selected = compute_layout(ratatui::layout::Rect::new(0, 0, 100, 30), &state, &registry)
            .action_rows()
            .find(|(index, _)| *index == state.selected)
            .map(|(_, row)| row)
            .unwrap();
        assert_eq!(buffer[(selected.x, selected.y)].bg, Color::Rgb(18, 97, 92));
    }

    #[test]
    fn unicode_query_is_display_width_safe_and_descriptions_hide_at_64x20() {
        let runtime = runtime();
        let registry = registry(&runtime);
        let mut state = search_state(&registry, "pane");
        state.query = crate::state::TextBuffer::with_text("界🙂é pane");
        let buffer = draw(64, 20, &state, &registry, &runtime);
        let output = text(&buffer);

        assert!(output.contains('界'));
        assert!(output.contains('🙂'));
        assert!(output.contains('é'));
        assert!(output.contains("[PANE]"));
        assert!(!output.contains("Focus the pane"));
        assert!(output.contains("scopes: >"));
    }

    #[test]
    fn compact_36x12_preserves_query_selected_title_category_and_action_hint() {
        let runtime = runtime();
        let registry = registry(&runtime);
        let state = search_state(&registry, "pane");
        let buffer = draw(36, 12, &state, &registry, &runtime);
        let output = text(&buffer);

        assert!(output.contains("pane"));
        assert!(output.contains("Focus pane"));
        assert!(output.contains("[PANE]"));
        assert!(output.contains("scopes: >"));
        assert!(!output.contains("↑↓ move"));
    }

    #[test]
    fn loading_bootstrap_failure_and_empty_results_are_explicit_actions() {
        let runtime = runtime();
        let registry = registry(&runtime);
        let mut loading = search_state(&registry, "");
        loading.loading = LoadingState::Loading;
        assert!(text(&draw(64, 20, &loading, &registry, &runtime)).contains("Loading commands"));

        loading.loading = LoadingState::Failed("Herdr is unavailable".into());
        let failed = text(&draw(64, 20, &loading, &registry, &runtime));
        assert!(failed.contains("Herdr is unavailable"));
        assert!(failed.contains("Retry"));
        assert!(failed.contains("Close"));

        loading.loading = LoadingState::Fatal("context is invalid".into());
        let fatal = text(&draw(64, 20, &loading, &registry, &runtime));
        assert!(fatal.contains("context is invalid"));
        assert!(fatal.contains("Esc Close"));
        assert!(!fatal.contains("Retry"));

        let empty = search_state(&registry, "not-a-command");
        let output = text(&draw(36, 12, &empty, &registry, &runtime));
        assert!(output.contains("No matching results"));
        assert!(output.contains("scopes: >"));
    }

    #[test]
    fn text_form_renders_cursor_validation_and_execution_error() {
        let runtime = runtime();
        let registry = registry(&runtime);
        let mut state = search_state(&registry, "prompt");
        state.begin_command(catalog_command(
            &registry,
            CommandId::Core(CoreCommand::PromptFocusedAgent),
        ));
        state.insert_text("hello");
        state.set_error("a value is required");
        let output = text(&draw(64, 20, &state, &registry, &runtime));
        assert!(output.contains("Send text to the focused agent"));
        assert!(output.contains("hello│"));
        assert!(output.contains("a value is required"));

        state.set_error("agent rejected prompt");
        let output = text(&draw(64, 20, &state, &registry, &runtime));
        assert!(output.contains("agent rejected prompt"));
    }

    #[test]
    fn execution_error_stays_visible_with_search_results() {
        let runtime = runtime();
        let registry = registry(&runtime);
        let mut state = search_state(&registry, "pane");
        state.set_error("pane run failed; the new pane remains");
        let output = text(&draw(64, 20, &state, &registry, &runtime));

        assert!(output.contains("pane run failed; the new pane remains"));
        assert!(output.contains("Focus pane"));
    }

    #[test]
    fn choice_and_negative_default_confirmation_render_safely() {
        let runtime = runtime();
        let registry = registry(&runtime);
        let mut choice = search_state(&registry, "workspace");
        choice.begin_command(catalog_command(
            &registry,
            CommandId::Core(CoreCommand::SwitchWorkspace),
        ));
        let output = text(&draw(64, 20, &choice, &registry, &runtime));
        assert!(output.contains("Choose workspace"));
        assert!(output.contains("Docs"));

        let mut confirm = search_state(&registry, "close pane");
        confirm.begin_command(catalog_command(
            &registry,
            CommandId::Core(CoreCommand::ClosePane),
        ));
        let buffer = draw(64, 20, &confirm, &registry, &runtime);
        let output = text(&buffer);
        assert!(output.contains("Close pane agent?"));
        assert!(output.contains("[ ] Yes"));
        assert!(output.contains("[x] No"));
    }

    #[test]
    fn moved_cursors_stay_visible_in_search_and_long_unicode_text_inputs() {
        let runtime = runtime();
        let registry = registry(&runtime);
        let mut search = search_state(&registry, "abcdef");
        search.query.move_left();
        search.query.move_left();
        search.query.move_left();
        assert!(text(&draw(32, 12, &search, &registry, &runtime)).contains("› abc│def"));

        let mut form = search_state(&registry, "prompt");
        form.begin_command(catalog_command(
            &registry,
            CommandId::Core(CoreCommand::PromptFocusedAgent),
        ));
        form.insert_text("界🙂abcdefghijklmnopqrstuvwx");
        for _ in 0..5 {
            form.move_cursor_left();
        }
        let buffer = draw(20, 12, &form, &registry, &runtime);
        let input_y = 6;
        let row = (1..19)
            .map(|x| buffer[(x, input_y)].symbol())
            .collect::<String>();
        let cursor = row.find('│').expect("the input cursor is rendered");
        assert!(cursor < row.trim_end().len().saturating_sub(1));
        assert!(row.contains('界') || row.contains('…'));
        assert!(ratatui::text::Line::from(row.as_str()).width() <= 18);
    }

    #[test]
    fn form_selection_windows_keep_overflow_choices_and_default_no_visible_at_36x9() {
        let runtime = runtime();
        let registry = registry(&runtime);
        let mut choice = search_state(&registry, "workspace");
        choice.begin_command(catalog_command(
            &registry,
            CommandId::Core(CoreCommand::SwitchWorkspace),
        ));
        choice.move_form_selection(1);
        let choice_output = text(&draw(36, 9, &choice, &registry, &runtime));
        assert!(choice_output.contains("[x] Docs"));

        let mut confirm = search_state(&registry, "close pane");
        confirm.begin_command(catalog_command(
            &registry,
            CommandId::Core(CoreCommand::ClosePane),
        ));
        let confirm_output = text(&draw(36, 9, &confirm, &registry, &runtime));
        assert!(confirm_output.contains("[x] No"));
    }

    #[test]
    fn layout_only_exposes_rows_that_render_and_reserves_error_space() {
        let runtime = runtime();
        let registry = registry(&runtime);
        let area = ratatui::layout::Rect::new(0, 0, 64, 12);
        let mut state = search_state(&registry, "pane");
        state.loading = LoadingState::Loading;
        assert!(compute_layout(area, &state, &registry).rows.is_empty());
        state.loading = LoadingState::Failed("unavailable".into());
        assert!(compute_layout(area, &state, &registry).rows.is_empty());

        state.loading = LoadingState::Ready;
        state.ranked = vec![RankedPaletteItem::stale(PaletteItemId::Command(
            CommandId::Plugin("missing.command".into()),
        ))];
        assert!(compute_layout(area, &state, &registry).rows.is_empty());

        let mut form = search_state(&registry, "workspace");
        form.begin_command(catalog_command(
            &registry,
            CommandId::Core(CoreCommand::SwitchWorkspace),
        ));
        form.set_error("selection is required");
        let layout = compute_layout(area, &form, &registry);
        assert!(layout
            .action_rows()
            .last()
            .is_none_or(|(_, row)| row.bottom() < layout.footer.y));
    }

    #[test]
    fn stale_rank_skips_are_bounded() {
        let runtime = runtime();
        let registry = registry(&runtime);
        let mut state = search_state(&registry, "pane");
        state.ranked = (0..32)
            .map(|index| {
                RankedPaletteItem::stale(PaletteItemId::Command(CommandId::Plugin(format!(
                    "missing.{index}"
                ))))
            })
            .collect();
        let (layout, stats) =
            compute_layout_with_stats(ratatui::layout::Rect::new(0, 0, 64, 12), &state, &registry);
        assert!(layout.rows.is_empty());
        assert_eq!(stats.ranked_entries_examined, super::MAX_STALE_RANKED_SKIPS);
    }

    #[test]
    fn layout_excludes_actionable_rows_without_usable_inner_width() {
        let runtime = runtime();
        let registry = registry(&runtime);
        let search = search_state(&registry, "pane");
        let mut form = search_state(&registry, "workspace");
        form.begin_command(catalog_command(
            &registry,
            CommandId::Core(CoreCommand::SwitchWorkspace),
        ));

        for width in 0..=2 {
            let area = ratatui::layout::Rect::new(0, 0, width, 12);
            assert!(compute_layout(area, &search, &registry).rows.is_empty());
            assert!(compute_layout(area, &form, &registry).rows.is_empty());
        }
    }

    #[test]
    fn tiny_areas_do_not_overwrite_borders_or_panic_on_inconsistent_selection() {
        let runtime = runtime();
        let registry = registry(&runtime);
        let mut form = search_state(&registry, "prompt");
        form.begin_command(catalog_command(
            &registry,
            CommandId::Core(CoreCommand::PromptFocusedAgent),
        ));
        form.set_error("a value is required");
        let buffer = draw(10, 3, &form, &registry, &runtime);
        assert_eq!(buffer[(1, 2)].symbol(), "─");

        for (width, height) in [(35, 11), (1, 1), (0, 0)] {
            let _ = draw(width, height, &form, &registry, &runtime);
        }

        let mut search = search_state(&registry, "pane");
        search.selected = usize::MAX;
        assert!(std::panic::catch_unwind(|| {
            compute_layout(ratatui::layout::Rect::new(0, 0, 64, 12), &search, &registry)
        })
        .is_ok());
    }

    #[test]
    fn selected_rows_use_high_contrast_descriptions_and_text_forms_have_text_hints() {
        let runtime = runtime();
        let registry = registry(&runtime);
        let mut search = search_state(&registry, "pane");
        search.set_selection(1, 8);
        let layout = compute_layout(
            ratatui::layout::Rect::new(0, 0, 100, 30),
            &search,
            &registry,
        );
        let selected = layout
            .action_rows()
            .find(|(index, _)| *index == search.selected)
            .map(|(_, row)| row)
            .unwrap();
        let buffer = draw(100, 30, &search, &registry, &runtime);
        for x in selected.x..selected.right() {
            assert_eq!(buffer[(x, selected.y)].bg, Color::Rgb(18, 97, 92));
            assert_eq!(buffer[(x, selected.y + 1)].bg, Color::Rgb(18, 97, 92));
        }
        assert_eq!(
            buffer[(selected.x, selected.y + 1)].fg,
            Color::Rgb(239, 255, 252)
        );

        let mut form = search_state(&registry, "prompt");
        form.begin_command(catalog_command(
            &registry,
            CommandId::Core(CoreCommand::PromptFocusedAgent),
        ));
        let output = text(&draw(64, 20, &form, &registry, &runtime));
        assert!(output.contains("Enter continue · Esc back"));
        assert!(!output.contains("↑↓ choose"));
    }
}
