use std::collections::{HashMap, HashSet};

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::Line,
    widgets::{Paragraph, Widget},
};

use super::*;
use crate::protocol::ClientShellAgent;

pub(super) struct AgentRow {
    pub(super) pane_id: String,
    pub(super) status: crate::api::schema::AgentStatus,
    pub(super) focused: bool,
    pub(super) rows: Vec<Vec<crate::ui::ResolvedToken>>,
    /// Nesting depth in the agent tree; zero outside the `tree` sort.
    pub(super) depth: u8,
    /// Whether this entry is the last child of its parent (picks `└─` over `├─`).
    pub(super) last_sibling: bool,
    /// Whether other listed agents nest under this entry.
    pub(super) has_children: bool,
    /// Whether this entry's subtree is folded away.
    pub(super) collapsed: bool,
    /// Agents hidden under this entry while it is collapsed.
    pub(super) hidden_descendants: usize,
    /// Most urgent status among the hidden descendants.
    pub(super) hidden_status: Option<crate::api::schema::AgentStatus>,
}

impl AgentRow {
    /// Take the tree placement computed for this row's pane.
    pub(super) fn place(mut self, entry: &OrderedAgent) -> Self {
        self.depth = entry.depth;
        self.last_sibling = entry.last_sibling;
        self.has_children = entry.has_children;
        self.collapsed = entry.collapsed;
        self.hidden_descendants = entry.hidden_descendants;
        self.hidden_status = entry.hidden_status;
        self
    }

    /// Drop the workspace and tab labels a child shares with the agent it
    /// nests under: the parent's entry already says where the child runs. A
    /// layout line left without values disappears, as for any missing token.
    pub(super) fn hide_shared_labels(
        mut self,
        snapshot: &ClientShellSnapshot,
        entry: &OrderedAgent,
    ) -> Self {
        if entry.depth == 0 {
            return self;
        }
        let find = |pane_id: &str| {
            snapshot
                .agents
                .iter()
                .find(|agent| agent.pane_id == pane_id)
        };
        let Some(child) = find(&entry.pane_id) else {
            return self;
        };
        let Some(parent) = child.parent_pane_id.as_deref().and_then(find) else {
            return self;
        };
        if parent.workspace_id != child.workspace_id {
            return self;
        }
        let same_tab = parent.tab_id == child.tab_id;
        for tokens in &mut self.rows {
            tokens.retain(|token| match token.kind {
                crate::ui::ResolvedTokenKind::Workspace(_) => false,
                crate::ui::ResolvedTokenKind::Tab(_) => !same_tab,
                _ => true,
            });
        }
        self.rows.retain(|tokens| !tokens.is_empty());
        self
    }
}

/// Collapsed agent subtrees per endpoint, as renderers and navigation see them.
#[derive(Clone, Copy)]
pub(super) struct AgentCollapse<'a> {
    pub(super) local: &'a HashSet<String>,
    pub(super) remote: &'a HashMap<ClientEndpointId, HashSet<String>>,
}

impl<'a> AgentCollapse<'a> {
    pub(super) fn for_endpoint(&self, endpoint_id: &ClientEndpointId) -> &'a HashSet<String> {
        if endpoint_id.is_local() {
            self.local
        } else {
            match self.remote.get(endpoint_id) {
                Some(subtrees) => subtrees,
                None => empty_collapsed_set(),
            }
        }
    }
}

/// Shared empty set for endpoints without collapsed subtrees.
pub(super) fn empty_collapsed_set() -> &'static HashSet<String> {
    static EMPTY: std::sync::OnceLock<HashSet<String>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(HashSet::new)
}

/// Deeper entries share this indentation so narrow sidebars stay readable.
pub(super) const MAX_TREE_RENDER_DEPTH: u8 = 4;

