use thiserror::Error;

pub use crate::model::Direction;
use crate::model::{
    AgentInfo, PaneInfo, PluginInvocationContext, RuntimeSnapshot, TabInfo, WorkspaceInfo,
};

#[derive(Debug, Error)]
pub enum InvocationContextError {
    #[error("invalid HERDR_PLUGIN_CONTEXT_JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
}

pub fn parse_invocation_context(
    raw: &str,
) -> Result<PluginInvocationContext, InvocationContextError> {
    serde_json::from_str(raw).map_err(InvocationContextError::from)
}

impl RuntimeSnapshot {
    pub fn focused_workspace(&self) -> Option<&WorkspaceInfo> {
        let focused_id = self.session.focused_workspace_id.as_deref()?;
        self.session
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == focused_id)
    }

    pub fn focused_tab(&self) -> Option<&TabInfo> {
        let focused_id = self.session.focused_tab_id.as_deref()?;
        self.session
            .tabs
            .iter()
            .find(|tab| tab.tab_id == focused_id)
    }

    pub fn focused_pane(&self) -> Option<&PaneInfo> {
        let focused_id = self.session.focused_pane_id.as_deref()?;
        self.session
            .panes
            .iter()
            .find(|pane| pane.pane_id == focused_id)
    }

    pub fn focused_agent(&self) -> Option<&AgentInfo> {
        let pane_id = self.session.focused_pane_id.as_deref()?;
        self.session
            .agents
            .iter()
            .find(|agent| agent.pane_id == pane_id)
    }

    pub fn has_active_agent(&self, pane_id: &str) -> bool {
        self.session
            .agents
            .iter()
            .any(|agent| agent.pane_id == pane_id)
    }

    pub fn has_selection(&self) -> bool {
        self.invocation
            .selected_text
            .as_deref()
            .is_some_and(|selection| !selection.is_empty())
    }

    pub fn current_workspace_cwd(&self) -> Option<&str> {
        let invocation_matches_live_workspace = matches!(
            (
                self.invocation.workspace_id.as_deref(),
                self.session.focused_workspace_id.as_deref()
            ),
            (Some(invocation_id), Some(focused_id)) if invocation_id == focused_id
        );
        if invocation_matches_live_workspace {
            if let Some(cwd) = self.invocation.workspace_cwd.as_deref() {
                return Some(cwd);
            }
        }
        self.focused_pane().and_then(|pane| pane.cwd.as_deref())
    }

    pub fn has_neighbor(&self, direction: Direction) -> bool {
        let Some(focused_pane_id) = self.session.focused_pane_id.as_deref() else {
            return false;
        };
        let Some(focused_tab_id) = self.session.focused_tab_id.as_deref() else {
            return false;
        };
        let Some(layout) = self
            .session
            .layouts
            .iter()
            .find(|layout| layout.tab_id == focused_tab_id)
        else {
            return false;
        };
        let Some(pane) = layout
            .panes
            .iter()
            .find(|pane| pane.pane_id == focused_pane_id)
        else {
            return false;
        };

        let area_left = u32::from(layout.area.x);
        let area_top = u32::from(layout.area.y);
        let area_right = area_left + u32::from(layout.area.width);
        let area_bottom = area_top + u32::from(layout.area.height);
        let pane_left = u32::from(pane.rect.x);
        let pane_top = u32::from(pane.rect.y);
        let pane_right = pane_left + u32::from(pane.rect.width);
        let pane_bottom = pane_top + u32::from(pane.rect.height);

        match direction {
            Direction::Left => pane_left > area_left,
            Direction::Right => pane_right < area_right,
            Direction::Up => pane_top > area_top,
            Direction::Down => pane_bottom < area_bottom,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        ApiCapabilities, PluginInvocationContext, SessionSnapshotResult, SuccessEnvelope,
    };

    fn runtime_fixture() -> RuntimeSnapshot {
        let response: SuccessEnvelope<SessionSnapshotResult> =
            serde_json::from_str(include_str!("../tests/fixtures/session-snapshot.json")).unwrap();
        RuntimeSnapshot {
            session: response.result.snapshot,
            worktrees: Vec::new(),
            invocation: parse_invocation_context(include_str!(
                "../tests/fixtures/plugin-invocation-context.json"
            ))
            .unwrap(),
            capabilities: ApiCapabilities {
                typed_agent_start: false,
                agent_prompt: false,
            },
        }
    }

    #[test]
    fn invocation_context_rejects_malformed_json_and_defaults_optional_fields() {
        assert!(parse_invocation_context("not json").is_err());
        assert_eq!(
            parse_invocation_context("{}").unwrap(),
            PluginInvocationContext::default()
        );
    }

    #[test]
    fn focused_resources_resolve_from_live_snapshot_ids() {
        let runtime = runtime_fixture();
        assert_eq!(
            runtime.focused_workspace().map(|item| item.label.as_str()),
            Some("Palette")
        );
        assert_eq!(
            runtime.focused_tab().map(|item| item.label.as_str()),
            Some("Code")
        );
        assert_eq!(
            runtime.focused_pane().map(|item| item.pane_id.as_str()),
            Some("pane-a")
        );
        assert_eq!(
            runtime
                .focused_agent()
                .and_then(|item| item.name.as_deref()),
            Some("reviewer")
        );
        assert!(runtime.has_active_agent("pane-a"));
        assert!(!runtime.has_active_agent("pane-b"));
    }

    #[test]
    fn selection_must_be_non_empty() {
        let mut runtime = runtime_fixture();
        assert!(runtime.has_selection());
        runtime.invocation.selected_text = Some(String::new());
        assert!(!runtime.has_selection());
        runtime.invocation.selected_text = None;
        assert!(!runtime.has_selection());
    }

    #[test]
    fn invocation_cwd_is_trusted_only_for_the_live_focused_workspace() {
        let mut runtime = runtime_fixture();
        assert_eq!(runtime.current_workspace_cwd(), Some("/invoked/cwd"));

        runtime.invocation.workspace_id = Some("stale-workspace".into());
        assert_eq!(runtime.current_workspace_cwd(), Some("/repo/main"));

        runtime.invocation.workspace_id = None;
        runtime.session.focused_workspace_id = None;
        assert_eq!(runtime.current_workspace_cwd(), Some("/repo/main"));

        runtime.session.focused_pane_id = Some("pane-c".into());
        runtime
            .session
            .panes
            .iter_mut()
            .find(|pane| pane.pane_id == "pane-c")
            .unwrap()
            .cwd = None;
        assert_eq!(runtime.current_workspace_cwd(), None);
    }

    #[test]
    fn neighbor_detection_uses_all_four_layout_edges() {
        let mut runtime = runtime_fixture();
        assert!(runtime.has_neighbor(Direction::Right));
        assert!(runtime.has_neighbor(Direction::Down));
        assert!(!runtime.has_neighbor(Direction::Left));
        assert!(!runtime.has_neighbor(Direction::Up));

        runtime.session.focused_pane_id = Some("pane-d".into());
        assert!(runtime.has_neighbor(Direction::Left));
        assert!(runtime.has_neighbor(Direction::Up));
        assert!(!runtime.has_neighbor(Direction::Right));
        assert!(!runtime.has_neighbor(Direction::Down));
    }

    #[test]
    fn missing_focused_layout_has_no_neighbors() {
        let mut runtime = runtime_fixture();
        runtime.session.layouts.clear();
        for direction in [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
        ] {
            assert!(!runtime.has_neighbor(direction));
        }
    }
}
