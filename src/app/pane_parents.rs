//! Pane lineage: which pane a pane was created from, and which agent session
//! that pane ran at the time.
//!
//! The link is a server-owned fact keyed by pane. It survives the agent in the
//! parent pane exiting and follows panes across tab and workspace moves because
//! raw pane ids never change or get reused. The recorded agent session lets a
//! presentation tell a parent pane that still runs the creating agent from one
//! that now runs a different agent. When a parent closes, each child takes over
//! the closed parent's own link, so it is promoted to the nearest live ancestor
//! together with the agent session expected there.

use std::collections::HashMap;

use crate::agent_resume::PersistedAgentSession;
use crate::layout::PaneId;

use super::state::PaneParent;
use super::AppState;

impl AppState {
    pub(crate) fn pane_is_live(&self, pane_id: PaneId) -> bool {
        self.workspaces
            .iter()
            .any(|ws| ws.pane_state(pane_id).is_some())
    }

    /// Record `parent` as the parent of `child`, with the agent session the
    /// parent pane reported at that moment, when it reported one.
    ///
    /// Returns false, leaving state unchanged, when either pane is not live,
    /// when the panes are the same, or when the link would form a cycle.
    pub(crate) fn set_pane_parent(
        &mut self,
        child: PaneId,
        parent: PaneId,
        agent_session: Option<PersistedAgentSession>,
    ) -> bool {
        if child == parent || !self.pane_is_live(child) || !self.pane_is_live(parent) {
            return false;
        }
        let mut cursor = Some(parent);
        let mut hops = 0usize;
        while let Some(current) = cursor {
            if current == child {
                return false;
            }
            cursor = self.pane_parents.get(&current).map(|link| link.pane);
            hops += 1;
            if hops > self.pane_parents.len() {
                break;
            }
        }
        self.pane_parents.insert(
            child,
            PaneParent {
                pane: parent,
                agent_session,
            },
        );
        true
    }

    /// The nearest recorded ancestor of `pane_id` that is still a live pane.
    #[cfg(test)]
    pub(crate) fn effective_pane_parent(&self, pane_id: PaneId) -> Option<PaneId> {
        self.effective_pane_parent_link(pane_id)
            .map(|link| link.pane)
    }

    /// The link that reaches the nearest live ancestor of `pane_id`: that pane
    /// and the agent session expected to run in it.
    pub(crate) fn effective_pane_parent_link(&self, pane_id: PaneId) -> Option<&PaneParent> {
        self.nearest_live_link(self.pane_parents.get(&pane_id), &[])
    }

    /// Follow `start` up the recorded links to the first one whose pane is
    /// live and not in `excluded`.
    fn nearest_live_link<'a>(
        &'a self,
        start: Option<&'a PaneParent>,
        excluded: &[PaneId],
    ) -> Option<&'a PaneParent> {
        let mut cursor = start;
        let mut hops = 0usize;
        while let Some(link) = cursor {
            if !excluded.contains(&link.pane) && self.pane_is_live(link.pane) {
                return Some(link);
            }
            cursor = self.pane_parents.get(&link.pane);
            hops += 1;
            if hops > self.pane_parents.len() {
                return None;
            }
        }
        None
    }

    /// Forget closed panes and promote their children to the nearest live
    /// ancestor. A promoted child takes over the link that reaches that
    /// ancestor, so it expects the agent session its closed parent expected.
    /// Children whose parent changed are queued in
    /// `pending_pane_parent_updates` so the app layer can emit `pane.updated`.
    ///
    /// Safe to call before or after the closed panes leave their workspaces:
    /// the closed set is treated as dead either way.
    pub(crate) fn retire_pane_parents(&mut self, closed: impl IntoIterator<Item = PaneId>) {
        let closed = closed.into_iter().collect::<Vec<_>>();
        if closed.is_empty() || self.pane_parents.is_empty() {
            return;
        }
        let children = self
            .pane_parents
            .iter()
            .filter(|(child, parent)| closed.contains(&parent.pane) && !closed.contains(child))
            .map(|(child, _)| *child)
            .collect::<Vec<_>>();
        for child in children {
            let promoted = self
                .nearest_live_link(self.pane_parents.get(&child), &closed)
                .cloned();
            match promoted {
                Some(link) => {
                    self.pane_parents.insert(child, link);
                }
                None => {
                    self.pane_parents.remove(&child);
                }
            }
            if !self.pending_pane_parent_updates.contains(&child) {
                self.pending_pane_parent_updates.push(child);
            }
        }
        for pane_id in &closed {
            self.pane_parents.remove(pane_id);
        }
        self.pending_pane_parent_updates
            .retain(|pane_id| !closed.contains(pane_id));
    }

    /// Effective parent of every live pane that has one, as its public pane id
    /// and the agent session expected to run in it.
    pub(crate) fn public_pane_parents(
        &self,
    ) -> HashMap<PaneId, (String, Option<PersistedAgentSession>)> {
        let mut parents = HashMap::new();
        for ws in &self.workspaces {
            for tab in &ws.tabs {
                for pane_id in tab.panes.keys() {
                    let Some(link) = self.effective_pane_parent_link(*pane_id) else {
                        continue;
                    };
                    if let Some(public_id) = self.public_pane_id_anywhere(link.pane) {
                        parents.insert(*pane_id, (public_id, link.agent_session.clone()));
                    }
                }
            }
        }
        parents
    }

    /// Public id of a pane in whichever workspace currently holds it.
    pub(crate) fn public_pane_id_anywhere(&self, pane_id: PaneId) -> Option<String> {
        self.workspaces.iter().find_map(|ws| {
            let number = ws.public_pane_number(pane_id)?;
            Some(crate::workspace::public_pane_id_for_number(&ws.id, number))
        })
    }

    #[cfg(test)]
    pub(crate) fn assert_pane_parent_invariants_for_test(&self) {
        let live = self
            .workspaces
            .iter()
            .flat_map(|ws| ws.tabs.iter())
            .flat_map(|tab| tab.panes.keys().copied())
            .collect::<std::collections::HashSet<_>>();
        for (child, link) in &self.pane_parents {
            let parent = &link.pane;
            assert_ne!(child, parent, "pane {child:?} must not be its own parent");
            assert!(
                live.contains(child),
                "pane parent key {child:?} must be live"
            );
            assert!(
                live.contains(parent),
                "pane parent value {parent:?} for {child:?} must be live"
            );
            let mut cursor = Some(*parent);
            let mut hops = 0usize;
            while let Some(current) = cursor {
                assert_ne!(current, *child, "pane parent cycle through {child:?}");
                cursor = self.pane_parents.get(&current).map(|link| link.pane);
                hops += 1;
                assert!(
                    hops <= self.pane_parents.len(),
                    "pane parent chain from {child:?} does not terminate"
                );
            }
        }
        for pane_id in &self.pending_pane_parent_updates {
            assert!(
                live.contains(pane_id),
                "pending pane parent update {pane_id:?} must be live"
            );
        }
    }
}