/// One agent in display order, with its place in the agent tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OrderedAgent {
    pub(super) pane_id: String,
    pub(super) depth: u8,
    pub(super) last_sibling: bool,
    pub(super) has_children: bool,
    pub(super) collapsed: bool,
    pub(super) hidden_descendants: usize,
    pub(super) hidden_status: Option<crate::api::schema::AgentStatus>,
}

impl OrderedAgent {
    /// An entry outside any tree: a root with nothing folded under it.
    pub(super) fn flat(pane_id: &str) -> Self {
        Self {
            pane_id: pane_id.to_owned(),
            depth: 0,
            last_sibling: false,
            has_children: false,
            collapsed: false,
            hidden_descendants: 0,
            hidden_status: None,
        }
    }
}

pub(super) fn ordered_agent_pane_ids(
    snapshot: &ClientShellSnapshot,
    sort: crate::config::AgentPanelSortConfig,
    collapsed: &HashSet<String>,
) -> Vec<String> {
    ordered_agent_entries(snapshot, sort, collapsed)
        .into_iter()
        .map(|entry| entry.pane_id)
        .collect()
}

/// Agents in display order. `collapsed` holds pane ids whose subtree is folded
/// in the `tree` sort; the other sorts stay flat and ignore it.
pub(super) fn ordered_agent_entries(
    snapshot: &ClientShellSnapshot,
    sort: crate::config::AgentPanelSortConfig,
    collapsed: &HashSet<String>,
) -> Vec<OrderedAgent> {
    if snapshot.agent_view_label.is_some() {
        return snapshot
            .agent_order
            .iter()
            .filter(|pane_id| {
                snapshot
                    .agents
                    .iter()
                    .any(|agent| agent.pane_id == pane_id.as_str())
            })
            .map(|pane_id| OrderedAgent::flat(pane_id))
            .collect();
    }
    match sort {
        crate::config::AgentPanelSortConfig::Tree => {
            agent_tree_entries(&snapshot.agents, collapsed)
        }
        crate::config::AgentPanelSortConfig::Priority => {
            let mut agents = snapshot.agents.iter().collect::<Vec<_>>();
            agents.sort_by_key(|agent| {
                (
                    std::cmp::Reverse(status_priority(agent.agent_status)),
                    std::cmp::Reverse(agent.state_change_seq),
                )
            });
            agents
                .into_iter()
                .map(|agent| OrderedAgent::flat(&agent.pane_id))
                .collect()
        }
        crate::config::AgentPanelSortConfig::Spaces => snapshot
            .agents
            .iter()
            .map(|agent| OrderedAgent::flat(&agent.pane_id))
            .collect(),
    }
}

