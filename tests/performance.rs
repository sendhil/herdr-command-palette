use std::hint::black_box;
use std::time::{Duration, Instant};

use herdr_command_palette::{
    context::parse_invocation_context,
    model::{
        AgentInfo, AgentStatus, ApiCapabilities, InstalledPluginInfo, PaneInfo,
        PluginActionContext, PluginActionInfo, PluginPlatform, RuntimeSnapshot,
        SessionSnapshotResult, SuccessEnvelope, TabInfo, WorkspaceInfo, WorktreeListResult,
    },
    registry::{PaletteCatalog, PaletteItemId, PaletteSource},
    state::PaletteState,
    view::compute_layout_with_stats,
};
use ratatui::layout::Rect;

const SAMPLES: usize = 7;
const LAYOUT_ITERATIONS: usize = 100;
const VISIBLE_ROWS: usize = 7;
const DEFAULT_LIVE_ITEM_COUNT: usize = 8;
const MAX_STALE_RANKED_SKIPS: usize = 16;
const LAYOUT_AREA: Rect = Rect::new(0, 0, 80, 20);

struct MixedInputs {
    runtime: RuntimeSnapshot,
    plugins: Vec<InstalledPluginInfo>,
    actions: Vec<PluginActionInfo>,
    workspace_count: usize,
    tab_count: usize,
    agent_count: usize,
    command_count: usize,
}

#[derive(Clone, Copy)]
struct Measurements {
    build: Duration,
    default_rank: Duration,
    tab_rank: Duration,
    agent_rank: Duration,
    layout_100: Duration,
    head_layout_ranked_entries: usize,
    tail_layout_ranked_entries: usize,
    stale_layout_ranked_entries: usize,
}

fn base_runtime() -> RuntimeSnapshot {
    let session: SuccessEnvelope<SessionSnapshotResult> =
        serde_json::from_str(include_str!("fixtures/session-snapshot.json")).unwrap();
    let worktrees: SuccessEnvelope<WorktreeListResult> =
        serde_json::from_str(include_str!("fixtures/worktree-list.json")).unwrap();
    RuntimeSnapshot {
        session: session.result.snapshot,
        worktrees: worktrees.result.worktrees,
        invocation: parse_invocation_context(include_str!(
            "fixtures/plugin-invocation-context.json"
        ))
        .unwrap(),
        capabilities: ApiCapabilities {
            typed_agent_start: true,
            agent_prompt: true,
        },
    }
}

fn benchmark_plugin() -> InstalledPluginInfo {
    InstalledPluginInfo {
        plugin_id: "benchmark.actions".into(),
        name: "Benchmark Actions".into(),
        version: "1.0.0".into(),
        min_herdr_version: "0.7.4".into(),
        description: Some("Deterministic release benchmark plugin".into()),
        manifest_path: "/benchmark/herdr-plugin.toml".into(),
        plugin_root: "/benchmark".into(),
        enabled: true,
        platforms: Some(vec![PluginPlatform::Macos]),
        warnings: Vec::new(),
    }
}

