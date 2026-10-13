# Pane parent link is a server-owned, pane-scoped fact

Status: accepted (2026-10-12)

The Agents panel needed to show which agents were created by which other agent,
in the style of Firefox Tree Style Tab. We decided that every pane created
through the API records the pane whose process issued the request as its
parent. The link is server state, persisted by public pane id, scoped to the
pane rather than to the agent running in it, promoted to the grandparent when
the parent pane closes, and allowed to span tabs and workspaces. The origin
travels as a capability-gated optional `caller_pane_id` on the existing
creating methods, not as a new method. Pane identity is what Herdr already
persists and aliases across moves, so it is the only key that survives agent
restarts, session restore, and live handoff without a second identity system.

## Considered options

- **Agent-session-scoped link**, cleared when the parent agent exits. Rejected:
  the common workflow restarts an agent in the same pane, and the children
  would lose their parent for no user-visible reason.
- **TUI-only grouping** kept in the client. Rejected by the runtime/client
  boundary rule in `CLAUDE.md`; a server-owned fact reaches the CLI, plugins,
  and every presentation for free.
- **Nesting only inside the parent's tab or workspace.** Rejected: the tree
  expresses delegation, not layout, and agents routinely create worktree
  workspaces.
- **A new `pane.set_parent` method called after creation** instead of a gated
  field. Rejected: a second round trip, one snapshot where the child is a root,
  and it introduces re-parenting semantics that v1 deliberately excludes.
- **Tree as a transform over every sort mode.** Rejected in favour of a
  dedicated `tree` sort mode so `spaces` and `priority` keep their meaning.

## Consequences

- The method-shape digest test needs an explicit gated-field list so the
  published generation-1 fixture stays untouched.
- Older servers ignore the field. Implicit attribution degrades silently;
  an explicit `--parent` fails with an error.
- The field is advisory on the server: a caller pane it does not know records
  no parent instead of failing the request, because a command run inside a
  pane against another session carries that session's foreign `HERDR_PANE_ID`.
  The CLI validates an explicit `--parent` before sending.
- A child whose parent pane hosts no agent is drawn as a root until an agent
  starts in that pane; invisible intermediate panes are not skipped.
- Re-parenting and detaching existing panes are out of scope until a later
  decision adds them.