/// Nest agents under their parent pane, keeping server order among roots and
/// among siblings. A parent that is not itself a listed agent makes its
/// children roots; the server forbids cycles, but a stale snapshot must not
/// hang the client, so every agent is emitted exactly once.
pub(super) fn agent_tree_entries(
    agents: &[ClientShellAgent],
    collapsed: &HashSet<String>,
) -> Vec<OrderedAgent> {
    let listed = agents
        .iter()
        .map(|agent| agent.pane_id.as_str())
        .collect::<HashSet<_>>();
    let mut children: HashMap<&str, Vec<&ClientShellAgent>> = HashMap::new();
    let mut roots = Vec::new();
    for agent in agents {
        match agent
            .parent_pane_id
            .as_deref()
            .filter(|parent| *parent != agent.pane_id && listed.contains(parent))
        {
            Some(parent) => children.entry(parent).or_default().push(agent),
            None => roots.push(agent),
        }
    }

    /// Mark a folded subtree visited and report how many agents it hides and
    /// the most urgent status among them.
    fn hide_subtree<'a>(
        agent: &'a ClientShellAgent,
        children: &HashMap<&str, Vec<&'a ClientShellAgent>>,
        visited: &mut HashSet<&'a str>,
    ) -> (usize, Option<crate::api::schema::AgentStatus>) {
        let mut hidden = 0;
        let mut status = None;
        for kid in children
            .get(agent.pane_id.as_str())
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            if !visited.insert(kid.pane_id.as_str()) {
                continue;
            }
            hidden += 1;
            status = most_urgent(status, kid.agent_status);
            let (nested_hidden, nested_status) = hide_subtree(kid, children, visited);
            hidden += nested_hidden;
            if let Some(nested_status) = nested_status {
                status = most_urgent(status, nested_status);
            }
        }
        (hidden, status)
    }

    fn visit<'a>(
        agent: &'a ClientShellAgent,
        depth: u8,
        last_sibling: bool,
        children: &HashMap<&str, Vec<&'a ClientShellAgent>>,
        collapsed: &HashSet<String>,
        visited: &mut HashSet<&'a str>,
        out: &mut Vec<OrderedAgent>,
    ) {
        if !visited.insert(agent.pane_id.as_str()) {
            return;
        }
        let kids = children.get(agent.pane_id.as_str());
        let has_children = kids.is_some_and(|kids| !kids.is_empty());
        let fold = has_children && collapsed.contains(&agent.pane_id);
        let index = out.len();
        out.push(OrderedAgent {
            pane_id: agent.pane_id.clone(),
            depth,
            last_sibling,
            has_children,
            collapsed: fold,
            hidden_descendants: 0,
            hidden_status: None,
        });
        if fold {
            let (hidden, status) = hide_subtree(agent, children, visited);
            out[index].hidden_descendants = hidden;
            out[index].hidden_status = status;
            return;
        }
        if let Some(kids) = kids {
            let count = kids.len();
            for (position, kid) in kids.iter().enumerate() {
                visit(
                    kid,
                    depth.saturating_add(1),
                    position + 1 == count,
                    children,
                    collapsed,
                    visited,
                    out,
                );
            }
        }
    }

    let mut visited = HashSet::new();
    let mut out = Vec::with_capacity(agents.len());
    for root in &roots {
        visit(root, 0, false, &children, collapsed, &mut visited, &mut out);
    }
    for agent in agents {
        if !visited.contains(agent.pane_id.as_str()) {
            // Only reachable through a parent cycle; show it as a root.
            visit(
                agent,
                0,
                false,
                &children,
                collapsed,
                &mut visited,
                &mut out,
            );
        }
    }
    out
}

fn most_urgent(
    current: Option<crate::api::schema::AgentStatus>,
    candidate: crate::api::schema::AgentStatus,
) -> Option<crate::api::schema::AgentStatus> {
    match current {
        Some(current) if status_priority(current) >= status_priority(candidate) => Some(current),
        _ => Some(candidate),
    }
}

pub(super) fn render_agent_panel(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    agent_scroll: &mut usize,
    collapsed: &HashSet<String>,
    hits: &mut ShellHitMap,
) {
    if !render_agent_panel_header(
        buffer,
        area,
        snapshot.agent_view_label.as_deref(),
        config,
        hits,
    ) {
        return;
    }

    let rows = agent_rows(snapshot, config, None, collapsed);
    render_agent_list(
        buffer,
        area,
        &rows,
        snapshot
            .agent_view_label
            .as_ref()
            .map(|_| " no matching agents"),
        config,
        agent_scroll,
        hits,
        |row| row.rows.len(),
        |buffer, rect, row, hits| {
            hits.agents.push((rect, row.pane_id.clone()));
            if let Some(toggle) = render_agent_row(buffer, rect, row, config) {
                hits.agent_subtree_toggles
                    .push((toggle, row.pane_id.clone()));
            }
        },
    );
}

