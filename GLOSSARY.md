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

**Origin agent**:
The agent session running in the origin pane when the request was made. It is
unknown when that pane hosts no agent or its agent does not report a session.
_Avoid_: Dispatcher, creator

**Parent pane**:
The pane an agent entry is nested under. At creation it is the origin pane.
_Avoid_: Owner, supervisor

**Parent agent**:
The agent session a child pane's entry expects in its parent pane. At creation
it is the origin agent. The entry nests only while the parent pane runs its
parent agent, or under any agent there when the parent agent is unknown.
_Avoid_: Supervisor, manager

**Child pane**:
A pane whose parent pane is set. It is shown indented under its parent's agent
entry while both host agents and the parent pane runs the child's parent agent.
_Avoid_: Subagent, worker, delegate

**Agent tree**:
The nesting of agent entries by parent pane and parent agent. It spans tabs
and workspaces, and only the Agents panel shows it.
_Avoid_: Group, hierarchy

**Promotion**:
What happens to child panes when their parent pane closes: they move under the
parent's own parent and take over its parent agent, or to the top level when
there is none.
_Avoid_: Orphaning, re-parenting (which means a deliberate move)
