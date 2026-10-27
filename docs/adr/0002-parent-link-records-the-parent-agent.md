# Pane parent link records the parent agent session

Status: accepted (2026-10-27). Amends ADR 0001.

ADR 0001 scoped the pane parent link to the pane, so a child stayed nested
under whatever agent ran in its parent pane. A reviewer on the upstream
Discussion (#5210) named the cost: when an unrelated agent starts in a reused
parent pane, the tree still shows the children under it, which reads as a
delegation the new agent never made. The link stays keyed by pane and now also
records the agent session the parent pane reported at creation, the origin
agent. Presentations nest a child only while its parent pane reports that
session; a link recorded without a session keeps the pane-only behaviour. The
session is the identity the agent's own Herdr integration reports, already
exposed as `agent_session` and already persisted for resume, so it survives
live handoff and a restore that resumes the agent, and it changes exactly when
a fresh agent session starts in the pane.

## Considered options

- **Agent process id** as the identity. Rejected: it changes on every restart,
  a resume of the same session included, and it does not survive restore.
- **A session link the server clears when the session changes**, replacing the
  pane link. Rejected: the server would have to watch session changes and
  mutate links, and the creation-ancestry fact, still useful to the CLI and to
  plugins, would be lost.
- **Process id as a fallback** for agents that report no session. Rejected: a
  second identity mechanism; those agents keep the pane-only behaviour.
- **Mark a mismatched child** instead of showing it at the top level.
  Rejected: one rule already covers a parent pane that hosts no agent.

## Consequences

- A parent agent restarted as a fresh session, rather than resumed, no longer
  has its children nested under it. Re-adopting them stays out of scope, as in
  ADR 0001.
- Promotion hands the closed parent's own link, session included, to its
  children: they nest under the grandparent only while it runs the agent the
  closed parent expected.
- Pane and agent records gain an optional `parent_agent_session`, and the
  client snapshot gains two optional opaque session keys. Peers that lack them
  fall back to pane-only nesting.