pub(super) fn render_agent_panel_header(
    buffer: &mut Buffer,
    area: Rect,
    agent_view_label: Option<&str>,
    config: &ClientShellConfig,
    hits: &mut ShellHitMap,
) -> bool {
    if area.height == 0 {
        return false;
    }
    put_text(
        buffer,
        area.x,
        area.y,
        area.width,
        &"─".repeat(area.width as usize),
        Style::default().fg(config.palette.surface_dim),
    );
    if area.height < 2 {
        return false;
    }
    put_text(
        buffer,
        area.x,
        area.y + 1,
        area.width,
        " agents",
        Style::default()
            .fg(config.palette.overlay0)
            .add_modifier(Modifier::BOLD),
    );
    let sort_label = agent_view_label.unwrap_or(match config.agent_panel_sort {
        crate::config::AgentPanelSortConfig::Spaces => "grouped",
        crate::config::AgentPanelSortConfig::Priority => "priority",
        crate::config::AgentPanelSortConfig::Tree => "tree",
    });
    let sort_width = display_width(sort_label).min(area.width as usize) as u16;
    let sort_rect = Rect::new(
        area.right().saturating_sub(sort_width),
        area.y + 1,
        sort_width,
        1,
    );
    hits.agent_sort_toggle = if config.mouse_capture && agent_view_label.is_none() {
        sort_rect
    } else {
        Rect::default()
    };
    put_text(
        buffer,
        sort_rect.x,
        sort_rect.y,
        sort_rect.width,
        sort_label,
        Style::default()
            .fg(if agent_view_label.is_some() {
                config.palette.accent
            } else {
                config.palette.overlay0
            })
            .add_modifier(Modifier::BOLD),
    );
    true
}

pub(super) fn render_agent_list<T>(
    buffer: &mut Buffer,
    area: Rect,
    rows: &[T],
    empty_message: Option<&str>,
    config: &ClientShellConfig,
    agent_scroll: &mut usize,
    hits: &mut ShellHitMap,
    row_lines: impl Fn(&T) -> usize,
    mut render_row: impl FnMut(&mut Buffer, Rect, &T, &mut ShellHitMap),
) {
    let body = Rect::new(
        area.x,
        area.y.saturating_add(3),
        area.width,
        area.height.saturating_sub(3),
    );
    hits.agent_body = body;
    if body.is_empty() || rows.is_empty() {
        *agent_scroll = 0;
        if let Some(message) = empty_message.filter(|_| !body.is_empty()) {
            put_text(
                buffer,
                body.x,
                body.y,
                body.width,
                message,
                Style::default()
                    .fg(config.palette.overlay0)
                    .add_modifier(Modifier::DIM),
            );
        }
        return;
    }

    let row_heights = rows
        .iter()
        .map(|row| row_lines(row).max(1).min(u16::MAX as usize) as u16)
        .collect::<Vec<_>>();
    let gaps = rows
        .iter()
        .enumerate()
        .map(|(index, _)| {
            if index + 1 < rows.len() {
                config.agents.row_gap
            } else {
                0
            }
        })
        .collect::<Vec<_>>();
    let metrics =
        super::scroll::list_scroll_metrics(&row_heights, &gaps, body.height, *agent_scroll);
    hits.agent_max_scroll = metrics.max_offset_from_bottom;
    hits.agent_scroll_metrics = Some(metrics);
    *agent_scroll = metrics
        .max_offset_from_bottom
        .saturating_sub(metrics.offset_from_bottom);
    let show_scrollbar = metrics.max_offset_from_bottom > 0 && body.width > 1;
    let content_width = body.width.saturating_sub(u16::from(show_scrollbar));
    let mut y = body.y;
    for (index, row) in rows.iter().enumerate().skip(*agent_scroll) {
        let height = row_heights[index].min(body.height);
        if y.saturating_add(height) > body.bottom() {
            break;
        }
        let rect = Rect::new(body.x, y, content_width, height);
        render_row(buffer, rect, row, hits);
        y = y
            .saturating_add(height)
            .saturating_add(if index + 1 < rows.len() {
                config.agents.row_gap
            } else {
                0
            });
    }

    if show_scrollbar {
        let track = Rect::new(body.right().saturating_sub(1), body.y, 1, body.height);
        hits.agent_scrollbar = track;
        super::scroll::render_list_scrollbar(buffer, track, metrics, &config.palette);
    }
}