/// Lineage links rebuilt by `crate::persist::restored_pane_parents`.
pub(crate) fn restored_links(
    restored: HashMap<PaneId, (PaneId, Option<PersistedAgentSession>)>,
) -> HashMap<PaneId, PaneParent> {
    restored
        .into_iter()
        .map(|(child, (pane, agent_session))| {
            (
                child,
                PaneParent {
                    pane,
                    agent_session,
                },
            )
        })
        .collect()
}

impl super::App {
    /// Resolve a request's `caller_pane_id`: a public pane id (aliases included)
    /// or a unique live agent name.
    pub(crate) fn resolve_caller_pane(&self, caller: &str) -> Option<PaneId> {
        if let Some((_, pane_id)) = self.parse_pane_id(caller) {
            return Some(pane_id);
        }
        self.resolve_agent_target(caller)
            .ok()
            .map(|target| target.pane_id)
    }

    /// Parent for a pane about to be created from a request's `caller_pane_id`.
    ///
    /// The field is advisory: a caller pane the server does not know (for
    /// example `HERDR_PANE_ID` from another session) records no parent instead
    /// of failing the request. The CLI validates an explicit `--parent` itself.
    pub(crate) fn parent_pane_from_caller(&self, caller: Option<&str>) -> Option<PaneId> {
        let caller = caller?;
        let parent = self.resolve_caller_pane(caller);
        if parent.is_none() {
            tracing::debug!(caller, "ignoring unknown caller pane for lineage");
        }
        parent
    }

    /// Record `parent` for a pane that was just created, with the agent
    /// session the parent pane's agent reports right now. A new pane has no
    /// children, so this cannot form a cycle; a failure is a server bug.
    pub(crate) fn adopt_new_pane(&mut self, child: PaneId, parent: Option<PaneId>) {
        let Some(parent) = parent else {
            return;
        };
        let agent_session = self.origin_agent_session(parent);
        if !self.state.set_pane_parent(child, parent, agent_session) {
            tracing::warn!(
                child = child.raw(),
                parent = parent.raw(),
                "could not record pane parent"
            );
        }
    }

    /// The agent session reported by the agent in `pane`. A pane that hosts
    /// no agent has none, so links from it nest under any later agent there.
    fn origin_agent_session(&self, pane: PaneId) -> Option<PersistedAgentSession> {
        let (_, pane_state) = self.find_pane(pane)?;
        let terminal = self.state.terminals.get(&pane_state.attached_terminal_id)?;
        if !terminal.is_agent_terminal() {
            return None;
        }
        terminal.reported_agent_session()
    }

