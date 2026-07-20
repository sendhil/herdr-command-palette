use std::cmp::{Ordering, Reverse};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::Arc;

use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Matcher, Utf32Str};
use unicode_segmentation::UnicodeSegmentation;

use crate::command::{CommandCategory, CommandId, CommandSpec, CoreCommand};
use crate::model::{
    AgentInfo, AgentStatus, InstalledPluginInfo, PaneInfo, PluginActionContext, PluginActionInfo,
    PluginPlatform, RuntimeSnapshot, TabInfo, WorkspaceInfo,
};

const SELF_OPEN_ACTION: &str = "herdr.command-palette.open";
const MAX_METADATA_SOURCE_BYTES: usize = 4 * 1024;
const MAX_METADATA_GRAPHEMES: usize = 256;

// Each catalog generation invalidates ranks from its predecessors.
static NEXT_CATALOG_GENERATION: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteItemId {
    Command(CommandId),
    Workspace(String),
    Tab(String),
    Agent(String),
}

impl PaletteItemId {
    fn stable_key(&self) -> String {
        match self {
            Self::Command(id) => format!("command:{}", id.stable_id()),
            Self::Workspace(id) => format!("workspace:{id}"),
            Self::Tab(id) => format!("tab:{id}"),
            Self::Agent(id) => format!("agent:{id}"),
        }
    }

    fn sort_key(&self) -> (u8, u8, &str) {
        match self {
            Self::Command(CommandId::Core(id)) => (0, 0, id.stable_id()),
            Self::Command(CommandId::Plugin(id)) => (0, 1, id),
            Self::Workspace(id) => (1, 0, id),
            Self::Tab(id) => (2, 0, id),
            Self::Agent(id) => (3, 0, id),
        }
    }
}