pub(super) fn agent_rows(
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    machine: Option<&str>,
    collapsed: &HashSet<String>,
) -> Vec<AgentRow> {
    ordered_agent_entries(snapshot, config.agent_panel_sort, collapsed)
        .into_iter()
        .filter_map(|entry| {
            let row = agent_row(snapshot, &entry.pane_id, config, machine)?;
            Some(row.place(&entry).hide_shared_labels(snapshot, &entry))
        })
        .collect()
}

pub(super) fn agent_row(
    snapshot: &ClientShellSnapshot,
    pane_id: &str,
    config: &ClientShellConfig,
    machine: Option<&str>,
) -> Option<AgentRow> {
    let agent = snapshot
        .agents
        .iter()
        .find(|agent| agent.pane_id == pane_id)?;
    let workspace = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == agent.workspace_id)?;
    let tab = snapshot.tabs.iter().find(|tab| tab.tab_id == agent.tab_id);
    let pane = snapshot
        .panes
        .iter()
        .find(|pane| pane.pane_id == agent.pane_id);
    let tab_count = snapshot
        .tabs
        .iter()
        .filter(|candidate| candidate.workspace_id == agent.workspace_id)
        .count();
    let tab_label = tab
        .filter(|tab| tab_count > 1 || tab.custom_label)
        .map(|tab| tab.label.as_str());
    let agent_label = agent
        .display_agent
        .as_deref()
        .or(agent.name.as_deref())
        .or(agent.agent.as_deref())
        .or(agent.title.as_deref());
    let labels = agent
        .state_labels
        .iter()
        .cloned()
        .collect::<HashMap<_, _>>();
    let tokens = agent.tokens.iter().cloned().collect::<HashMap<_, _>>();
    let state_text = labels
        .get(status_text(agent.agent_status))
        .map(String::as_str)
        .unwrap_or_else(|| sidebar_status_text(agent.agent_status));
    let canonical_agent = agent
        .agent
        .as_deref()
        .and_then(crate::detect::parse_agent_label);
    let rows = crate::ui::sidebar_agent_rows(
        &config.agents,
        crate::ui::AgentTokenContext {
            machine,
            workspace: &workspace.label,
            tab: tab_label,
            pane: agent
                .title
                .as_deref()
                .or_else(|| pane.and_then(|pane| pane.label.as_deref())),
            agent_label,
            terminal_title: agent.terminal_title.as_deref(),
            terminal_title_stripped: agent.terminal_title_stripped.as_deref(),
            canonical_agent,
            tokens: &tokens,
        },
        state_text,
    );
    Some(AgentRow {
        pane_id: agent.pane_id.clone(),
        status: agent.agent_status,
        focused: agent.focused,
        rows,
        depth: 0,
        last_sibling: false,
        has_children: false,
        collapsed: false,
        hidden_descendants: 0,
        hidden_status: None,
    })
}