    /// Emit `pane.updated` for panes whose parent changed through promotion.
    pub(crate) fn emit_pending_pane_parent_updates(&mut self) {
        let pending = std::mem::take(&mut self.state.pending_pane_parent_updates);
        for pane_id in pending {
            if let Some((ws_idx, _)) = self.find_pane(pane_id) {
                self.emit_pane_updated(ws_idx, pane_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::PaneId;

    fn state_with_panes(count: usize) -> (AppState, Vec<PaneId>) {
        let mut state = AppState::test_new();
        state.workspaces = vec![crate::workspace::Workspace::test_new("lineage")];
        state.active = Some(0);
        state.selected = 0;
        let mut panes = Vec::new();
        let ws = &mut state.workspaces[0];
        panes.push(ws.focused_pane_id().expect("test workspace has a pane"));
        for _ in 1..count {
            panes.push(ws.test_split(ratatui::layout::Direction::Horizontal));
        }
        state.ensure_test_terminals();
        (state, panes)
    }

    #[test]
    fn set_parent_records_live_link_and_rejects_cycles() {
        let (mut state, panes) = state_with_panes(3);
        assert!(state.set_pane_parent(panes[1], panes[0], None));
        assert!(state.set_pane_parent(panes[2], panes[1], None));
        assert_eq!(state.effective_pane_parent(panes[2]), Some(panes[1]));
        assert_eq!(state.effective_pane_parent(panes[1]), Some(panes[0]));
        assert_eq!(state.effective_pane_parent(panes[0]), None);

        assert!(
            !state.set_pane_parent(panes[0], panes[2], None),
            "cycle must be rejected"
        );
        assert!(
            !state.set_pane_parent(panes[0], panes[0], None),
            "self link must be rejected"
        );
        assert!(
            !state.set_pane_parent(panes[1], PaneId::from_raw(9_999), None),
            "unknown parent must be rejected"
        );
        assert_eq!(state.effective_pane_parent(panes[0]), None);
        state.assert_pane_parent_invariants_for_test();
    }

    #[test]
    fn closing_a_parent_promotes_children_to_the_grandparent() {
        let (mut state, panes) = state_with_panes(3);
        assert!(state.set_pane_parent(panes[1], panes[0], None));
        assert!(state.set_pane_parent(panes[2], panes[1], None));

        state.workspaces[0].close_pane(panes[1]);
        state.retire_pane_parents([panes[1]]);

        assert_eq!(state.effective_pane_parent(panes[2]), Some(panes[0]));
        assert!(!state.pane_parents.contains_key(&panes[1]));
        assert_eq!(state.pending_pane_parent_updates, vec![panes[2]]);
        state.assert_pane_parent_invariants_for_test();
    }

    #[test]
    fn closing_the_whole_chain_leaves_children_as_roots() {
        let (mut state, panes) = state_with_panes(3);
        assert!(state.set_pane_parent(panes[1], panes[0], None));
        assert!(state.set_pane_parent(panes[2], panes[1], None));

        // The closed set is honoured before the panes leave the workspace.
        state.retire_pane_parents([panes[0], panes[1]]);
        assert_eq!(state.effective_pane_parent(panes[2]), None);
        assert!(state.pane_parents.is_empty());
        assert_eq!(state.pending_pane_parent_updates, vec![panes[2]]);

        state.workspaces[0].close_pane(panes[0]);
        state.workspaces[0].close_pane(panes[1]);
        state.assert_pane_parent_invariants_for_test();
    }

    #[test]
    fn effective_parent_skips_dead_entries_without_retirement() {
        let (mut state, panes) = state_with_panes(3);
        assert!(state.set_pane_parent(panes[1], panes[0], None));
        assert!(state.set_pane_parent(panes[2], panes[1], None));

        state.workspaces[0].close_pane(panes[1]);
        assert_eq!(
            state.effective_pane_parent(panes[2]),
            Some(panes[0]),
            "a dead intermediate entry is walked through"
        );
    }

    #[test]
    fn public_parent_ids_use_the_holding_workspace() {
        let (mut state, panes) = state_with_panes(2);
        assert!(state.set_pane_parent(panes[1], panes[0], None));
        let expected = state
            .public_pane_id_anywhere(panes[0])
            .expect("root pane has a public id");
        assert!(expected.starts_with(&format!("{}:p", state.workspaces[0].id)));
        let parents = state.public_pane_parents();
        assert_eq!(
            parents.get(&panes[1]).map(|(public_id, _)| public_id),
            Some(&expected)
        );
        assert!(!parents.contains_key(&panes[0]));
    }

    fn claude_session(value: &str) -> PersistedAgentSession {
        PersistedAgentSession {
            source: "herdr:claude".into(),
            agent: "claude".into(),
            session_ref: crate::agent_resume::AgentSessionRef {
                kind: crate::agent_resume::AgentSessionRefKind::Id,
                value: value.into(),
            },
        }
    }

    #[test]
    fn promotion_hands_the_closed_parent_link_to_its_children() {
        let (mut state, panes) = state_with_panes(3);
        assert!(state.set_pane_parent(panes[1], panes[0], Some(claude_session("root"))));
        assert!(state.set_pane_parent(panes[2], panes[1], Some(claude_session("middle"))));

        state.retire_pane_parents([panes[1]]);
        assert_eq!(
            state.pane_parents.get(&panes[2]),
            Some(&PaneParent {
                pane: panes[0],
                agent_session: Some(claude_session("root")),
            }),
            "the child expects the agent its closed parent expected"
        );
        assert_eq!(
            state
                .public_pane_parents()
                .get(&panes[2])
                .map(|(_, session)| session.clone()),
            Some(Some(claude_session("root")))
        );
    }
}