impl PartialOrd for PaletteItemId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PaletteItemId {
    fn cmp(&self, other: &Self) -> Ordering {
        self.sort_key().cmp(&other.sort_key())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteSource {
    Live,
    Commands,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteScope {
    Default,
    Commands,
    Workspaces,
    Tabs,
    Agents,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPaletteQuery {
    pub scope: PaletteScope,
    pub fuzzy_query: String,
}

pub fn parse_query(raw: &str) -> ParsedPaletteQuery {
    let trimmed = raw.trim_start();
    if let Some(query) = trimmed.strip_prefix('>') {
        return parsed_query(PaletteScope::Commands, query);
    }
    for (prefix, scope) in [
        ("space:", PaletteScope::Workspaces),
        ("tab:", PaletteScope::Tabs),
        ("agent:", PaletteScope::Agents),
    ] {
        if trimmed
            .get(..prefix.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix))
        {
            return parsed_query(scope, &trimmed[prefix.len()..]);
        }
    }
    ParsedPaletteQuery {
        scope: PaletteScope::Default,
        fuzzy_query: trimmed.to_owned(),
    }
}

fn parsed_query(scope: PaletteScope, query: &str) -> ParsedPaletteQuery {
    ParsedPaletteQuery {
        scope,
        fuzzy_query: query.trim_start().to_owned(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntityKind {
    Workspace,
    Tab,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityItem {
    pub id: PaletteItemId,
    pub kind: EntityKind,
    pub label: Arc<str>,
    pub breadcrumb: Arc<str>,
    pub status: AgentStatus,
    pub stable_suffix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteItem {
    Command(CommandSpec),
    Entity(EntityItem),
}

impl PaletteItem {
    pub fn id(&self) -> PaletteItemId {
        match self {
            Self::Command(command) => PaletteItemId::Command(command.id.clone()),
            Self::Entity(entity) => entity.id.clone(),
        }
    }

    pub const fn source(&self) -> PaletteSource {
        match self {
            Self::Command(_) => PaletteSource::Commands,
            Self::Entity(_) => PaletteSource::Live,
        }
    }

    pub fn primary_label(&self) -> &str {
        match self {
            Self::Command(command) => &command.title,
            Self::Entity(entity) => &entity.label,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RankKey {
    match_class: u8,
    fuzzy_score: Reverse<u32>,
    source_tie: u8,
    catalog_priority: u16,
    normalized_primary: Arc<str>,
    stable_id: PaletteItemId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankedPaletteItem {
    pub id: PaletteItemId,
    catalog_generation: u64,
    item_index: Option<usize>,
    rank_key: RankKey,
}

impl RankedPaletteItem {
    #[cfg(test)]
    pub(crate) fn stale(id: PaletteItemId) -> Self {
        Self {
            id: id.clone(),
            catalog_generation: 0,
            item_index: None,
            rank_key: RankKey {
                match_class: 0,
                fuzzy_score: Reverse(0),
                source_tie: 0,
                catalog_priority: 0,
                normalized_primary: Arc::from(""),
                stable_id: id,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CatalogRecord {
    item: PaletteItem,
    normalized_primary: Arc<str>,
    search_fields: SearchFields,
    snapshot_order: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SearchFields {
    Command {
        primary: String,
        secondary: String,
    },
    Entity {
        primary: Arc<str>,
        breadcrumb: Arc<str>,
        normalized_breadcrumb: Arc<str>,
        lowercase_primary: Arc<str>,
        lowercase_breadcrumb: Arc<str>,
        status: &'static str,
    },
}

#[derive(Default)]
struct MetadataInterner {
    values: HashMap<String, Arc<str>>,
    normalized: HashMap<Arc<str>, Arc<str>>,
    lowercase: HashMap<Arc<str>, Arc<str>>,
}

impl MetadataInterner {
    fn preview(&mut self, value: &str) -> Arc<str> {
        self.intern(bounded_preview(value))
    }

    fn breadcrumb(&mut self, workspace: &Arc<str>, tab: &Arc<str>) -> Arc<str> {
        self.intern(bounded_preview_parts(&[workspace, " / ", tab]).0)
    }

    fn intern(&mut self, value: String) -> Arc<str> {
        if let Some(interned) = self.values.get(value.as_str()) {
            return interned.clone();
        }
        let interned: Arc<str> = Arc::from(value.clone());
        self.values.insert(value, interned.clone());
        interned
    }

    fn normalized(&mut self, value: &Arc<str>) -> Arc<str> {
        if let Some(normalized) = self.normalized.get(value) {
            return normalized.clone();
        }
        let normalized: Arc<str> = Arc::from(normalize_for_match(value));
        self.normalized.insert(value.clone(), normalized.clone());
        normalized
    }

    fn lowercase(&mut self, value: &Arc<str>) -> Arc<str> {
        if let Some(lowercase) = self.lowercase.get(value) {
            return lowercase.clone();
        }
        let lowercase = self.intern(value.to_lowercase());
        self.lowercase.insert(value.clone(), lowercase.clone());
        lowercase
    }
}

pub struct PaletteCatalog {
    generation: u64,
    items: Vec<CatalogRecord>,
    item_indices: HashMap<String, usize>,
    colliding_item_indices: HashMap<String, Vec<usize>>,
    tab_workspace_ids: HashMap<String, String>,
    focused_workspace_id: Option<String>,
    focused_tab_id: Option<String>,
    focused_pane_id: Option<String>,
}

impl PaletteCatalog {
    pub fn empty() -> Self {
        Self {
            generation: NEXT_CATALOG_GENERATION.fetch_add(1, AtomicOrdering::Relaxed),
            items: Vec::new(),
            item_indices: HashMap::new(),
            colliding_item_indices: HashMap::new(),
            tab_workspace_ids: HashMap::new(),
            focused_workspace_id: None,
            focused_tab_id: None,
            focused_pane_id: None,
        }
    }

    pub fn new(
        runtime: &RuntimeSnapshot,
        plugins: &[InstalledPluginInfo],
        actions: &[PluginActionInfo],
    ) -> Self {
        let session = &runtime.session;
        let workspaces = session
            .workspaces
            .iter()
            .filter(|workspace| !workspace.workspace_id.is_empty())
            .map(|workspace| (workspace.workspace_id.as_str(), workspace))
            .collect::<HashMap<_, _>>();
        let tabs = session
            .tabs
            .iter()
            .filter(|tab| {
                !tab.tab_id.is_empty() && workspaces.contains_key(tab.workspace_id.as_str())
            })
            .map(|tab| (tab.tab_id.as_str(), tab))
            .collect::<HashMap<_, _>>();
        let panes = session
            .panes
            .iter()
            .filter(|pane| !pane.pane_id.is_empty())
            .map(|pane| (pane.pane_id.as_str(), pane))
            .collect::<HashMap<_, _>>();
        let mut metadata = MetadataInterner::default();
        let empty_metadata = metadata.intern(String::new());
        let workspace_labels = session
            .workspaces
            .iter()
            .filter(|workspace| !workspace.workspace_id.is_empty())
            .map(|workspace| {
                (
                    workspace.workspace_id.as_str(),
                    metadata.preview(&workspace.label),
                )
            })
            .collect::<HashMap<_, _>>();
        let tab_labels = session
            .tabs
            .iter()
            .filter(|tab| {
                !tab.tab_id.is_empty() && workspaces.contains_key(tab.workspace_id.as_str())
            })
            .map(|tab| (tab.tab_id.as_str(), metadata.preview(&tab.label)))
            .collect::<HashMap<_, _>>();
        let agent_breadcrumbs = session
            .tabs
            .iter()
            .filter_map(|tab| {
                let workspace = workspace_labels.get(tab.workspace_id.as_str())?;
                let tab_label = tab_labels.get(tab.tab_id.as_str())?;
                Some((
                    tab.tab_id.as_str(),
                    metadata.breadcrumb(workspace, tab_label),
                ))
            })
            .collect::<HashMap<_, _>>();
        let mut items = Vec::new();
        let mut item_indices = HashMap::new();
        let mut colliding_item_indices = HashMap::new();
        let mut tab_workspace_ids = HashMap::new();
        let mut snapshot_order = 0;

        for workspace in &session.workspaces {
            if workspace.workspace_id.is_empty() {
                continue;
            }
            insert_record(
                &mut items,
                &mut item_indices,
                &mut colliding_item_indices,
                catalog_record(
                    PaletteItem::Entity(EntityItem {
                        id: PaletteItemId::Workspace(workspace.workspace_id.clone()),
                        kind: EntityKind::Workspace,
                        label: workspace_labels[workspace.workspace_id.as_str()].clone(),
                        breadcrumb: empty_metadata.clone(),
                        status: workspace.agent_status,
                        stable_suffix: None,
                    }),
                    snapshot_order,
                    &mut metadata,
                ),
            );
            snapshot_order += 1;
        }
        for tab in &session.tabs {
            let Some(workspace) = workspaces.get(tab.workspace_id.as_str()) else {
                continue;
            };
            if tab.tab_id.is_empty() {
                continue;
            }
            tab_workspace_ids
                .entry(tab.tab_id.clone())
                .or_insert_with(|| tab.workspace_id.clone());
            insert_record(
                &mut items,
                &mut item_indices,
                &mut colliding_item_indices,
                catalog_record(
                    PaletteItem::Entity(EntityItem {
                        id: PaletteItemId::Tab(tab.tab_id.clone()),
                        kind: EntityKind::Tab,
                        label: tab_labels[tab.tab_id.as_str()].clone(),
                        breadcrumb: workspace_labels[workspace.workspace_id.as_str()].clone(),
                        status: tab.agent_status,
                        stable_suffix: None,
                    }),
                    snapshot_order,
                    &mut metadata,
                ),
            );
            snapshot_order += 1;
        }
        for agent in &session.agents {
            if agent.pane_id.is_empty() || !valid_agent(agent, &workspaces, &tabs, &panes) {
                continue;
            }
            let tab = tabs[agent.tab_id.as_str()];
            insert_record(
                &mut items,
                &mut item_indices,
                &mut colliding_item_indices,
                catalog_record(
                    PaletteItem::Entity(entity_for_agent(
                        agent,
                        agent_breadcrumbs[tab.tab_id.as_str()].clone(),
                        &mut metadata,
                    )),
                    snapshot_order,
                    &mut metadata,
                ),
            );
            snapshot_order += 1;
        }
        set_duplicate_stable_suffixes(&mut items);
        for command in available_commands(runtime, plugins, actions) {
            insert_record(
                &mut items,
                &mut item_indices,
                &mut colliding_item_indices,
                catalog_record(PaletteItem::Command(command), snapshot_order, &mut metadata),
            );
            snapshot_order += 1;
        }

        Self {
            generation: NEXT_CATALOG_GENERATION.fetch_add(1, AtomicOrdering::Relaxed),
            items,
            item_indices,
            colliding_item_indices,
            tab_workspace_ids,
            focused_workspace_id: session.focused_workspace_id.clone(),
            focused_tab_id: session.focused_tab_id.clone(),
            focused_pane_id: session.focused_pane_id.clone(),
        }
    }

    pub fn get(&self, id: &PaletteItemId) -> Option<&PaletteItem> {
        let key = id.stable_key();
        let index = *self.item_indices.get(&key)?;
        if self
            .items
            .get(index)
            .is_some_and(|record| record.item.id() == *id)
        {
            return self.items.get(index).map(|record| &record.item);
        }
        self.colliding_item_indices
            .get(&key)?
            .iter()
            .copied()
            .find(|index| self.items[*index].item.id() == *id)
            .and_then(|index| self.items.get(index))
            .map(|record| &record.item)
    }

    pub(crate) fn get_ranked(&self, ranked: &RankedPaletteItem) -> Option<&PaletteItem> {
        if ranked.catalog_generation != self.generation {
            return None;
        }
        ranked
            .item_index
            .and_then(|index| self.items.get(index))
            .map(|record| &record.item)
            .filter(|item| item.id() == ranked.id)
    }

    pub fn parse_query(&self, raw: &str) -> ParsedPaletteQuery {
        parse_query(raw)
    }

    /// Whether this item is the workspace, tab, or agent the session is currently focused on.
    pub fn is_focused(&self, id: &PaletteItemId) -> bool {
        match id {
            PaletteItemId::Workspace(workspace_id) => {
                self.focused_workspace_id.as_deref() == Some(workspace_id)
            }
            PaletteItemId::Tab(tab_id) => self.focused_tab_id.as_deref() == Some(tab_id),
            PaletteItemId::Agent(pane_id) => self.focused_pane_id.as_deref() == Some(pane_id),
            PaletteItemId::Command(_) => false,
        }
    }

    pub fn rank(&self, raw_query: &str) -> Vec<RankedPaletteItem> {
        let parsed = self.parse_query(raw_query);
        let candidates = self.candidates(parsed.scope);
        if parsed.fuzzy_query.is_empty() {
            return self.rank_empty(parsed.scope, candidates);
        }

        let normalized_query = normalize_for_match(&parsed.fuzzy_query);
        let pattern = Pattern::new(
            &parsed.fuzzy_query,
            CaseMatching::Smart,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );
        let mut matcher = Matcher::default();
        let mut utf32_buffer = Vec::new();
        let mut live = Vec::new();
        let mut commands = Vec::new();
        for index in candidates {
            let record = &self.items[index];
            let Some(key) = rank_record(
                record,
                &normalized_query,
                &pattern,
                &mut matcher,
                &mut utf32_buffer,
            ) else {
                continue;
            };
            match record.item.source() {
                PaletteSource::Live => live.push((index, key)),
                PaletteSource::Commands => commands.push((index, key)),
            }
        }
        self.rank_sections(live, commands)
    }

    fn candidates(&self, scope: PaletteScope) -> Vec<usize> {
        self.items
            .iter()
            .enumerate()
            .filter_map(|(index, record)| match (scope, &record.item) {
                (PaletteScope::Default, PaletteItem::Command(_))
                | (PaletteScope::Commands, PaletteItem::Command(_)) => Some(index),
                (PaletteScope::Default, PaletteItem::Entity(entity)) => match entity.kind {
                    EntityKind::Workspace | EntityKind::Agent => Some(index),
                    EntityKind::Tab => matches!(&entity.id, PaletteItemId::Tab(tab_id)
                        if self.focused_workspace_id.as_deref()
                            == self.tab_workspace_ids.get(tab_id).map(String::as_str))
                    .then_some(index),
                },
                (PaletteScope::Workspaces, PaletteItem::Entity(entity))
                    if entity.kind == EntityKind::Workspace =>
                {
                    Some(index)
                }
                (PaletteScope::Tabs, PaletteItem::Entity(entity))
                    if entity.kind == EntityKind::Tab =>
                {
                    Some(index)
                }
                (PaletteScope::Agents, PaletteItem::Entity(entity))
                    if entity.kind == EntityKind::Agent =>
                {
                    Some(index)
                }
                _ => None,
            })
            .collect()
    }

    fn rank_empty(&self, scope: PaletteScope, candidates: Vec<usize>) -> Vec<RankedPaletteItem> {
        match scope {
            PaletteScope::Default => {
                let mut ordered = Vec::with_capacity(8);
                // Blocked agents need attention before anything else, including the focused
                // chain: an empty query is a "what should I look at" browse.
                self.push_matching(&mut ordered, |item| {
                    matches!(
                        item,
                        PaletteItem::Entity(EntityItem {
                            kind: EntityKind::Agent,
                            status: AgentStatus::Blocked,
                            ..
                        })
                    )
                });
                self.push_focused(&mut ordered, |item| {
                    matches!(item, PaletteItem::Entity(EntityItem { kind: EntityKind::Workspace, id: PaletteItemId::Workspace(id), .. }) if self.focused_workspace_id.as_deref() == Some(id))
                });
                self.push_focused(&mut ordered, |item| {
                    matches!(item, PaletteItem::Entity(EntityItem { kind: EntityKind::Tab, id: PaletteItemId::Tab(id), .. })
                        if self.focused_tab_id.as_deref() == Some(id)
                            && self.focused_workspace_id.as_deref()
                                == self.tab_workspace_ids.get(id).map(String::as_str))
                });
                self.push_focused(&mut ordered, |item| {
                    matches!(item, PaletteItem::Entity(EntityItem { kind: EntityKind::Agent, id: PaletteItemId::Agent(id), .. }) if self.focused_pane_id.as_deref() == Some(id))
                });
                self.push_matching(&mut ordered, |item| {
                    matches!(
                        item,
                        PaletteItem::Entity(EntityItem {
                            kind: EntityKind::Workspace,
                            ..
                        })
                    )
                });
                self.push_matching(&mut ordered, |item| {
                    matches!(item, PaletteItem::Entity(EntityItem { kind: EntityKind::Tab, id: PaletteItemId::Tab(id), .. })
                        if self.focused_workspace_id.as_deref()
                            == self.tab_workspace_ids.get(id).map(String::as_str))
                });
                self.push_matching(&mut ordered, |item| {
                    matches!(
                        item,
                        PaletteItem::Entity(EntityItem {
                            kind: EntityKind::Agent,
                            ..
                        })
                    )
                });
                ordered.truncate(8);
                let mut ranked = ordered
                    .into_iter()
                    .map(|index| self.ranked(index, self.empty_rank_key(index)))
                    .collect::<Vec<_>>();
                ranked.extend(
                    self.rank_empty_commands(
                        candidates
                            .into_iter()
                            .filter(|index| {
                                matches!(self.items[*index].item, PaletteItem::Command(_))
                            })
                            .collect(),
                    ),
                );
                ranked
            }
            PaletteScope::Commands => self.rank_empty_commands(candidates),
            PaletteScope::Workspaces | PaletteScope::Tabs | PaletteScope::Agents => candidates
                .into_iter()
                .map(|index| self.ranked(index, self.empty_rank_key(index)))
                .collect(),
        }
    }

    fn push_focused(&self, ordered: &mut Vec<usize>, matches: impl Fn(&PaletteItem) -> bool) {
        if let Some(index) = self.items.iter().position(|record| matches(&record.item)) {
            if !ordered.contains(&index) {
                ordered.push(index);
            }
        }
    }

    fn push_matching(&self, ordered: &mut Vec<usize>, matches: impl Fn(&PaletteItem) -> bool) {
        if ordered.len() == 8 {
            return;
        }
        for (index, record) in self.items.iter().enumerate() {
            if matches(&record.item) && !ordered.contains(&index) {
                ordered.push(index);
                if ordered.len() == 8 {
                    return;
                }
            }
        }
    }

    fn rank_empty_commands(&self, candidates: Vec<usize>) -> Vec<RankedPaletteItem> {
        let mut commands = candidates
            .into_iter()
            .map(|index| (index, self.empty_rank_key(index)))
            .collect::<Vec<_>>();
        commands.sort_by(|left, right| self.compare_ranked(left, right));
        commands
            .into_iter()
            .map(|(index, key)| self.ranked(index, key))
            .collect()
    }

    fn rank_sections(
        &self,
        mut live: Vec<(usize, RankKey)>,
        mut commands: Vec<(usize, RankKey)>,
    ) -> Vec<RankedPaletteItem> {
        live.sort_by(|left, right| self.compare_ranked(left, right));
        commands.sort_by(|left, right| self.compare_ranked(left, right));
        let commands_first = match (live.first(), commands.first()) {
            (Some(live), Some(command)) => self.compare_ranked(command, live).is_lt(),
            (None, Some(_)) => true,
            _ => false,
        };
        let sections = if commands_first {
            [commands, live]
        } else {
            [live, commands]
        };
        sections
            .into_iter()
            .flatten()
            .map(|(index, key)| self.ranked(index, key))
            .collect()
    }

    fn compare_ranked(&self, left: &(usize, RankKey), right: &(usize, RankKey)) -> Ordering {
        left.1.cmp(&right.1)
    }

    fn ranked(&self, index: usize, rank_key: RankKey) -> RankedPaletteItem {
        RankedPaletteItem {
            id: self.items[index].item.id(),
            catalog_generation: self.generation,
            item_index: Some(index),
            rank_key,
        }
    }

    fn empty_rank_key(&self, index: usize) -> RankKey {
        self.items[index].rank_key(0, 0)
    }

    #[cfg(test)]
    fn unique_entity_metadata_bytes_for_test(&self) -> usize {
        let mut seen = HashSet::new();
        let mut bytes = 0;
        let mut add = |value: &Arc<str>| {
            if seen.insert(Arc::as_ptr(value) as *const () as usize) {
                bytes += value.len();
            }
        };
        for record in &self.items {
            let (
                PaletteItem::Entity(entity),
                SearchFields::Entity {
                    normalized_breadcrumb,
                    ..
                },
            ) = (&record.item, &record.search_fields)
            else {
                continue;
            };
            add(&entity.label);
            add(&entity.breadcrumb);
            add(&record.normalized_primary);
            add(normalized_breadcrumb);
        }
        bytes
    }
}

impl CatalogRecord {
    fn rank_key(&self, match_class: u8, fuzzy_score: u32) -> RankKey {
        let (source_tie, catalog_priority) = match &self.item {
            PaletteItem::Entity(entity) => (
                0,
                match entity.kind {
                    EntityKind::Workspace => 0,
                    EntityKind::Agent => 1,
                    EntityKind::Tab => 2,
                },
            ),
            PaletteItem::Command(command) => (1, command.priority),
        };
        RankKey {
            match_class,
            fuzzy_score: Reverse(fuzzy_score),
            source_tie,
            catalog_priority,
            normalized_primary: self.normalized_primary.clone(),
            stable_id: self.item.id(),
        }
    }
}

fn rank_record(
    record: &CatalogRecord,
    normalized_query: &str,
    pattern: &Pattern,
    matcher: &mut Matcher,
    utf32_buffer: &mut Vec<char>,
) -> Option<RankKey> {
    if record.normalized_primary.as_ref() == normalized_query {
        return Some(record.rank_key(0, 0));
    }
    if token_prefix(&record.normalized_primary, normalized_query) {
        return Some(record.rank_key(1, 0));
    }
    match &record.search_fields {
        SearchFields::Command { primary, secondary } => {
            fuzzy_score(primary, pattern, matcher, utf32_buffer)
                .map(|score| record.rank_key(2, score))
                .or_else(|| {
                    fuzzy_score(secondary, pattern, matcher, utf32_buffer)
                        .map(|score| record.rank_key(3, score))
                })
        }
        SearchFields::Entity {
            primary,
            breadcrumb,
            status,
            ..
        } => fuzzy_score(primary, pattern, matcher, utf32_buffer)
            .map(|score| record.rank_key(2, score))
            .or_else(|| {
                let breadcrumb_score = fuzzy_score(breadcrumb, pattern, matcher, utf32_buffer);
                let status_score = fuzzy_score(status, pattern, matcher, utf32_buffer);
                breadcrumb_score
                    .into_iter()
                    .chain(status_score)
                    .max()
                    .map(|score| record.rank_key(4, score))
            }),
    }
}

fn fuzzy_score(
    document: &str,
    pattern: &Pattern,
    matcher: &mut Matcher,
    utf32_buffer: &mut Vec<char>,
) -> Option<u32> {
    pattern.score(Utf32Str::new(document, utf32_buffer), matcher)
}

fn token_prefix(document: &str, query: &str) -> bool {
    document
        .split_whitespace()
        .any(|token| token.starts_with(query))
}

fn normalize_for_match(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .map(nucleo_matcher::chars::normalize)
        .filter(|character| !matches!(*character, '\u{0300}'..='\u{036f}'))
        .collect()
}

// This is deliberately a prefix operation: metadata may originate outside the plugin, so previews
// and matching normalization never walk or copy an arbitrary suffix. Agent fallback selection is
// the sole exception: it scans each unique source field to preserve its full trimmed-empty meaning.
fn bounded_preview(value: &str) -> String {
    bounded_preview_parts(&[value]).0
}

// `str::floor_char_boundary` is newer than this crate's declared Rust version. A UTF-8 code
// point has at most three continuation bytes, so this preserves its behavior with bounded work.
fn floor_char_boundary_compat(value: &str, index: usize) -> usize {
    let mut index = index.min(value.len());
    for _ in 0..3 {
        if index == 0
            || index == value.len()
            || value.as_bytes()[index] & 0b1100_0000 != 0b1000_0000
        {
            break;
        }
        index -= 1;
    }
    index
}

fn bounded_preview_parts(parts: &[&str]) -> (String, usize, usize) {
    let mut output = String::new();
    let mut inspected_bytes = 0;
    let mut inspected_graphemes = 0;
    let mut copied_graphemes = 0;
    let mut omitted = false;

    'parts: for (part_index, part) in parts.iter().enumerate() {
        let bytes_left = MAX_METADATA_SOURCE_BYTES.saturating_sub(inspected_bytes);
        if bytes_left == 0 {
            omitted = parts[part_index..].iter().any(|part| !part.is_empty());
            break;
        }
        let end = floor_char_boundary_compat(part, part.len().min(bytes_left));
        let prefix = &part[..end];
        let source_cut = end < part.len();
        inspected_bytes += prefix.len();
        let mut graphemes = prefix.grapheme_indices(true).peekable();
        while let Some((start, grapheme)) = graphemes.next() {
            inspected_graphemes += 1;
            // A prefix-ending cluster may continue past the source ceiling. Omit it rather than
            // copy a valid UTF-8 fragment which is not a complete grapheme cluster.
            if source_cut && start + grapheme.len() == prefix.len() {
                omitted = true;
                break 'parts;
            }
            if copied_graphemes == MAX_METADATA_GRAPHEMES {
                omitted = true;
                break 'parts;
            }
            output.push_str(grapheme);
            copied_graphemes += 1;
            if copied_graphemes == MAX_METADATA_GRAPHEMES
                && (graphemes.peek().is_some()
                    || source_cut
                    || parts[part_index + 1..].iter().any(|part| !part.is_empty()))
            {
                omitted = true;
                break 'parts;
            }
        }
        if source_cut {
            omitted = true;
            break;
        }
    }
    if omitted {
        output.push('…');
    }
    (output, inspected_bytes, inspected_graphemes)
}

#[cfg(test)]
fn bounded_preview_with_inspections_for_test(value: &str) -> (String, usize) {
    let (preview, inspected_bytes, _) = bounded_preview_parts(&[value]);
    (preview, inspected_bytes)
}

#[cfg(test)]
fn bounded_preview_with_bounds_for_test(value: &str) -> (String, usize, usize) {
    bounded_preview_parts(&[value])
}

fn catalog_record(
    item: PaletteItem,
    snapshot_order: usize,
    metadata: &mut MetadataInterner,
) -> CatalogRecord {
    let normalized_primary = match &item {
        PaletteItem::Command(command) => Arc::from(normalize_for_match(&command.title)),
        PaletteItem::Entity(entity) => metadata.normalized(&entity.label),
    };
    let search_fields = match &item {
        PaletteItem::Command(command) => SearchFields::Command {
            primary: command.title.clone(),
            secondary: command_metadata_document(command),
        },
        PaletteItem::Entity(entity) => SearchFields::Entity {
            primary: entity.label.clone(),
            breadcrumb: entity.breadcrumb.clone(),
            normalized_breadcrumb: metadata.normalized(&entity.breadcrumb),
            lowercase_primary: metadata.lowercase(&entity.label),
            lowercase_breadcrumb: metadata.lowercase(&entity.breadcrumb),
            status: entity.status.as_str(),
        },
    };
    CatalogRecord {
        item,
        normalized_primary,
        search_fields,
        snapshot_order,
    }
}

fn command_metadata_document(command: &CommandSpec) -> String {
    let mut parts = Vec::with_capacity(3 + command.aliases.len());
    parts.extend(command.aliases.iter().map(String::as_str));
    if let Some(description) = command.description.as_deref() {
        parts.push(description);
    }
    parts.push(command.category.as_str());
    if let Some(plugin_name) = command.plugin_name.as_deref() {
        parts.push(plugin_name);
    }
    parts.join(" ")
}

fn insert_record(
    items: &mut Vec<CatalogRecord>,
    item_indices: &mut HashMap<String, usize>,
    colliding_item_indices: &mut HashMap<String, Vec<usize>>,
    record: CatalogRecord,
) {
    let id = record.item.id();
    let key = id.stable_key();
    match item_indices.entry(key.clone()) {
        std::collections::hash_map::Entry::Vacant(entry) => {
            entry.insert(items.len());
            items.push(record);
        }
        std::collections::hash_map::Entry::Occupied(entry) => {
            let first_index = *entry.get();
            if items[first_index].item.id() == id
                || colliding_item_indices.get(&key).is_some_and(|indices| {
                    indices.iter().any(|index| items[*index].item.id() == id)
                })
            {
                return;
            }
            colliding_item_indices
                .entry(key)
                .or_insert_with(|| vec![first_index])
                .push(items.len());
            items.push(record);
        }
    }
}

fn valid_agent(
    agent: &AgentInfo,
    workspaces: &HashMap<&str, &WorkspaceInfo>,
    tabs: &HashMap<&str, &TabInfo>,
    panes: &HashMap<&str, &PaneInfo>,
) -> bool {
    let Some(tab) = tabs.get(agent.tab_id.as_str()) else {
        return false;
    };
    let Some(pane) = panes.get(agent.pane_id.as_str()) else {
        return false;
    };
    workspaces.contains_key(agent.workspace_id.as_str())
        && tab.workspace_id == agent.workspace_id
        && pane.workspace_id == agent.workspace_id
        && pane.tab_id == agent.tab_id
}

fn agent_label(agent: &AgentInfo, metadata: &mut MetadataInterner) -> Arc<str> {
    for value in [
        agent.name.as_deref(),
        agent.display_agent.as_deref(),
        agent.agent.as_deref(),
        agent.terminal_title_stripped.as_deref(),
        agent.title.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        // Fallback precedence is determined from the complete source. The preview must not turn
        // an overlong whitespace-only value into a nonempty ellipsis and hide a later field.
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return metadata.preview(trimmed);
        }
    }
    metadata.preview(&agent.pane_id)
}

fn entity_for_agent(
    agent: &AgentInfo,
    breadcrumb: Arc<str>,
    metadata: &mut MetadataInterner,
) -> EntityItem {
    EntityItem {
        id: PaletteItemId::Agent(agent.pane_id.clone()),
        kind: EntityKind::Agent,
        label: agent_label(agent, metadata),
        breadcrumb,
        status: agent.agent_status,
        stable_suffix: None,
    }
}

fn set_duplicate_stable_suffixes(items: &mut [CatalogRecord]) {
    let mut counts = HashMap::<(EntityKind, Arc<str>, Arc<str>), usize>::new();
    for record in items.iter() {
        let (
            PaletteItem::Entity(entity),
            SearchFields::Entity {
                lowercase_primary,
                lowercase_breadcrumb,
                ..
            },
        ) = (&record.item, &record.search_fields)
        else {
            continue;
        };
        *counts
            .entry((
                entity.kind,
                lowercase_primary.clone(),
                lowercase_breadcrumb.clone(),
            ))
            .or_default() += 1;
    }
    for record in items {
        let key = match (&record.item, &record.search_fields) {
            (
                PaletteItem::Entity(entity),
                SearchFields::Entity {
                    lowercase_primary,
                    lowercase_breadcrumb,
                    ..
                },
            ) => (
                entity.kind,
                lowercase_primary.clone(),
                lowercase_breadcrumb.clone(),
            ),
            _ => continue,
        };
        if counts.get(&key).is_some_and(|count| *count > 1) {
            let PaletteItem::Entity(entity) = &mut record.item else {
                unreachable!("the duplicate key only belongs to entities")
            };
            entity.stable_suffix = Some(match &entity.id {
                PaletteItemId::Workspace(id)
                | PaletteItemId::Tab(id)
                | PaletteItemId::Agent(id) => id.clone(),
                PaletteItemId::Command(_) => unreachable!("entity IDs are always live"),
            });
        }
    }
}

fn available_commands(
    runtime: &RuntimeSnapshot,
    plugins: &[InstalledPluginInfo],
    actions: &[PluginActionInfo],
) -> Vec<CommandSpec> {
    let mut commands = CoreCommand::all()
        .iter()
        .filter_map(|command| command.spec(runtime))
        .collect::<Vec<_>>();
    let mut seen_ids = HashSet::new();
    for action in actions {
        let qualified_id = action.qualified_id();
        if qualified_id == SELF_OPEN_ACTION || !seen_ids.insert(qualified_id.clone()) {
            continue;
        }
        let Some(plugin) = plugins
            .iter()
            .find(|plugin| plugin.plugin_id == action.plugin_id)
        else {
            continue;
        };
        if !plugin.enabled
            || !plugin.warnings.is_empty()
            || !supports_macos(plugin.platforms.as_deref())
            || !supports_macos(action.platforms.as_deref())
            || !context_matches(runtime, &action.contexts)
        {
            continue;
        }
        let title = bounded_preview(&action.title);
        let description = action.description.as_deref().map(bounded_preview);
        let plugin_name = bounded_preview(&plugin.name);
        commands.push(CommandSpec {
            id: CommandId::Plugin(qualified_id),
            title,
            description,
            category: CommandCategory::Plugin,
            aliases: vec![plugin_name.clone()],
            priority: 20,
            steps: Vec::new(),
            plugin_name: Some(plugin_name),
        });
    }
    commands
}

fn supports_macos(platforms: Option<&[PluginPlatform]>) -> bool {
    platforms.is_none_or(|platforms| platforms.contains(&PluginPlatform::Macos))
}

fn context_matches(runtime: &RuntimeSnapshot, contexts: &[PluginActionContext]) -> bool {
    if contexts.is_empty() {
        return true;
    }
    contexts.iter().any(|context| match context {
        PluginActionContext::Global => true,
        PluginActionContext::Workspace => runtime.session.focused_workspace_id.is_some(),
        PluginActionContext::Tab => runtime.session.focused_tab_id.is_some(),
        PluginActionContext::Pane => runtime.session.focused_pane_id.is_some(),
        PluginActionContext::Selection => runtime.has_selection(),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::context::parse_invocation_context;
    use crate::model::{
        ApiCapabilities, PluginActionContext, PluginActionListResult, PluginListResult,
        PluginPlatform, SessionSnapshotResult, SuccessEnvelope, WorktreeListResult,
    };
    use unicode_segmentation::UnicodeSegmentation;

    fn fixture<T: serde::de::DeserializeOwned>(raw: &str) -> T {
        serde_json::from_str(raw).unwrap()
    }

    fn inputs() -> (
        RuntimeSnapshot,
        Vec<InstalledPluginInfo>,
        Vec<PluginActionInfo>,
    ) {
        let session: SuccessEnvelope<SessionSnapshotResult> =
            fixture(include_str!("../tests/fixtures/session-snapshot.json"));
        let worktrees: SuccessEnvelope<WorktreeListResult> =
            fixture(include_str!("../tests/fixtures/worktree-list.json"));
        let plugins: SuccessEnvelope<PluginListResult> =
            fixture(include_str!("../tests/fixtures/plugin-list.json"));
        let actions: SuccessEnvelope<PluginActionListResult> =
            fixture(include_str!("../tests/fixtures/plugin-actions.json"));
        (
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
            },
            plugins.result.plugins,
            actions.result.actions,
        )
    }

    fn catalog_command(catalog: &PaletteCatalog, id: CommandId) -> Option<&CommandSpec> {
        match catalog.get(&PaletteItemId::Command(id)) {
            Some(PaletteItem::Command(command)) => Some(command),
            Some(PaletteItem::Entity(_)) | None => None,
        }
    }

    fn plugin_command_ids(catalog: &PaletteCatalog) -> Vec<String> {
        catalog
            .items
            .iter()
            .filter_map(|record| match &record.item {
                PaletteItem::Command(CommandSpec {
                    id: CommandId::Plugin(id),
                    ..
                }) => Some(id.clone()),
                PaletteItem::Command(CommandSpec {
                    id: CommandId::Core(_),
                    ..
                })
                | PaletteItem::Entity(_) => None,
            })
            .collect()
    }

    fn runtime_with_two_workspaces() -> RuntimeSnapshot {
        let (mut runtime, _, _) = inputs();
        let mut tab = runtime.session.tabs[0].clone();
        tab.tab_id = "w1:t2".into();
        tab.label = "Second".into();
        runtime.session.tabs.push(tab);
        let mut pane = runtime.session.panes[0].clone();
        pane.pane_id = "w1:p1".into();
        pane.tab_id = "w1:t2".into();
        runtime.session.panes.push(pane);
        let mut agent = runtime.session.agents[0].clone();
        agent.pane_id = "w1:p1".into();
        agent.tab_id = "w1:t2".into();
        runtime.session.agents.push(agent);
        let mut remote_agent = runtime.session.agents[0].clone();
        remote_agent.workspace_id = "ws-2".into();
        remote_agent.tab_id = "tab-2".into();
        remote_agent.pane_id = "pane-e".into();
        runtime.session.agents.push(remote_agent);
        runtime.session.workspaces[0].workspace_id = "w1".into();
        runtime.session.workspaces[1].workspace_id = "w2".into();
        runtime.session.focused_workspace_id = Some("w1".into());
        runtime.session.focused_tab_id = Some("w1:t1".into());
        runtime.session.focused_pane_id = Some("w1:p0".into());
        for tab in &mut runtime.session.tabs {
            tab.workspace_id = if tab.workspace_id == "ws-1" {
                "w1"
            } else {
                "w2"
            }
            .into();
            tab.tab_id = match tab.tab_id.as_str() {
                "tab-1" => "w1:t1".into(),
                "tab-2" => "w2:t1".into(),
                other => other.into(),
            };
        }
        for pane in &mut runtime.session.panes {
            pane.workspace_id = if pane.workspace_id == "ws-1" {
                "w1"
            } else {
                "w2"
            }
            .into();
            pane.tab_id = if pane.tab_id == "tab-1" {
                "w1:t1"
            } else if pane.tab_id == "tab-2" {
                "w2:t1"
            } else {
                &pane.tab_id
            }
            .into();
            pane.pane_id = match pane.pane_id.as_str() {
                "pane-a" => "w1:p0".into(),
                "pane-e" => "w2:p1".into(),
                other => other.into(),
            };
        }
        for agent in &mut runtime.session.agents {
            agent.workspace_id = if agent.workspace_id == "ws-1" {
                "w1"
            } else {
                "w2"
            }
            .into();
            agent.tab_id = if agent.tab_id == "tab-1" {
                "w1:t1"
            } else if agent.tab_id == "tab-2" {
                "w2:t1"
            } else {
                &agent.tab_id
            }
            .into();
            agent.pane_id = match agent.pane_id.as_str() {
                "pane-a" => "w1:p0".into(),
                "pane-e" => "w2:p1".into(),
                other => other.into(),
            };
        }
        runtime
    }

    fn runtime_with_broken_agent_relationships() -> RuntimeSnapshot {
        let mut runtime = runtime_with_two_workspaces();
        let mut missing_pane = runtime.session.agents[0].clone();
        missing_pane.pane_id = "missing-pane".into();
        runtime.session.agents.push(missing_pane);
        let mut wrong_tab_pane = runtime.session.agents[0].clone();
        wrong_tab_pane.pane_id = "wrong-tab-pane".into();
        runtime.session.panes.push(crate::model::PaneInfo {
            pane_id: "wrong-tab-pane".into(),
            tab_id: "w2:t1".into(),
            ..runtime.session.panes[0].clone()
        });
        runtime.session.agents.push(wrong_tab_pane);
        let mut orphan_tab = runtime.session.tabs[0].clone();
        orphan_tab.tab_id = "orphan-tab".into();
        orphan_tab.workspace_id = "missing-workspace".into();
        runtime.session.tabs.push(orphan_tab);
        runtime
    }

    fn agent_fields(
        name: Option<&str>,
        display_agent: Option<&str>,
        agent_kind: Option<&str>,
        terminal_title_stripped: Option<&str>,
        title: Option<&str>,
    ) -> crate::model::AgentInfo {
        let mut agent = runtime_with_two_workspaces().session.agents.remove(0);
        agent.pane_id = "w1:p1".into();
        agent.tab_id = "w1:t1".into();
        agent.name = name.map(str::to_owned);
        agent.display_agent = display_agent.map(str::to_owned);
        agent.agent = agent_kind.map(str::to_owned);
        agent.terminal_title_stripped = terminal_title_stripped.map(str::to_owned);
        agent.title = title.map(str::to_owned);
        agent
    }

    #[test]
    fn empty_query_ranks_blocked_agents_before_the_focused_chain() {
        let (mut runtime, _, _) = inputs();
        let builder = runtime
            .session
            .agents
            .iter_mut()
            .find(|agent| agent.name.as_deref() == Some("builder"))
            .expect("fixture has the non-focused builder agent");
        builder.agent_status = AgentStatus::Blocked;
        let catalog = PaletteCatalog::new(&runtime, &[], &[]);

        let ranked = catalog.rank("");
        assert_eq!(ranked[0].id, PaletteItemId::Agent("pane-d".into()));
        assert_eq!(ranked[1].id, PaletteItemId::Workspace("ws-1".into()));

        // Typed queries keep pure relevance ordering: blocked status must not reorder them.
        let (unblocked_runtime, _, _) = inputs();
        let unblocked = PaletteCatalog::new(&unblocked_runtime, &[], &[]);
        let queried_ids = |catalog: &PaletteCatalog| {
            catalog
                .rank("docs")
                .into_iter()
                .map(|ranked| ranked.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(queried_ids(&catalog), queried_ids(&unblocked));
    }

    #[test]
    fn multi_workspace_snapshot_indexes_workspace_local_tab_and_global_agent_items() {
        let runtime = runtime_with_two_workspaces();
        let catalog = PaletteCatalog::new(&runtime, &[], &[]);

        assert!(catalog
            .get(&PaletteItemId::Workspace("w1".into()))
            .is_some());
        assert!(catalog
            .get(&PaletteItemId::Workspace("w2".into()))
            .is_some());
        assert!(catalog.get(&PaletteItemId::Tab("w1:t1".into())).is_some());
        assert!(catalog.get(&PaletteItemId::Tab("w2:t1".into())).is_some());
        assert!(catalog.get(&PaletteItemId::Agent("w1:p1".into())).is_some());
        assert!(catalog.get(&PaletteItemId::Agent("w2:p1".into())).is_some());
    }

    #[test]
    fn malformed_agent_tab_workspace_and_pane_relationships_are_excluded() {
        let runtime = runtime_with_broken_agent_relationships();
        let catalog = PaletteCatalog::new(&runtime, &[], &[]);

        assert!(catalog
            .get(&PaletteItemId::Agent("missing-pane".into()))
            .is_none());
        assert!(catalog
            .get(&PaletteItemId::Agent("wrong-tab-pane".into()))
            .is_none());
        assert!(catalog
            .get(&PaletteItemId::Tab("orphan-tab".into()))
            .is_none());
    }

    #[test]
    fn duplicate_entity_labels_and_breadcrumbs_get_precomputed_stable_suffixes() {
        let mut runtime = runtime_with_two_workspaces();
        let mut pane = runtime.session.panes[0].clone();
        pane.pane_id = "duplicate-pane".into();
        runtime.session.panes.push(pane);
        let mut agent = runtime.session.agents[0].clone();
        agent.pane_id = "duplicate-pane".into();
        runtime.session.agents.push(agent);
        let catalog = PaletteCatalog::new(&runtime, &[], &[]);

        let first = catalog.get(&PaletteItemId::Agent("w1:p0".into())).unwrap();
        let duplicate = catalog
            .get(&PaletteItemId::Agent("duplicate-pane".into()))
            .unwrap();
        assert!(
            matches!(first, PaletteItem::Entity(EntityItem { stable_suffix: Some(id), .. }) if id == "w1:p0")
        );
        assert!(
            matches!(duplicate, PaletteItem::Entity(EntityItem { stable_suffix: Some(id), .. }) if id == "duplicate-pane")
        );
    }

    #[test]
    fn duplicate_suffixes_use_lowercase_without_accent_folding() {
        let mut runtime = runtime_with_two_workspaces();
        runtime.session.agents[0].name = Some("Café".into());
        let mut pane = runtime.session.panes[0].clone();
        pane.pane_id = "cafe-pane".into();
        runtime.session.panes.push(pane);
        let mut agent = runtime.session.agents[0].clone();
        agent.pane_id = "cafe-pane".into();
        agent.name = Some("Cafe".into());
        runtime.session.agents.push(agent);
        let catalog = PaletteCatalog::new(&runtime, &[], &[]);

        for pane_id in ["w1:p0", "cafe-pane"] {
            assert!(matches!(
                catalog.get(&PaletteItemId::Agent(pane_id.into())),
                Some(PaletteItem::Entity(EntityItem {
                    stable_suffix: None,
                    ..
                }))
            ));
        }
    }

    #[test]
    fn ranked_lookup_rejects_a_result_from_another_catalog_generation() {
        let runtime = runtime_with_two_workspaces();
        let catalog = PaletteCatalog::new(&runtime, &[], &[]);
        let id = PaletteItemId::Workspace("w1".into());
        let ranked = RankedPaletteItem {
            id: id.clone(),
            catalog_generation: catalog.generation,
            item_index: catalog.item_indices[&id.stable_key()].into(),
            rank_key: RankKey {
                match_class: 0,
                fuzzy_score: Reverse(0),
                source_tie: 0,
                catalog_priority: 0,
                normalized_primary: Arc::from(""),
                stable_id: id,
            },
        };
        let replacement = PaletteCatalog::new(&runtime, &[], &[]);

        assert!(catalog.get_ranked(&ranked).is_some());
        assert!(replacement.get_ranked(&ranked).is_none());
    }

    #[test]
    fn agent_primary_label_uses_the_fixed_trimmed_fallback_order() {
        let cases = [
            (
                agent_fields(
                    Some("  named  "),
                    Some("display"),
                    Some("kind"),
                    Some("term"),
                    Some("title"),
                ),
                "named",
            ),
            (
                agent_fields(
                    None,
                    Some(" display "),
                    Some("kind"),
                    Some("term"),
                    Some("title"),
                ),
                "display",
            ),
            (
                agent_fields(None, None, Some(" kind "), Some("term"), Some("title")),
                "kind",
            ),
            (
                agent_fields(None, None, None, Some(" term "), Some("title")),
                "term",
            ),
            (
                agent_fields(None, None, None, None, Some(" title ")),
                "title",
            ),
            (agent_fields(None, None, None, None, None), "w1:p1"),
        ];
        for (agent, expected) in cases {
            let mut metadata = MetadataInterner::default();
            assert_eq!(
                entity_for_agent(&agent, Arc::from("Workspace / Tab"), &mut metadata)
                    .label
                    .as_ref(),
                expected
            );
        }
    }

    #[test]
    fn agent_label_selects_a_full_nonempty_source_before_previewing_it() {
        let whitespace_only_name = " ".repeat(MAX_METADATA_SOURCE_BYTES + 1);
        let agent = agent_fields(
            Some(&whitespace_only_name),
            Some(" display agent "),
            Some("kind"),
            None,
            None,
        );
        let mut metadata = MetadataInterner::default();
        assert_eq!(agent_label(&agent, &mut metadata).as_ref(), "display agent");

        let selected_name = format!("  selected{}", "x".repeat(MAX_METADATA_SOURCE_BYTES));
        let agent = agent_fields(
            Some(&selected_name),
            Some("display agent"),
            None,
            None,
            None,
        );
        let label = agent_label(&agent, &mut metadata);
        assert!(label.starts_with("selected"));
        assert!(label.ends_with('…'));
        assert_eq!(label.graphemes(true).count(), MAX_METADATA_GRAPHEMES + 1);
    }

    fn plugin_action(
        action_id: &str,
        title: &str,
        contexts: Vec<PluginActionContext>,
    ) -> PluginActionInfo {
        PluginActionInfo {
            plugin_id: "demo.tools".into(),
            action_id: action_id.into(),
            title: title.into(),
            description: Some("Dynamic action".into()),
            contexts,
            command: vec!["bin/demo".into()],
            platforms: Some(vec![PluginPlatform::Macos]),
        }
    }

    #[test]
    fn capability_gated_agent_commands_enter_the_registry_only_with_valid_context() {
        let (mut runtime, plugins, actions) = inputs();
        runtime.capabilities = ApiCapabilities {
            typed_agent_start: false,
            agent_prompt: false,
        };
        let old = PaletteCatalog::new(&runtime, &plugins, &actions);
        assert!(catalog_command(&old, CommandId::Core(CoreCommand::PromptFocusedAgent)).is_none());
        assert!(catalog_command(&old, CommandId::Core(CoreCommand::StartAgent)).is_none());

        runtime.capabilities = ApiCapabilities {
            typed_agent_start: true,
            agent_prompt: true,
        };
        let capable_with_agent = PaletteCatalog::new(&runtime, &plugins, &actions);
        assert!(catalog_command(
            &capable_with_agent,
            CommandId::Core(CoreCommand::PromptFocusedAgent)
        )
        .is_some());
        assert!(catalog_command(
            &capable_with_agent,
            CommandId::Core(CoreCommand::StartAgent)
        )
        .is_none());

        runtime
            .session
            .agents
            .retain(|agent| agent.pane_id != "pane-a");
        let capable_without_agent = PaletteCatalog::new(&runtime, &plugins, &actions);
        assert!(catalog_command(
            &capable_without_agent,
            CommandId::Core(CoreCommand::PromptFocusedAgent)
        )
        .is_none());
        assert!(catalog_command(
            &capable_without_agent,
            CommandId::Core(CoreCommand::StartAgent)
        )
        .is_some());
    }

    #[test]
    fn plugin_join_excludes_every_ineligible_action_and_deduplicates_ids() {
        let (runtime, plugins, mut actions) = inputs();
        actions.push(plugin_action("run", "Duplicate run", vec![]));
        let registry = PaletteCatalog::new(&runtime, &plugins, &actions);
        assert_eq!(plugin_command_ids(&registry).len(), 2);
        let run = catalog_command(&registry, CommandId::Plugin("demo.tools.run".into())).unwrap();
        assert_eq!(run.title, "Run demo");
        assert_eq!(run.description.as_deref(), Some("Runs a demo"));
        assert_eq!(run.aliases, vec!["Demo Tools"]);
        assert_eq!(run.plugin_name.as_deref(), Some("Demo Tools"));
        assert_eq!(run.category, CommandCategory::Plugin);
        assert_eq!(run.priority, 20);
        assert!(run.steps.is_empty());
        for excluded in [
            "demo.tools.linux",
            "demo.disabled.run",
            "demo.broken.run",
            "herdr.command-palette.open",
        ] {
            assert!(catalog_command(&registry, CommandId::Plugin(excluded.into())).is_none());
        }
    }

    #[test]
    fn plugin_action_metadata_is_bounded_before_catalog_search_documents() {
        const RESPONSE_METADATA_BYTES: usize = 4 * 1024 * 1024;
        let field_bytes = RESPONSE_METADATA_BYTES / 3 - 32;
        let (runtime, mut plugins, mut actions) = inputs();
        let title = format!("title {}", "t".repeat(field_bytes));
        let description = format!("description {}", "d".repeat(field_bytes));
        let plugin_name = format!("plugin {}", "p".repeat(field_bytes));
        assert!(title.len() + description.len() + plugin_name.len() <= RESPONSE_METADATA_BYTES);
        plugins[0].name = plugin_name.clone();
        actions[0].title = title.clone();
        actions[0].description = Some(description.clone());

        let catalog = PaletteCatalog::new(&runtime, &plugins, &actions);
        let id = CommandId::Plugin("demo.tools.run".into());
        let command = catalog_command(&catalog, id.clone()).unwrap();
        let bounded_title = bounded_preview(&title);
        let bounded_description = bounded_preview(&description);
        let bounded_plugin_name = bounded_preview(&plugin_name);

        assert_eq!(command.id, id);
        assert_eq!(command.title, bounded_title);
        assert_eq!(
            command.description.as_deref(),
            Some(bounded_description.as_str())
        );
        assert_eq!(command.aliases, vec![bounded_plugin_name.clone()]);
        assert_eq!(
            command.plugin_name.as_deref(),
            Some(bounded_plugin_name.as_str())
        );
        for value in [
            command.title.as_str(),
            command.description.as_deref().unwrap(),
            command.aliases[0].as_str(),
            command.plugin_name.as_deref().unwrap(),
        ] {
            assert!(value.len() <= MAX_METADATA_SOURCE_BYTES);
            assert!(value.graphemes(true).count() <= MAX_METADATA_GRAPHEMES + 1);
            assert!(value.ends_with('…'));
        }

        let record = catalog
            .items
            .iter()
            .find(|record| record.item.id() == PaletteItemId::Command(id.clone()))
            .unwrap();
        assert_eq!(
            record.normalized_primary.as_ref(),
            normalize_for_match(&bounded_title)
        );
        let SearchFields::Command { primary, secondary } = &record.search_fields else {
            panic!("plugin action must have command search fields");
        };
        assert_eq!(primary, &bounded_title);
        assert_eq!(
            secondary,
            &format!("{bounded_plugin_name} {bounded_description} plugin {bounded_plugin_name}")
        );
        assert!(record.normalized_primary.len() <= MAX_METADATA_SOURCE_BYTES);
        assert!(secondary.len() < 4 * MAX_METADATA_SOURCE_BYTES);
        assert_eq!(catalog.rank(">title")[0].id, PaletteItemId::Command(id));
    }

    #[test]
    fn plugin_and_action_platforms_must_each_allow_macos() {
        let (runtime, mut plugins, _) = inputs();
        let actions = vec![plugin_action("platform", "Platform", vec![])];
        plugins[0].platforms = Some(vec![PluginPlatform::Linux]);
        assert!(catalog_command(
            &PaletteCatalog::new(&runtime, &plugins, &actions),
            CommandId::Plugin("demo.tools.platform".into()),
        )
        .is_none());
        plugins[0].platforms = None;
        assert!(catalog_command(
            &PaletteCatalog::new(&runtime, &plugins, &actions),
            CommandId::Plugin("demo.tools.platform".into()),
        )
        .is_some());
    }

    #[test]
    fn plugin_contexts_are_alternatives_and_empty_or_global_matches_everywhere() {
        let (mut runtime, plugins, _) = inputs();
        runtime.session.focused_workspace_id = None;
        runtime.session.focused_tab_id = None;
        runtime.session.focused_pane_id = None;
        runtime.invocation.selected_text = None;
        let mut actions = vec![
            plugin_action("empty", "Empty", vec![]),
            plugin_action("global", "Global", vec![PluginActionContext::Global]),
            plugin_action(
                "workspace",
                "Workspace",
                vec![PluginActionContext::Workspace],
            ),
            plugin_action("tab", "Tab", vec![PluginActionContext::Tab]),
            plugin_action("pane", "Pane", vec![PluginActionContext::Pane]),
            plugin_action(
                "selection",
                "Selection",
                vec![PluginActionContext::Selection],
            ),
            plugin_action(
                "any",
                "Any",
                vec![PluginActionContext::Selection, PluginActionContext::Pane],
            ),
        ];
        let ids = |runtime: &RuntimeSnapshot, actions: &[PluginActionInfo]| {
            plugin_command_ids(&PaletteCatalog::new(runtime, &plugins, actions))
        };
        assert_eq!(
            ids(&runtime, &actions),
            vec!["demo.tools.empty", "demo.tools.global"]
        );

        runtime.session.focused_pane_id = Some("pane-a".into());
        assert!(ids(&runtime, &actions).contains(&"demo.tools.any".into()));
        runtime.session.focused_pane_id = None;
        runtime.invocation.selected_text = Some(String::new());
        assert!(!ids(&runtime, &actions).contains(&"demo.tools.selection".into()));
        runtime.invocation.selected_text = Some("selected".into());
        let selected = ids(&runtime, &actions);
        assert!(selected.contains(&"demo.tools.selection".into()));
        assert!(selected.contains(&"demo.tools.any".into()));

        actions.retain(|action| action.action_id != "any");
        runtime.session.focused_workspace_id = Some("ws-1".into());
        runtime.session.focused_tab_id = Some("tab-1".into());
        let present = ids(&runtime, &actions);
        assert!(present.contains(&"demo.tools.workspace".into()));
        assert!(present.contains(&"demo.tools.tab".into()));
    }

    #[test]
    fn ranking_searches_unicode_alias_description_category_and_plugin_name() {
        let (runtime, plugins, mut actions) = inputs();
        actions.push(plugin_action("unicode", "Déployer aperçu", vec![]));
        let catalog = PaletteCatalog::new(&runtime, &plugins, &actions);

        assert_eq!(
            catalog.rank(">ws")[0].id,
            PaletteItemId::Command(CommandId::Core(CoreCommand::SwitchWorkspace))
        );
        assert_eq!(
            catalog.rank(">Runs a demo")[0].id,
            PaletteItemId::Command(CommandId::Plugin("demo.tools.run".into()))
        );
        assert!(catalog.rank(">Demo Tools").iter().any(|ranked| {
            ranked.id == PaletteItemId::Command(CommandId::Plugin("demo.tools.run".into()))
        }));
        assert_eq!(
            catalog.rank(">Déployer")[0].id,
            PaletteItemId::Command(CommandId::Plugin("demo.tools.unicode".into()))
        );
        assert!(catalog
            .rank(">plugin")
            .iter()
            .all(|ranked| matches!(&ranked.id, PaletteItemId::Command(CommandId::Plugin(_)))));
        assert!(catalog.rank(">this-will-not-match-any-command").is_empty());
    }

    #[test]
    fn empty_command_scope_uses_priority_then_unicode_lowercase_title_then_id() {
        let (runtime, plugins, mut actions) = inputs();
        actions.push(plugin_action("z", "Same", vec![]));
        actions.push(plugin_action("a", "Same", vec![]));
        let catalog = PaletteCatalog::new(&runtime, &plugins, &actions);
        let ranked = catalog.rank(">");
        let ordering = ranked
            .iter()
            .map(|item| {
                let PaletteItemId::Command(id) = &item.id else {
                    panic!("command scope returned a live item");
                };
                let spec = catalog_command(&catalog, id.clone()).unwrap();
                (
                    spec.priority,
                    spec.title.to_lowercase(),
                    spec.stable_id().to_owned(),
                )
            })
            .collect::<Vec<_>>();
        assert!(ordering.windows(2).all(|pair| pair[0] <= pair[1]));
        let ties = ranked
            .iter()
            .filter_map(|item| match &item.id {
                PaletteItemId::Command(CommandId::Plugin(id))
                    if id == "demo.tools.a" || id == "demo.tools.z" =>
                {
                    Some(id.clone())
                }
                PaletteItemId::Command(CommandId::Plugin(_))
                | PaletteItemId::Command(CommandId::Core(_))
                | PaletteItemId::Workspace(_)
                | PaletteItemId::Tab(_)
                | PaletteItemId::Agent(_) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(ties, vec!["demo.tools.a", "demo.tools.z"]);
    }

    #[test]
    fn palette_catalog_keeps_colliding_typed_command_ids_retrievable_and_rank_resolvable() {
        let (runtime, mut plugins, mut actions) = inputs();
        let mut plugin = plugins[0].clone();
        plugin.plugin_id = "workspace".into();
        plugins.push(plugin);
        let mut action = actions[0].clone();
        action.plugin_id = "workspace".into();
        action.action_id = "switch".into();
        actions.push(action);
        let catalog = PaletteCatalog::new(&runtime, &plugins, &actions);
        let ids = [
            PaletteItemId::Command(CommandId::Core(CoreCommand::SwitchWorkspace)),
            PaletteItemId::Command(CommandId::Plugin("workspace.switch".into())),
        ];

        for id in ids {
            assert_eq!(catalog.get(&id).map(PaletteItem::id), Some(id.clone()));
            let index = catalog
                .items
                .iter()
                .position(|record| record.item.id() == id)
                .expect("colliding command ID should be retained");
            let ranked = RankedPaletteItem {
                id: id.clone(),
                catalog_generation: catalog.generation,
                item_index: Some(index),
                rank_key: RankKey {
                    match_class: 0,
                    fuzzy_score: Reverse(0),
                    source_tie: 0,
                    catalog_priority: 0,
                    normalized_primary: Arc::from(""),
                    stable_id: id.clone(),
                },
            };
            assert_eq!(catalog.get_ranked(&ranked).map(PaletteItem::id), Some(id));
        }
    }

    fn ids(ranked: Vec<RankedPaletteItem>) -> Vec<PaletteItemId> {
        ranked.into_iter().map(|item| item.id).collect()
    }

    fn catalog_with_local_and_remote_tabs() -> PaletteCatalog {
        let mut runtime = runtime_with_two_workspaces();
        for tab in &mut runtime.session.tabs {
            tab.label = "Deploy".into();
        }
        PaletteCatalog::new(&runtime, &[], &[])
    }

    #[test]
    fn parses_all_scope_prefixes_with_leading_whitespace_and_ascii_case() {
        assert_eq!(
            parse_query(" > rename"),
            ParsedPaletteQuery {
                scope: PaletteScope::Commands,
                fuzzy_query: "rename".into(),
            }
        );
        assert_eq!(
            parse_query(" SPACE: docs"),
            ParsedPaletteQuery {
                scope: PaletteScope::Workspaces,
                fuzzy_query: "docs".into(),
            }
        );
        assert_eq!(
            parse_query("Tab: deploy"),
            ParsedPaletteQuery {
                scope: PaletteScope::Tabs,
                fuzzy_query: "deploy".into(),
            }
        );
        assert_eq!(
            parse_query("agent: reviewer"),
            ParsedPaletteQuery {
                scope: PaletteScope::Agents,
                fuzzy_query: "reviewer".into(),
            }
        );
    }

    #[test]
    fn unknown_word_colon_remains_a_normal_query() {
        assert_eq!(
            parse_query("status:blocked"),
            ParsedPaletteQuery {
                scope: PaletteScope::Default,
                fuzzy_query: "status:blocked".into(),
            }
        );
    }

    #[test]
    fn default_scope_excludes_remote_tabs_but_tab_scope_includes_every_valid_tab() {
        let catalog = catalog_with_local_and_remote_tabs();
        assert!(!ids(catalog.rank("deploy")).contains(&PaletteItemId::Tab("w2:t1".into())));
        assert!(ids(catalog.rank("tab: deploy")).contains(&PaletteItemId::Tab("w2:t1".into())));
    }

    #[test]
    fn default_scope_has_no_tabs_when_the_focused_workspace_is_absent() {
        let mut runtime = runtime_with_two_workspaces();
        runtime.session.focused_workspace_id = None;
        let catalog = PaletteCatalog::new(&runtime, &[], &[]);

        assert!(ids(catalog.rank("deploy"))
            .iter()
            .all(|id| !matches!(id, PaletteItemId::Tab(_))));
    }

    #[test]
    fn exact_primary_matching_normalizes_unicode_case_and_combining_accents() {
        let mut runtime = runtime_with_two_workspaces();
        runtime.session.workspaces[0].label = "Cafe\u{301}".into();
        let catalog = PaletteCatalog::new(&runtime, &[], &[]);

        assert_eq!(
            ids(catalog.rank("space: CAFÉ")),
            vec![PaletteItemId::Workspace("w1".into())]
        );
    }

    #[test]
    fn exact_and_token_prefix_primary_matches_beat_weaker_fuzzy_matches() {
        let mut runtime = runtime_with_two_workspaces();
        runtime.session.workspaces[0].label = "Alpha".into();
        runtime.session.workspaces[1].label = "Alphabet soup".into();
        runtime.session.tabs[0].label = "Axlpha".into();
        let catalog = PaletteCatalog::new(&runtime, &[], &[]);

        assert_eq!(
            ids(catalog.rank("alpha")),
            vec![
                PaletteItemId::Workspace("w1".into()),
                PaletteItemId::Workspace("w2".into()),
                PaletteItemId::Tab("w1:t1".into()),
                PaletteItemId::Agent("pane-d".into()),
                PaletteItemId::Agent("w1:p0".into()),
                PaletteItemId::Agent("w1:p1".into()),
                PaletteItemId::Agent("w2:p1".into()),
                PaletteItemId::Tab("w1:t2".into()),
                PaletteItemId::Command(CommandId::Core(CoreCommand::RunCommandInNewPane)),
            ]
        );
    }

    #[test]
    fn equal_live_primary_matches_use_workspace_agent_tab_priority() {
        let mut runtime = runtime_with_two_workspaces();
        runtime.session.workspaces[0].label = "Shared".into();
        runtime.session.tabs[0].label = "shared".into();
        runtime.session.agents[0].name = Some("SHARED".into());
        let catalog = PaletteCatalog::new(&runtime, &[], &[]);

        let ranked = catalog.rank("shared");
        assert!(ranked[..3]
            .iter()
            .all(|item| item.rank_key.match_class == 0 && item.rank_key.fuzzy_score == Reverse(0)));
        let ranked_ids = ids(ranked);

        assert_eq!(
            ranked_ids[..3],
            [
                PaletteItemId::Workspace("w1".into()),
                PaletteItemId::Agent("w1:p0".into()),
                PaletteItemId::Tab("w1:t1".into()),
            ]
        );
    }

    #[test]
    fn strong_command_primary_or_metadata_match_beats_live_secondary_match() {
        let (runtime, mut plugins, mut actions) = inputs();
        let mut plugin = plugins.remove(0);
        plugin.plugin_id = "working.plugin".into();
        plugin.name = "Working plugin".into();
        plugins.push(plugin);
        let mut action = actions.remove(0);
        action.plugin_id = "working.plugin".into();
        action.action_id = "run".into();
        action.title = "Working command".into();
        actions = vec![action];
        let catalog = PaletteCatalog::new(&runtime, &plugins, &actions);
        let ranked = ids(catalog.rank("working"));

        assert_eq!(
            ranked[0],
            PaletteItemId::Command(CommandId::Plugin("working.plugin.run".into()))
        );
        assert!(
            ranked
                .iter()
                .position(|id| matches!(id, PaletteItemId::Agent(_)))
                .unwrap()
                > ranked
                    .iter()
                    .position(|id| matches!(id, PaletteItemId::Command(_)))
                    .unwrap()
        );
    }

    #[test]
    fn entity_breadcrumb_and_status_match_only_as_class_four_recall() {
        let mut runtime = runtime_with_two_workspaces();
        let mut pane = runtime.session.panes[0].clone();
        pane.pane_id = "w1:p2".into();
        runtime.session.panes.push(pane);
        let mut agent = runtime.session.agents[0].clone();
        agent.pane_id = "w1:p2".into();
        agent.name = Some("Coder".into());
        runtime.session.agents.push(agent);
        let catalog = PaletteCatalog::new(&runtime, &[], &[]);

        assert_eq!(
            ids(catalog.rank("agent: code")),
            vec![
                PaletteItemId::Agent("w1:p2".into()),
                PaletteItemId::Agent("pane-d".into()),
                PaletteItemId::Agent("w1:p0".into()),
            ]
        );
    }

    #[test]
    fn empty_default_order_is_focused_then_snapshot_order_and_capped_at_eight() {
        let mut runtime = runtime_with_two_workspaces();
        let mut workspace = runtime.session.workspaces[0].clone();
        workspace.workspace_id = "w3".into();
        workspace.label = "Third".into();
        runtime.session.workspaces.push(workspace);
        let catalog = PaletteCatalog::new(&runtime, &[], &[]);

        assert_eq!(
            ids(catalog.rank(""))
                .into_iter()
                .filter(|id| !matches!(id, PaletteItemId::Command(_)))
                .collect::<Vec<_>>(),
            vec![
                PaletteItemId::Workspace("w1".into()),
                PaletteItemId::Tab("w1:t1".into()),
                PaletteItemId::Agent("w1:p0".into()),
                PaletteItemId::Workspace("w2".into()),
                PaletteItemId::Workspace("w3".into()),
                PaletteItemId::Tab("w1:t2".into()),
                PaletteItemId::Agent("pane-d".into()),
                PaletteItemId::Agent("w1:p1".into()),
            ]
        );
    }

    #[test]
    fn empty_explicit_tab_scope_returns_all_workspace_tabs_without_cap() {
        let mut runtime = runtime_with_two_workspaces();
        for number in 3..=10 {
            let mut tab = runtime.session.tabs[1].clone();
            tab.tab_id = format!("w2:t{number}");
            tab.label = format!("Remote {number}");
            runtime.session.tabs.push(tab);
        }
        let catalog = PaletteCatalog::new(&runtime, &[], &[]);

        assert_eq!(
            ids(catalog.rank("tab:")),
            vec![
                PaletteItemId::Tab("w1:t1".into()),
                PaletteItemId::Tab("w2:t1".into()),
                PaletteItemId::Tab("w1:t2".into()),
                PaletteItemId::Tab("w2:t3".into()),
                PaletteItemId::Tab("w2:t4".into()),
                PaletteItemId::Tab("w2:t5".into()),
                PaletteItemId::Tab("w2:t6".into()),
                PaletteItemId::Tab("w2:t7".into()),
                PaletteItemId::Tab("w2:t8".into()),
                PaletteItemId::Tab("w2:t9".into()),
                PaletteItemId::Tab("w2:t10".into()),
            ]
        );
    }

    #[test]
    fn catalog_bounds_and_interns_adversarial_external_metadata() {
        let mut runtime = runtime_with_two_workspaces();
        runtime.session.workspaces[0].label =
            format!("{} suffix is never inspected", "🙂".repeat(1_048_576));
        runtime.session.tabs[0].label = format!("{} ignored", "界".repeat(2_048));
        for number in 0..64 {
            let mut tab = runtime.session.tabs[0].clone();
            tab.tab_id = format!("w1:adversarial-tab-{number}");
            tab.label = format!("tab-{number}");
            runtime.session.tabs.push(tab);
        }
        runtime.session.agents[0].name = None;
        runtime.session.agents[0].display_agent = None;
        runtime.session.agents[0].agent = None;
        runtime.session.agents[0].terminal_title_stripped = None;
        runtime.session.agents[0].title = Some(format!("{} ignored", "é".repeat(4_096)));
        for number in 0..128 {
            let mut pane = runtime.session.panes[0].clone();
            pane.pane_id = format!("shared-pane-{number}");
            runtime.session.panes.push(pane);
            let mut agent = runtime.session.agents[0].clone();
            agent.pane_id = format!("shared-pane-{number}");
            agent.name = Some(format!("agent-{number}"));
            runtime.session.agents.push(agent);
        }

        let catalog = PaletteCatalog::new(&runtime, &[], &[]);
        let workspace = match catalog.get(&PaletteItemId::Workspace("w1".into())).unwrap() {
            PaletteItem::Entity(entity) => entity,
            PaletteItem::Command(_) => panic!("workspace must be an entity"),
        };
        assert_eq!(workspace.label.graphemes(true).count(), 257);
        assert!(workspace.label.ends_with('…'));
        let tab = match catalog.get(&PaletteItemId::Tab("w1:t1".into())).unwrap() {
            PaletteItem::Entity(entity) => entity,
            PaletteItem::Command(_) => panic!("tab must be an entity"),
        };
        assert_eq!(tab.label.graphemes(true).count(), 257);
        assert!(tab.label.ends_with('…'));
        let title_agent = match catalog.get(&PaletteItemId::Agent("w1:p0".into())).unwrap() {
            PaletteItem::Entity(entity) => entity,
            PaletteItem::Command(_) => panic!("agent must be an entity"),
        };
        assert_eq!(title_agent.label.graphemes(true).count(), 257);
        let first = match catalog
            .get(&PaletteItemId::Agent("shared-pane-0".into()))
            .unwrap()
        {
            PaletteItem::Entity(entity) => entity,
            PaletteItem::Command(_) => panic!("agent must be an entity"),
        };
        let last = match catalog
            .get(&PaletteItemId::Agent("shared-pane-127".into()))
            .unwrap()
        {
            PaletteItem::Entity(entity) => entity,
            PaletteItem::Command(_) => panic!("agent must be an entity"),
        };
        assert!(Arc::ptr_eq(&workspace.label, &tab.breadcrumb));
        assert!(Arc::ptr_eq(&first.breadcrumb, &last.breadcrumb));
        assert!(catalog.unique_entity_metadata_bytes_for_test() < 32 * 1024);
        assert_eq!(
            bounded_preview_with_inspections_for_test(&runtime.session.workspaces[0].label).1,
            4096
        );
        assert!(catalog
            .rank("agent-127")
            .iter()
            .any(|ranked| ranked.id == PaletteItemId::Agent("shared-pane-127".into())));
        assert_eq!(catalog.rank("tab:").len(), 67);
    }

    #[test]
    fn bounded_metadata_preview_drops_clusters_cut_by_the_source_byte_ceiling() {
        let combining = format!("xy{}", "\u{0301}".repeat(4_096));
        let zwj = format!("x{}", "👩\u{200d}".repeat(1_024));

        for value in [&combining, &zwj] {
            let (preview, inspected_bytes, inspected_graphemes) =
                bounded_preview_with_bounds_for_test(value);
            assert_eq!(preview, "x…");
            assert!(preview.is_char_boundary(preview.len()));
            assert!(inspected_bytes <= MAX_METADATA_SOURCE_BYTES);
            assert!(inspected_graphemes <= MAX_METADATA_GRAPHEMES);
            assert!(
                preview.len() <= 4,
                "owned preview was {} bytes",
                preview.len()
            );
        }
    }

    #[test]
    fn bounded_metadata_preview_floors_source_byte_cuts_at_char_boundaries() {
        assert_eq!(floor_char_boundary_compat("ASCII", 3), 3);
        assert_eq!(floor_char_boundary_compat("é", 1), 0);
        assert_eq!(floor_char_boundary_compat("e\u{0301}", 2), 1);
        assert_eq!(floor_char_boundary_compat("👩\u{200d}💻", 6), 4);
    }

    #[test]
    fn bounded_metadata_preview_preserves_normal_unicode_exactly() {
        assert_eq!(bounded_preview("Café 👩\u{200d}💻"), "Café 👩\u{200d}💻");
    }

    #[test]
    fn nonempty_query_ties_use_score_priority_title_and_stable_id_deterministically() {
        let (runtime, plugins, mut actions) = inputs();
        actions.push(plugin_action("z", "Identical", vec![]));
        actions.push(plugin_action("a", "Identical", vec![]));
        let catalog = PaletteCatalog::new(&runtime, &plugins, &actions);
        let ranked = catalog.rank(">Identical");
        let ids = ranked
            .iter()
            .map(|item| match &item.id {
                PaletteItemId::Command(id) => id.stable_id(),
                PaletteItemId::Workspace(_) | PaletteItemId::Tab(_) | PaletteItemId::Agent(_) => {
                    panic!("commands scope returned a live item")
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["demo.tools.a", "demo.tools.z"]);
    }
}
