//! Pane lineage: which pane a pane was created from.
//!
//! The link is a server-owned, pane-scoped fact. It survives the agent in the
//! parent pane exiting and follows panes across tab and workspace moves because
//! raw pane ids never change or get reused. When a parent closes, its children
//! are promoted to the nearest live ancestor.

use std::collections::HashMap;

use crate::layout::PaneId;

use super::AppState;

impl AppState {
    pub(crate) fn pane_is_live(&self, pane_id: PaneId) -> bool {
        self.workspaces
            .iter()
            .any(|ws| ws.pane_state(pane_id).is_some())
    }

    /// Record `parent` as the parent of `child`.
    ///
    /// Returns false, leaving state unchanged, when either pane is not live,
    /// when the panes are the same, or when the link would form a cycle.
    pub(crate) fn set_pane_parent(&mut self, child: PaneId, parent: PaneId) -> bool {
        if child == parent || !self.pane_is_live(child) || !self.pane_is_live(parent) {
            return false;
        }
        let mut cursor = Some(parent);
        let mut hops = 0usize;
        while let Some(current) = cursor {
            if current == child {
                return false;
            }
            cursor = self.pane_parents.get(&current).copied();
            hops += 1;
            if hops > self.pane_parents.len() {
                break;
            }
        }
        self.pane_parents.insert(child, parent);
        true
    }

    /// The nearest recorded ancestor of `pane_id` that is still a live pane.
    pub(crate) fn effective_pane_parent(&self, pane_id: PaneId) -> Option<PaneId> {
        self.nearest_live_ancestor(self.pane_parents.get(&pane_id).copied(), &[])
    }

    fn nearest_live_ancestor(&self, start: Option<PaneId>, excluded: &[PaneId]) -> Option<PaneId> {
        let mut cursor = start;
        let mut hops = 0usize;
        while let Some(current) = cursor {
            if !excluded.contains(&current) && self.pane_is_live(current) {
                return Some(current);
            }
            cursor = self.pane_parents.get(&current).copied();
            hops += 1;
            if hops > self.pane_parents.len() {
                return None;
            }
        }
        None
    }

    /// Forget closed panes and promote their children to the nearest live
    /// ancestor. Children whose parent changed are queued in
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
            .filter(|(child, parent)| closed.contains(parent) && !closed.contains(child))
            .map(|(child, _)| *child)
            .collect::<Vec<_>>();
        for child in children {
            let start = self.pane_parents.get(&child).copied();
            match self.nearest_live_ancestor(start, &closed) {
                Some(parent) => {
                    self.pane_parents.insert(child, parent);
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

    /// Effective parent of every live pane that has one, as public pane ids.
    pub(crate) fn pane_parent_public_ids(&self) -> HashMap<PaneId, String> {
        let mut public_ids = HashMap::new();
        for ws in &self.workspaces {
            for tab in &ws.tabs {
                for pane_id in tab.panes.keys() {
                    let Some(parent) = self.effective_pane_parent(*pane_id) else {
                        continue;
                    };
                    if let Some(public_id) = self.public_pane_id_anywhere(parent) {
                        public_ids.insert(*pane_id, public_id);
                    }
                }
            }
        }
        public_ids
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
        for (child, parent) in &self.pane_parents {
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
                cursor = self.pane_parents.get(&current).copied();
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

    /// Record `parent` for a pane that was just created. A new pane has no
    /// children, so this cannot form a cycle; a failure is a server bug.
    pub(crate) fn adopt_new_pane(&mut self, child: PaneId, parent: Option<PaneId>) {
        let Some(parent) = parent else {
            return;
        };
        if !self.state.set_pane_parent(child, parent) {
            tracing::warn!(
                child = child.raw(),
                parent = parent.raw(),
                "could not record pane parent"
            );
        }
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
        assert!(state.set_pane_parent(panes[1], panes[0]));
        assert!(state.set_pane_parent(panes[2], panes[1]));
        assert_eq!(state.effective_pane_parent(panes[2]), Some(panes[1]));
        assert_eq!(state.effective_pane_parent(panes[1]), Some(panes[0]));
        assert_eq!(state.effective_pane_parent(panes[0]), None);

        assert!(
            !state.set_pane_parent(panes[0], panes[2]),
            "cycle must be rejected"
        );
        assert!(
            !state.set_pane_parent(panes[0], panes[0]),
            "self link must be rejected"
        );
        assert!(
            !state.set_pane_parent(panes[1], PaneId::from_raw(9_999)),
            "unknown parent must be rejected"
        );
        assert_eq!(state.effective_pane_parent(panes[0]), None);
        state.assert_pane_parent_invariants_for_test();
    }

    #[test]
    fn closing_a_parent_promotes_children_to_the_grandparent() {
        let (mut state, panes) = state_with_panes(3);
        assert!(state.set_pane_parent(panes[1], panes[0]));
        assert!(state.set_pane_parent(panes[2], panes[1]));

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
        assert!(state.set_pane_parent(panes[1], panes[0]));
        assert!(state.set_pane_parent(panes[2], panes[1]));

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
        assert!(state.set_pane_parent(panes[1], panes[0]));
        assert!(state.set_pane_parent(panes[2], panes[1]));

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
        assert!(state.set_pane_parent(panes[1], panes[0]));
        let expected = state
            .public_pane_id_anywhere(panes[0])
            .expect("root pane has a public id");
        assert!(expected.starts_with(&format!("{}:p", state.workspaces[0].id)));
        let public_ids = state.pane_parent_public_ids();
        assert_eq!(public_ids.get(&panes[1]), Some(&expected));
        assert!(!public_ids.contains_key(&panes[0]));
    }
}