pub(super) fn render_agent_row(
    buffer: &mut Buffer,
    rect: Rect,
    row: &AgentRow,
    config: &ClientShellConfig,
) -> Option<Rect> {
    let palette = &config.palette;
    let row_style = if row.focused {
        Style::default().bg(palette.active_row_bg)
    } else {
        Style::default()
    };
    // A folded parent speaks for its hidden subtree: the most urgent hidden
    // state colors the icon and the fold marker.
    let shown_status = match row.hidden_status {
        Some(hidden) if row.collapsed && status_priority(hidden) > status_priority(row.status) => {
            hidden
        }
        _ => row.status,
    };
    let name_style = if row.focused {
        Style::default()
            .fg(palette.text)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(palette.subtext0)
            .add_modifier(Modifier::BOLD)
    };
    let status_style = Style::default().fg(status_color(shown_status, palette));
    let secondary = Style::default().fg(palette.overlay0);
    let icon = (
        status_icon(shown_status, config.status_indicators),
        Style::default().fg(status_color(shown_status, palette)),
    );
    let marker = row.has_children.then(|| {
        if row.collapsed {
            format!("▸{}", row.hidden_descendants)
        } else {
            "▾".to_owned()
        }
    });
    let marker_width = marker.as_deref().map(display_width).unwrap_or(0) as u16;
    let rows = if row.rows.is_empty() {
        vec![vec![crate::ui::ResolvedToken {
            kind: crate::ui::ResolvedTokenKind::StateIcon,
            style: Default::default(),
        }]]
    } else {
        row.rows.clone()
    };
    let depth = usize::from(row.depth.min(MAX_TREE_RENDER_DEPTH));
    for (index, tokens) in rows.iter().take(rect.height as usize).enumerate() {
        let prefix = tree_row_prefix(depth, index == 0, row.last_sibling);
        let indent = display_width(&prefix);
        // The first line keeps room for the fold marker and one gap cell.
        let reserved = if index == 0 && marker_width > 0 {
            marker_width.saturating_add(1)
        } else {
            0
        };
        let mut spans = vec![ratatui::text::Span::styled(
            prefix,
            Style::default().fg(palette.overlay0),
        )];
        spans.extend(crate::ui::resolved_token_spans(
            tokens,
            icon,
            status_style,
            name_style,
            secondary,
            secondary,
            palette,
            rect.width
                .saturating_sub(indent as u16)
                .saturating_sub(reserved) as usize,
        ));
        Paragraph::new(Line::from(spans)).style(row_style).render(
            Rect::new(rect.x, rect.y + index as u16, rect.width, 1),
            buffer,
        );
    }
    let marker = marker.filter(|_| rect.height > 0 && rect.width > marker_width)?;
    let marker_rect = Rect::new(
        rect.right().saturating_sub(marker_width),
        rect.y,
        marker_width,
        1,
    );
    let marker_style = if row.collapsed {
        Style::default().fg(status_color(shown_status, palette))
    } else {
        Style::default().fg(palette.accent)
    };
    put_text(
        buffer,
        marker_rect.x,
        marker_rect.y,
        marker_rect.width,
        &marker,
        row_style.patch(marker_style),
    );
    Some(marker_rect)
}

/// Leading cells for one line of an agent entry: roots keep the flat layout
/// (1 column, then 3 for continuation lines); each tree level adds 2 columns,
/// and the first line of a child carries the same `├─`/`└─` guide the Spaces
/// panel uses for worktrees.
pub(super) fn tree_row_prefix(depth: usize, first_line: bool, last_sibling: bool) -> String {
    match (depth, first_line) {
        (0, true) => " ".to_owned(),
        (0, false) => "   ".to_owned(),
        (depth, true) => format!(
            "{}{} ",
            " ".repeat(2 * depth - 1),
            if last_sibling { "└─" } else { "├─" }
        ),
        (depth, false) => " ".repeat(3 + 2 * depth),
    }
}

fn put_text(buffer: &mut Buffer, x: u16, y: u16, width: u16, text: &str, style: Style) {
    for (offset, character) in text.chars().take(width as usize).enumerate() {
        if let Some(cell) = buffer.cell_mut((x + offset as u16, y)) {
            cell.set_char(character).set_style(style);
        }
    }
}

fn display_width(text: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(text)
}

fn sidebar_status_text(status: crate::api::schema::AgentStatus) -> &'static str {
    use crate::api::schema::AgentStatus;
    match status {
        AgentStatus::Blocked => "blocked",
        AgentStatus::Done => "done",
        AgentStatus::Working => "working",
        AgentStatus::Idle | AgentStatus::Unknown => "idle",
    }
}
