# Control Center — working notes for agents

The console for **mcpm** (Model Context Project Management): MCP-first
project management for agent crews, built with Idealyst (Rust).
`README.md` is the architecture; this file is how to work in the repo.

Naming: `mcpm-*` is the project-management system (crates, the MCP
server agents connect to, the `MCPM_*` env vars). **Control Center** is
this console and this cargo package. They are not interchangeable.

    Want ─┐
    Want ─┼─► Feature → Stage → Module → Task
    Want ─┘

The server is a **gatekeeper, not an orchestrator**: it enforces the
invariants (stage order, exclusive claims, checklist-proven completion,
the want pool's rules) and never spawns an agent. When you add a rule,
it goes in the store, inside the transaction it guards — not in a
caller.

## UI work: read UX_GUIDELINES.md first

**Anything that touches a screen — new components, edits to existing
ones, or a UX review — starts by reading [UX_GUIDELINES.md](UX_GUIDELINES.md)
and ends by checking the work against it.** It is the standing rulebook
for this repo (component paradigms, spacing, scroll padding, cards,
modals, tables, and what copy is allowed to say). Do not re-derive these
decisions per screen, and do not argue with them in code review — they
are settled.

Two duties that come with it:

- **It is a breathing document.** When you hit or are told about a UX
  problem that is *general* — one that could recur on another screen —
  add it as a new rule so the next agent inherits it. Keep the rule
  stateable without naming a screen; fix genuinely one-off problems in
  place and don't write them down.
- **Some rules cite helpers from a sibling app** (`widgets::blank_table_row`,
  `DateJumper`, `phase.settled()`). Those names may not exist here. The
  *principle* still binds; implement it with this project's own
  components.

Alongside it: prefer **idea-ui** components and theme tokens over
bespoke UI, and look things up in the catalog rather than guessing —
`list_components` / `describe_component` / `list_recipes` /
`describe_primitive` / `read_guide` on the `idealyst` MCP server. A
recipe is compile-verified against the current API; your memory of the
API is not.

## Layout

| Path | What it is |
| --- | --- |
| `crates/mcpm-core` | Domain + Postgres store. Every invariant lives here, in one place each. |
| `crates/mcpm-mcp` | The MCP server agents connect to: tools, prompts, `project://` resources, over stdio. |
| `crates/api` | Wire DTOs, the capture-syntax parser, `#[server]` fns, and the `mcpm-web` host binary. |
| `src/` | The Idealyst console. `components/` is one module per view. |

## Running and verifying

```bash
docker compose -p control-center-devc \
  -f .devcontainer/docker-compose.yml \
  -f .devcontainer/docker-compose.idealyst.yml \
  -f .devcontainer/docker-compose.host-db.yml up -d database   # :55432
cargo run -p api --bin mcpm-web --features server               # :3210
idealyst dev --web --local --port 8090                          # :8090
```

Ports are deliberate — 5432/5433/8080 belong to the user's other
projects. `mcpm-web` binds loopback (`HOST`/`PORT` override); a container
needs `HOST=0.0.0.0` or a published port reaches nothing, and nothing
outside a container should set it — the host is CORS-permissive and
unauthenticated. Before calling UI work done:

```bash
cargo check --workspace                              # console + client half
cargo check -p api --bin mcpm-web --features server  # the server half
cargo test -p mcpm-core -p api -p control-center   # needs the db up
idealyst lint
```

Both `cargo check` lines matter: they compile mutually exclusive halves
of the `server` SDK, and only running one hides breakage in the other.

## Gotchas that fail silently

- **Liveness is cross-process, so it goes through Postgres.** The MCP
  server and `mcpm-web` are separate processes on one database. An
  in-process broadcast channel would carry the console's own writes and
  silently miss every agent write — the exact traffic the console
  exists to show. The signal is LISTEN/NOTIFY: the `events_notify`
  trigger (`0004_event_notify.sql`) announces every committed row, and
  `Store::watch_events` is the only subscriber. Keep the notify in the
  trigger, not in a caller. `tests/notify.rs` guards the path, because
  a break here degrades silently to the 30s fallback poll. The listener
  opens its OWN connection and fans out over a broadcast: one per
  process, not one per console. Taking it from the query pool (4
  connections) caps concurrent consoles and then starves ordinary
  reads, which surfaces as `pool timed out waiting for an open
  connection` and a dead-looking console.
- **The capture syntax is defined once**, in `crates/api/src/capture.rs`,
  and used by both the editor's highlighting and the server function
  that writes. Never add a second parser — drift between them means the
  UI colors something different from what it stores.
- **Keep the capture composer outside any `switch` keyed on
  `Console.rev`.** A background poll would rebuild the text node and
  steal focus mid-sentence. Its buffers live on `Console` for the same
  reason.
- **Extension SDKs must be registered** in `register_scene_extensions`
  (`codeblock::register`). An unregistered payload panics at realize.
- **`mcpm-web` must reference the `api` crate** (`api::Snapshot::default()`)
  or the linker dead-strips its route inventory and every `/_srv/` path
  404s with no build error.
- **The host binary lives inside `crates/api`** behind
  `required-features = ["server"]`. As its own workspace member it turns
  on `server/server` workspace-wide and strips the client half out from
  under the console.
