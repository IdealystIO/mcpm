# Control Center

The console for **mcpm** (Model Context Project Management): MCP-first
project management for agent crews. One deployed instance is tied to one
project, and the entire agent-facing surface is an MCP server.

    Want ─┐
    Want ─┼─► Feature → Stage → Module → Task
    Want ─┘             ↑        ↑
                        │        └ one worker subagent each,
                        │          concurrent within a stage
                        └ sequential gate, server-enforced

A **manager agent** owns a feature: it plans the tree, dispatches one
worker per ready module, watches the ledger, and closes the feature. A
**worker agent** owns one module: it claims, works the checklist,
records what it learned, and exits through exactly one door.

The server is a **gatekeeper, not an orchestrator**. It never spawns an
agent. It enforces the invariants so a bad plan fails loudly at the tool
boundary.

## Prerequisites

- Docker (for Postgres)
- Rust stable
- The [Idealyst](https://github.com/IdealystIO/idealyst-native) CLI, for
  the console only: `cargo install --git https://github.com/IdealystIO/idealyst-native idealyst-cli`

## Setup

**1. Start Postgres.** Published on host port 55432 to avoid colliding
with other projects on 5432/5433.

```bash
docker compose -p control-center-devc \
  -f .devcontainer/docker-compose.yml \
  -f .devcontainer/docker-compose.idealyst.yml \
  -f .devcontainer/docker-compose.host-db.yml \
  up -d database
```

**2. Start the API host.** Migrations run automatically on connect, so
this step also creates the schema.

```bash
cargo run -p api --bin mcpm-web --features server     # http://127.0.0.1:3210
```

**3. Start the console.**

```bash
idealyst dev --web --local --port 8090                # http://127.0.0.1:8090
```

All three ports are deliberate. 5432, 5433 and 8080 are assumed to
belong to other projects.

### Connecting agents

Agents reach mcpm over stdio through `.mcp.json`, already in this repo:

```json
{
  "mcpServers": {
    "mcpm": {
      "command": "cargo",
      "args": ["run", "--quiet", "-p", "mcpm-mcp"],
      "env": {
        "DATABASE_URL": "postgres://app:app@localhost:55432/app",
        "MCPM_PROJECT_NAME": "control-center"
      }
    }
  }
}
```

The server is launched per connection and shares one database, so claims
and gates stay race-safe across concurrent agents. Restart your MCP
client after changing this file.

### Verifying

```bash
curl -s -X POST http://127.0.0.1:3210/_srv/load_snapshot \
  -H 'content-type: application/json' -d 'null' | head -c 200
```

A `200` with JSON means the API host and database are both up. Then open
http://127.0.0.1:8090 and check the header pill reads **live**, which
means the event WebSocket is connected. If it reads **polling**, the
socket is down and the console has fallen back to a 30 second refresh.

## Using it as an agent

`get_context(agent_name, role)` is the mandatory first call. Every other
tool attributes its writes to that identity and will refuse until it is
made. Errors carry a `code`, a `message`, and a `hint` saying what to do
next, so recovery does not require reading this file.

Beyond the 22 tools there are:

- **Prompts**, which brief a fresh agent for a role:
  `manager_briefing`, `worker_briefing`, `compose_wants`.
- **Resources**, read-only: `project://status`, `project://wants`,
  `project://events`, and `project://features/<id>`.

Ids are prefixed by kind: `feat_`, `stg_`, `mod_`, `tsk_`, `want_`.

## The gate

The one coordination rule the server owns: **a module may be claimed
only when every module in every earlier stage of its feature is done.**
It is checked in exactly one place, `claim_module`, inside the same
transaction that takes the claim.

When a manager dispatches a worker too early, the rejection travels two
independent paths. The worker gets a `STAGE_LOCKED` error whose `hint`
tells it to stop and report back, and the server appends a
`premature_claim` event the manager sees on its next `feature_status`
poll. Neither path depends on the other working.

## Wants

Ideas arrive long before plans do, so they get their own front door.
`add_want(body, tags)` is the cheapest write in the API: no structure
demanded, one idea per call, in the words it was said in.

A want is not a small feature. Features are composed out of *groups* of
wants, and one want can inform several. Composition is a separate,
deliberate step: `promote_wants` takes a group plus either the plan they
add up to or the id of a feature they belong in, then writes the
feature, every want/feature link, the event, and a feature-scope *origin
memory* in one transaction. That memory closes the loop. A worker four
stages downstream calling `search_memory(direction='up')` reads the raw
ideas its module exists to satisfy, in the user's own phrasing, beside
the rationale the composing agent recorded.

The pool's invariants:

- **`promoted` is derived, never held.** It means a `want_features` row
  exists, so the pool cannot claim an idea is planned when no feature
  holds it.
- **A promoted want is frozen.** Its wording is quoted by a plan, so
  rewriting it would make the record lie. Retagging stays legal.
- **Declining requires a reason.** An idea dropped without a why is just
  a lost idea, and the next agent re-proposes it. Declined wants are
  refused by `promote_wants` until explicitly reopened, and the refusal
  carries the original reason.
- **A want may inform several features.** Linking it twice to the *same*
  feature is refused.

### Capturing from the console

The want pool is the one place the console writes. Its capture card is a
`code_editor` where **every line is its own want**. It highlights `#tag`
as you type: existing tags in the info tone, tags that do not exist yet
in the success tone with a dotted underline, so "this will create a new
tag" is visible before you commit. **Tab** completes the tag under the
caret against the registry, to the longest common prefix when several
match, shell-style.

The syntax is one line of prose plus `#tags` anywhere in it. Tags are
lifted out of the body (a want's body is what was wanted; the tags are
how it is filed), a `#` before a digit stays prose (`see issue #42`),
and a line holding nothing but tags is not a want.

That syntax is defined **once**, in `crates/api/src/capture.rs`, and
used twice: the editor highlights with it, and the server function
parses the same buffer with the same code when it writes. There is no
second parser to drift, so what you saw colored is what lands.

### Tags

Tags are first-class rows, so one can exist before any want uses it.
Typing `#anything` creates it on the way through, and `create_tag`
registers one up front. Either way the name normalizes to a slug
(`Field Reports` becomes `field-reports`) while the label keeps the
spelling it was first typed with. Agents read the list through
`list_tags` and should reuse an existing tag rather than coin a synonym.

## Staying live

The console does not poll for changes. Every committed row in the event
ledger fires the `events_notify` trigger, which `pg_notify`s its `seq`.
`mcpm-web` holds a `PgListener` on that channel and streams a `Tick` to
each connected console over a WebSocket (`#[subscription] watch_events`).
The console answers a tick by refetching the snapshot. The tick carries
no data beyond the sequence number, so `apply_snapshot` stays the one
place the client model is written.

The route matters. The MCP server writes in a *different process* from
the web host, so the notification has to cross the database. An
in-process broadcast would only ever carry the console's own captures.

A 30 second snapshot poll remains as the fallback for a dropped socket.

## Crates

| Crate | What it is |
| --- | --- |
| `crates/mcpm-core` | Domain and Postgres store. The stage gate, exclusive claims, checklist-proven completion, the want pool, the append-only event ledger, and scoped memory search. Every invariant is enforced inside a transaction. |
| `crates/mcpm-mcp` | The MCP server: 22 tools, three briefing prompts, and read-only `project://` resources, over stdio. |
| `crates/api` | Wire DTOs, the capture-syntax parser, the `#[server]` functions and `#[subscription]` the console calls, plus the `mcpm-web` host binary (feature-gated). |
| `src/` | The Idealyst console: want pool with capture composer, board, hierarchy, live feed, dependency graph, composed-from, and the module and want drawers. |

## Development

```bash
cargo check --workspace                              # console + client half
cargo check -p api --bin mcpm-web --features server  # the server half
cargo test -p mcpm-core -p api -p control-center     # needs Postgres up
idealyst lint
```

Both `cargo check` lines matter. They compile mutually exclusive halves
of the `server` SDK, so running only one hides breakage in the other.

`cargo test` covers three layers. `-p api` checks the capture syntax
(tag lifting, `#42` staying prose, tags-only lines, UTF-16 caret
conversion). `-p control-center` checks Tab completion against a seeded
registry. `-p mcpm-core` runs against a real Postgres, replaying the
worked scenario end to end (premature claim, stage unlocks, discovered
tasks, blockers, memory search directions, every completion guard),
exercising the want pool (capture, search, group composition, the
frozen-once-promoted rule, declines and reopens, one want across two
features, atomicity of a promotion naming an unknown id, the tag
registry, bulk capture), and asserting that a committed event reaches
the liveness stream. Each run creates and drops its own scratch
database.

### A note on migrations

sqlx checksums every applied migration, so an already-applied file can
never be edited in place. `0004_event_notify.sql` still names the old
`ccpm_events` channel for exactly this reason, and
`0005_notify_channel_rename.sql` replaces the function to notify on
`mcpm_events`. Add a new migration rather than correcting an old one.
