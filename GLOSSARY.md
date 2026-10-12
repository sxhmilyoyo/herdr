# Herdr

Herdr is a terminal workspace manager for coding agents. A server owns the
session; the terminal UI is one client that presents it.

## Language

### Session organization

**Workspace**:
A top-level container of tabs and panes, usually one project checkout.
_Avoid_: Space (in prose), project

**Tab**:
A named layout of panes inside a workspace.

**Pane**:
One terminal inside a tab. A pane exists whether or not an agent runs in it.
_Avoid_: Window, terminal (when the container is meant)

**Agent**:
A supported interactive coding tool detected running in a pane, with a
lifecycle state such as idle, working, or blocked.

### Sidebar

**Spaces panel**:
The sidebar section that lists workspaces and their git worktrees.
_Avoid_: Workspace list, workspaces pane

**Agents panel**:
The sidebar section that lists every pane that currently hosts an agent.
_Avoid_: Agents pane, agent list, agent sidebar

**Agent entry**:
One agent's block in the Agents panel. It may span several display lines.
_Avoid_: Agent tab, agent row, card

### Agent tree

**Origin pane**:
The pane whose process issued the request that created another pane. A pane
created from the user interface has no origin pane.
_Avoid_: Creator, spawner, caller

**Parent pane**:
The pane an agent entry is nested under. At creation it is the origin pane.
_Avoid_: Owner, supervisor

**Child pane**:
A pane whose parent pane is set. It is shown indented under its parent's agent
entry when both host agents.
_Avoid_: Subagent, worker, delegate

**Agent tree**:
The nesting of agent entries by parent pane. It spans tabs and workspaces,
and only the Agents panel shows it.
_Avoid_: Group, hierarchy

**Promotion**:
What happens to child panes when their parent pane closes: they move under the
parent's own parent, or to the top level when there is none.
_Avoid_: Orphaning, re-parenting (which means a deliberate move)