/// Builds valid, mixed live and command sources.  Tab labels intentionally repeat within a
/// workspace, agent labels repeat within a tab, and every workspace after the first is remote.
fn mixed_inputs(size: usize) -> MixedInputs {
    let workspace_count = size / 100;
    let tabs_per_workspace = 4;
    let agents_per_tab = 3;
    let tab_count = workspace_count * tabs_per_workspace;
    let agent_count = tab_count * agents_per_tab;
    let mut runtime = base_runtime();
    runtime.session.workspaces = (0..workspace_count)
        .map(|workspace| WorkspaceInfo {
            workspace_id: format!("workspace-{workspace:03}"),
            number: workspace + 1,
            label: format!("Workspace {}", workspace % 3),
            focused: workspace == 0,
            pane_count: tabs_per_workspace * agents_per_tab,
            tab_count: tabs_per_workspace,
            active_tab_id: format!("tab-{workspace:03}-00"),
            agent_status: AgentStatus::Working,
            worktree: None,
        })
        .collect();
    runtime.session.tabs = (0..workspace_count)
        .flat_map(|workspace| {
            (0..tabs_per_workspace).map(move |tab| TabInfo {
                tab_id: format!("tab-{workspace:03}-{tab:02}"),
                workspace_id: format!("workspace-{workspace:03}"),
                number: tab + 1,
                label: "Deployment".into(),
                focused: workspace == 0 && tab == 0,
                pane_count: agents_per_tab,
                agent_status: AgentStatus::Working,
            })
        })
        .collect();
    runtime.session.panes = (0..workspace_count)
        .flat_map(|workspace| {
            (0..tabs_per_workspace).flat_map(move |tab| {
                (0..agents_per_tab).map(move |agent| PaneInfo {
                    pane_id: format!("pane-{workspace:03}-{tab:02}-{agent}"),
                    terminal_id: format!("terminal-{workspace:03}-{tab:02}-{agent}"),
                    workspace_id: format!("workspace-{workspace:03}"),
                    tab_id: format!("tab-{workspace:03}-{tab:02}"),
                    focused: workspace == 0 && tab == 0 && agent == 0,
                    cwd: Some(format!("/benchmark/workspace-{workspace:03}")),
                    foreground_cwd: None,
                    label: Some("reviewer".into()),
                    agent: Some("benchmark-agent".into()),
                    title: None,
                    terminal_title: None,
                    terminal_title_stripped: None,
                    display_agent: Some("Benchmark Agent".into()),
                    agent_status: AgentStatus::Working,
                    revision: 1,
                })
            })
        })
        .collect();
    runtime.session.agents = (0..workspace_count)
        .flat_map(|workspace| {
            (0..tabs_per_workspace).flat_map(move |tab| {
                (0..agents_per_tab).map(move |agent| AgentInfo {
                    terminal_id: format!("terminal-{workspace:03}-{tab:02}-{agent}"),
                    name: Some("reviewer".into()),
                    agent: Some("benchmark-agent".into()),
                    title: None,
                    terminal_title: None,
                    terminal_title_stripped: None,
                    display_agent: Some("Benchmark Agent".into()),
                    agent_status: AgentStatus::Working,
                    workspace_id: format!("workspace-{workspace:03}"),
                    tab_id: format!("tab-{workspace:03}-{tab:02}"),
                    pane_id: format!("pane-{workspace:03}-{tab:02}-{agent}"),
                    focused: workspace == 0 && tab == 0 && agent == 0,
                    launch_pending: false,
                    interactive_ready: true,
                    state_change_seq: 1,
                    cwd: Some(format!("/benchmark/workspace-{workspace:03}")),
                    foreground_cwd: None,
                    revision: 1,
                })
            })
        })
        .collect();
    runtime.session.layouts.clear();
    runtime.session.focused_workspace_id = Some("workspace-000".into());
    runtime.session.focused_tab_id = Some("tab-000-00".into());
    runtime.session.focused_pane_id = Some("pane-000-00-0".into());

    let plugin = benchmark_plugin();
    let live_count = workspace_count + tab_count + agent_count;
    let core_command_count = PaletteCatalog::new(&runtime, &[], &[]).rank(">").len();
    let action_count = size
        .checked_sub(live_count + core_command_count)
        .expect("benchmark size must fit mixed live and core candidates");
    let actions = (0..action_count)
        .map(|index| PluginActionInfo {
            plugin_id: plugin.plugin_id.clone(),
            action_id: format!("action-{index:05}"),
            title: format!("Deploy plugin action {index:05}"),
            description: Some("Plausible plugin action used by the release benchmark".into()),
            contexts: vec![PluginActionContext::Global],
            command: vec!["echo".into(), "benchmark".into()],
            platforms: Some(vec![PluginPlatform::Macos]),
        })
        .collect::<Vec<_>>();

    MixedInputs {
        runtime,
        plugins: vec![plugin],
        command_count: core_command_count + actions.len(),
        actions,
        workspace_count,
        tab_count,
        agent_count,
    }
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn measure(inputs: &MixedInputs) -> Measurements {
    let started = Instant::now();
    let catalog = black_box(PaletteCatalog::new(
        black_box(&inputs.runtime),
        black_box(&inputs.plugins),
        black_box(&inputs.actions),
    ));
    let build = started.elapsed();

    let default_ranked = catalog.rank("");
    let default_sources = default_ranked
        .iter()
        .map(|ranked| {
            catalog
                .get(&ranked.id)
                .expect("ranked default item must be present in its catalog")
                .source()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        default_ranked.len(),
        inputs.command_count + DEFAULT_LIVE_ITEM_COUNT
    );
    assert_eq!(
        default_sources
            .iter()
            .filter(|source| **source == PaletteSource::Commands)
            .count(),
        inputs.command_count
    );
    assert_eq!(
        default_sources
            .iter()
            .filter(|source| **source == PaletteSource::Live)
            .count(),
        DEFAULT_LIVE_ITEM_COUNT
    );
    assert!(default_ranked.iter().any(|ranked| {
        matches!(ranked.id, PaletteItemId::Workspace(_))
            && catalog
                .get(&ranked.id)
                .is_some_and(|item| item.source() == PaletteSource::Live)
    }));
    let focused_workspace_id = inputs
        .runtime
        .session
        .focused_workspace_id
        .as_deref()
        .expect("deterministic benchmark fixture must have a focused workspace");
    assert!(default_ranked.iter().any(|ranked| {
        matches!(
            &ranked.id,
            PaletteItemId::Tab(id)
                if inputs.runtime.session.tabs.iter().any(|tab| {
                    tab.tab_id.as_str() == id.as_str()
                        && tab.workspace_id.as_str() == focused_workspace_id
                })
        ) && catalog
            .get(&ranked.id)
            .is_some_and(|item| item.source() == PaletteSource::Live)
    }));
    assert!(default_ranked.iter().any(|ranked| {
        matches!(ranked.id, PaletteItemId::Agent(_))
            && catalog
                .get(&ranked.id)
                .is_some_and(|item| item.source() == PaletteSource::Live)
    }));

    let started = Instant::now();
    let ranked = black_box(catalog.rank(black_box("deploy")));
    let default_rank = started.elapsed();
    assert!(!ranked.is_empty());

    let started = Instant::now();
    assert_eq!(
        black_box(catalog.rank(black_box("tab: deploy"))).len(),
        inputs.tab_count
    );
    let tab_rank = started.elapsed();

    let started = Instant::now();
    assert_eq!(
        black_box(catalog.rank(black_box("agent: reviewer"))).len(),
        inputs.agent_count
    );
    let agent_rank = started.elapsed();

    let state = PaletteState::ready("deploy", ranked);
    let (head_layout, head_stats) = compute_layout_with_stats(LAYOUT_AREA, &state, &catalog);
    assert_eq!(head_layout.action_rows().count(), VISIBLE_ROWS);

    let mut tail = state.clone();
    tail.scroll_offset = tail.ranked.len().saturating_sub(VISIBLE_ROWS);
    let (tail_layout, tail_stats) = compute_layout_with_stats(LAYOUT_AREA, &tail, &catalog);
    assert_eq!(tail_layout.action_rows().count(), VISIBLE_ROWS);

    let stale_catalog = PaletteCatalog::new(&inputs.runtime, &inputs.plugins, &inputs.actions);
    let mut stale = state.clone();
    stale.ranked = stale_catalog.rank("deploy");
    let (_, stale_stats) = compute_layout_with_stats(LAYOUT_AREA, &stale, &catalog);

    let started = Instant::now();
    for _ in 0..LAYOUT_ITERATIONS {
        black_box(compute_layout_with_stats(
            LAYOUT_AREA,
            black_box(&state),
            black_box(&catalog),
        ));
    }
    let layout_100 = started.elapsed();

    Measurements {
        build,
        default_rank,
        tab_rank,
        agent_rank,
        layout_100,
        head_layout_ranked_entries: head_stats.ranked_entries_examined,
        tail_layout_ranked_entries: tail_stats.ranked_entries_examined,
        stale_layout_ranked_entries: stale_stats.ranked_entries_examined,
    }
}

fn median_samples(inputs: &MixedInputs) -> Measurements {
    let samples = (0..SAMPLES).map(|_| measure(inputs)).collect::<Vec<_>>();
    let first = samples[0];
    assert!(samples.iter().all(|sample| {
        sample.head_layout_ranked_entries == first.head_layout_ranked_entries
            && sample.tail_layout_ranked_entries == first.tail_layout_ranked_entries
            && sample.stale_layout_ranked_entries == first.stale_layout_ranked_entries
    }));
    Measurements {
        build: median(samples.iter().map(|sample| sample.build).collect()),
        default_rank: median(samples.iter().map(|sample| sample.default_rank).collect()),
        tab_rank: median(samples.iter().map(|sample| sample.tab_rank).collect()),
        agent_rank: median(samples.iter().map(|sample| sample.agent_rank).collect()),
        layout_100: median(samples.iter().map(|sample| sample.layout_100).collect()),
        ..first
    }
}

#[test]
#[ignore = "release-only benchmark harness; run cargo test --release --locked --test performance -- --ignored --nocapture"]
fn ranks_and_lays_out_large_mixed_catalogs() {
    let mut layout_operations = Vec::new();
    for size in [1_000, 5_000] {
        let inputs = mixed_inputs(size);
        let warm_catalog = PaletteCatalog::new(&inputs.runtime, &inputs.plugins, &inputs.actions);
        for _ in 0..3 {
            black_box(warm_catalog.rank(black_box("deploy")));
        }

        assert_eq!(warm_catalog.rank("space:").len(), inputs.workspace_count);
        assert_eq!(warm_catalog.rank("tab:").len(), inputs.tab_count);
        assert_eq!(warm_catalog.rank("agent:").len(), inputs.agent_count);
        assert_eq!(warm_catalog.rank(">").len(), inputs.command_count);
        assert_eq!(
            inputs.workspace_count + inputs.tab_count + inputs.agent_count + inputs.command_count,
            size
        );

        let measurements = median_samples(&inputs);
        assert_eq!(measurements.head_layout_ranked_entries, VISIBLE_ROWS);
        assert_eq!(measurements.tail_layout_ranked_entries, VISIBLE_ROWS);
        assert!(measurements.stale_layout_ranked_entries <= VISIBLE_ROWS + MAX_STALE_RANKED_SKIPS);
        layout_operations.push((
            measurements.head_layout_ranked_entries,
            measurements.tail_layout_ranked_entries,
            measurements.stale_layout_ranked_entries,
        ));
        println!(
            "items={size} workspaces={} tabs={} agents={} commands={} build_ms={:.3} default_rank_ms={:.3} tab_rank_ms={:.3} agent_rank_ms={:.3} layout_100_ms={:.3} head_layout_ranked_entries={} tail_layout_ranked_entries={} stale_layout_ranked_entries={}",
            inputs.workspace_count,
            inputs.tab_count,
            inputs.agent_count,
            inputs.command_count,
            measurements.build.as_secs_f64() * 1_000.0,
            measurements.default_rank.as_secs_f64() * 1_000.0,
            measurements.tab_rank.as_secs_f64() * 1_000.0,
            measurements.agent_rank.as_secs_f64() * 1_000.0,
            measurements.layout_100.as_secs_f64() * 1_000.0,
            measurements.head_layout_ranked_entries,
            measurements.tail_layout_ranked_entries,
            measurements.stale_layout_ranked_entries,
        );
    }
    assert_eq!(layout_operations, vec![layout_operations[0]; 2]);
}
