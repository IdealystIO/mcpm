# Control Center — working notes for agents

The console for **mcpm** (Model Context Project Management): MCP-first
project management for agent crews, built with Idealyst (Rust).
`README.md` is the architecture; this file is how to work in the repo.

Naming: `mcpm-*` is the project-management system (crates, the MCP
server agents connect to, the `MCPM_*` env vars). **Control Center** is
this console and this cargo package. They are not interchangeable.

    Want ─┐
    Want ─┼─► Feature → Module ──depends_on──► Module → Task
    Want ─┘

The server is a **gatekeeper, not an orchestrator**: it enforces the
invariants (prerequisites, exclusive claims, checklist-proven
completion, the plan validators, the want pool's rules) and never
spawns an agent. When you add a rule, it goes in the store, inside the
transaction it guards — not in a caller.

**A feature is a graph, and every ordering fact is derived from it.**
Stages were retired in migration 0012 (2026-09-10): they expressed one
shape of dependency and a plan that needed another had to hide it in
prompt text. `module_deps` is the edges; `claim_module` checks a
module's own prerequisites; `topo_order` in `store.rs` gives depth and
order at read time; nothing about readiness is stored. When you add a
plan op, it ends in `check_ownership` and, for an edge, `reaches` — the
cycle and overlap rules live once each and every writer calls them.

**Identity is a credential, not an argument.** Over HTTP an agent does
not say who it is: `Store::verify_key` resolves its API key to an agent
name and role, and that is what the ledger records and what the role
gate enforces. Two rules follow. A tool that becomes manager-only goes
in `KeyRole::MANAGER_ONLY` in `mcpm-core`, never in a transport — both
dispatchers read that one list. And `get_context` must keep ignoring its
arguments on a keyed connection: taking `agent_name` from the caller
there hands back the one thing the key exists to make unforgeable.

A key is per MACHINE, and a subagent cannot present a different one, so
**a delegation token is the one identity that travels in band** —
`mint_worker` issues it, the manager pastes it into the subagent's
prompt, and `Store::resolve_delegation` turns it back into an `Actor`.
It is still not an argument the caller gets to assert: it is minted
server-side, paired to ONE key, forced to WORKER whatever key carried
it, and scoped to one module. That is what lets one process tree hold a
manager and its workers.

**A WORKER MAY MINT, INSIDE A FEATURE IT ALREADY HOLDS.** `mint_worker`
came off `MANAGER_ONLY` on 2026-09-04. A flat gate had made the fleet's
own design impossible: a cloud branch box holds a worker key by
construction, so "modules concurrent as subagents" had never once
happened — measured across two live boxes, zero subagents ever, and an
8-vCPU instance averaging under 15% CPU because one agent worked one
module at a time.

In practice a box dispatched a whole feature now: claims one module
`next_work` returned itself, calls `mint_worker` for each other
dispatchable module, spawns one subagent per module with its token,
lets them claim and complete independently, then asks `next_work` again
as completions release more. Not for a module that is not dispatchable —
its claim is refused anyway. Everything `next_work` returns together is
safe to run together; the plan's `owns` declarations are what make that
true, so a planner that leaves them off is trusting the reader.

The rule that replaced the gate lives in `Store::mint_worker`, inside
the transaction, like every other invariant here: a worker's key may
mint only for a module whose feature it holds a live claim in. So
authority comes from having been dispatched, and it lapses on its own
when the box completes its last module — nothing revokes it. A manager
is unrestricted, because dispatching into a feature it holds no claim in
IS the manager's job.

**The two properties the gate protected are still enforced, and neither
ever depended on the list.** Delegation is one level deep because
`Store::mint_worker` refuses a minter with `is_delegated()`, and a
resolved delegation is forced to WORKER whatever key carried it. A
worker still cannot hand itself work, because it cannot reach a feature
it was never dispatched to. `issue_worker_key` stays manager-only: it
mints a standing credential rather than a scoped expiring one, and
dispatch is the manager's act.

**Delegation is the same-machine mechanism; a box that is not this
machine gets a key.** `issue_worker_key` is the manager-only tool for
that, and it exists because boxes sharing one key are one identity the
ledger cannot split — mutual exclusion between them silently does not
hold. Do not reach for delegation to cover it: `mint_worker`'s
`for_key_id` binds a token to another box's key for the case that needs
both a box identity and a single-module scope, and it moves where the
boundary sits (from "this machine" to "whoever holds that key") without
changing its shape. One key honours a token, always — a design where
"any key" resolves one gives up the property that makes pasting it into
a prompt safe, and it is the one thing here that must not be traded.

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

## Knowledge work: read KNOWLEDGE.md first

The memory graph has one rule that is easy to violate by accident:
**memories are immutable — no update, no delete, corrections
supersede.** [KNOWLEDGE.md](KNOWLEDGE.md) is the standing rulebook for
it (edge types, derived node state, what decay may and may not do, and
the anti-patterns each of which is a plausible next step that breaks the
model). Read it before touching `commit_memory`, `search_memory`, or
anything that writes to `memories`.

## Layout

| Path | What it is |
| --- | --- |
| `crates/mcpm-core` | Domain + Postgres store. Every invariant lives here, in one place each. |
| `crates/mcpm-mcp` | The MCP server agents connect to: tools, prompts, `project://` resources. Two transports (`rpc.rs` is the shared dispatcher), plus the key CLI. |
| `crates/api` | Wire DTOs, the capture-syntax parser, `#[server]` fns, and the `mcpm-web` host binary. |
| `src/` | The Idealyst console. `components/` is one module per view. |

When you change anything that affects ranking — weights, synonyms, the
tsquery construction — run `cargo test -p mcpm-core --test retrieval_eval
-- --nocapture` and read the margins, not just the pass/fail. It has
already caught two bugs that reasoning missed: a synonym crossing word
senses (`schema → table` returning a UI note), and short function words
scoring a perfect trigram similarity so every natural-language question
ranked by whichever entry contained "the".

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
cargo test --workspace                               # needs the db up
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
- **Memories are immutable, and the store is where that is enforced.**
  There is no `update_memory` and no delete: `commit_memory` takes a
  `supersedes` list, and the edge is written in the same transaction as
  the memory, because a correction that landed without its link would
  leave both versions reading as current. Edges only ever point from a
  newly committed memory backwards, which is what makes a cycle
  structurally impossible rather than something to check — any future
  tool that links two PRE-EXISTING memories loses that and must add the
  check itself.
- **Supersession is commit-time only; standing relations are not.**
  That asymmetry is what keeps the history acyclic — an edge that can
  only be created alongside its newer end always points backwards in
  time — so `relate_memories` refuses the four supersession kinds. Both
  kinds share `memory_edges`, and exactly one place tells them apart:
  the `ed` CTE lists the supersession kinds BY NAME, so a kind added to
  the schema and forgotten there fails safe.
- **No score is ever stored on a memory.** `knowledge_weights` is read
  at query time by `MEMORY_FIELDS`, so retuning is a SQL `UPDATE` with
  no backfill and every entry stays comparable. The corollary is that
  the ranking SQL exists once and is shared by every read path — two
  copies that drifted would mean agents and the console were looking at
  differently ordered versions of the same base, which nobody would
  notice.
- **The console reads in tiers, and every fetch is issued from one
  place.** `load_board` is the only global read and is scalars per
  feature; a feature's tree, a module's handoff and history, a page of
  a feed, a page of the pool are each their own read, fetched when
  they are on screen. The scheduler is `start_sync` in `src/app.rs`:
  it computes what the screen needs each frame, what a tick has made
  stale, and issues the difference. A view that spawns its own fetch
  breaks that — a rev bump rebuilds it, it fetches again, and the
  console quietly doubles its traffic. And nothing may grow back into
  the board: it was once the whole project (every tree, every document
  body, 500 events per feature, on every tick), and it did not scale.
  A tick's `feature_id`/`kind` are what let the client skip the tree
  refetch, so the notify trigger must keep sending them.
- **A console write is a server function that calls a store method,
  and nothing else.** `create_plan`, `revise_plan`, `delete_plan`,
  `shelve_plan`, `edit_want`, `set_want_state`, `delete_want` in
  `crates/api` map their arguments onto `mcpm_core::PlanOp` and friends
  and stop; the one rule they add is `require_planner` (a worker key
  cannot plan). A check that only the console makes is a check an agent
  never hits, and the two surfaces then disagree about what a plan may
  be. On the console side every edit goes through `components/edits.rs`:
  the form buffers live on `Console` (`form_*`, the plan editor's
  slots), and the request runs in `ActionRunner`, a hole keyed on
  `action_seq` — never from the button's own handler (UX_GUIDELINES
  rule 25). A `#[server]` fn's arguments are sent as a JSON array on
  the wire (`'["feat_x", true]'`), and the reply is `{"Ok": …}` /
  `{"Err": …}` — what to send when you curl one.
- **The capture syntax is defined once**, in `crates/api/src/capture.rs`,
  and used by both the editor's highlighting and the server function
  that writes. Never add a second parser — drift between them means the
  UI colors something different from what it stores.
- **Keep the capture composer outside any `switch` keyed on
  `Console.rev`.** A background poll would rebuild the text node and
  steal focus mid-sentence. Its buffers live on `Console` for the same
  reason.
- **`scroll_view` is single-axis and clips the other one silently.**
  Vertical unless `horizontal = true`, never both — so the board and the
  dependency graph each nest two, and the inner axis is pinned with
  `height: 100%` rather than `min_height`, which would let the row grow
  and push the outer scrollbar off-screen. UX_GUIDELINES rule 23 has the
  full pattern. Nothing about this fails a build or a test: it renders
  as a view that looks complete until the data gets wide.
- **Extension SDKs must be registered** in `register_scene_extensions`
  (`codeblock::register`, `table::register`). An unregistered payload
  panics at realize. Some idea-ui components ARE such payloads —
  `idea_ui::Table` renders the `table` SDK — so a component library
  import can need a registration line, and the crate must be a direct
  dep to be nameable there. Worse, `table` only emits its payload on
  **wasm** (off-web it lowers to a grid of views), so a host-side mount
  test cannot see the failure: pair the mount test with the
  handler-count assertion in `wants.rs`, and check a new SDK-backed
  component in the browser.
- **A delegation token is resolved BEFORE the role gate, not after.**
  Resolving it is what decides which role the gate applies: a token
  forces `KeyRole::Worker` however manager the key behind it is, and
  that single substitution in `rpc.rs` is the whole manager/worker split
  inside one process tree. Gate first and every subagent on a laptop
  inherits MANAGER again — silently, because everything still works.
  The other half is the scope: a delegated `Actor` may touch only the
  module it was minted for, and that check lives in `require_claim`
  beside the claim check, because both answer "may this actor write to
  this module?" and every write verb already funnels through it. A new
  write verb that reads `claimed_by` itself picks up neither.
- **An unresolvable `delegation_token` is an error, never a fall-back.**
  Quietly proceeding as the session's own identity would attribute a
  subagent's work to the machine — exactly the failure the token exists
  to prevent, in exactly the case nobody is watching. It is four errors
  and not one, because the four causes want four different reactions and
  a subagent that cannot tell them apart correctly blocks and escalates.
  The order they are told apart in is the safety argument: the secret is
  verified FIRST, so nothing below it — the expiry, the key it is bound
  to — is observable without already holding the token, and a guesser
  only ever reaches `delegation_unknown`.
- **The auth posture is derived from `HOST`, not configured
  alongside it.** `api::auth_required()` is true whenever `HOST` is not
  loopback, so a reachable console host cannot be an unauthenticated
  one — there is no env var that turns the gate off for a published
  port, and adding one would reintroduce exactly the accident this
  prevents. The subscription reads the same function rather than a
  local of the binary's, because a socket that authenticated
  differently from the POST path would be a hole nobody looks at.
- **The event socket authenticates in its own body**, not at the
  dispatch hook. A browser cannot put a header on a WebSocket
  handshake, so `ConsoleGate` deliberately leaves `on_open` at
  pass-through and `watch_events` verifies its own `key` argument.
  Gating it in the hook would reject every socket, and the console
  would silently fall back to its 30s poll — the same failure mode
  `tests/notify.rs` exists to catch.
- **Never print a `DATABASE_URL`; print `redact_url` of one.** The
  userinfo is a rotating credential, and a log has a different retention
  policy and a different audience than the secret store it came from —
  so this is a leak that raises no error and shows up in a journal weeks
  later. It applies to the SUCCESS path (a startup banner) as much as an
  error branch; the failure path is worse only because it is the one
  people paste into chats. `redact_url` redacts wholesale rather than
  guess when it cannot find the boundary, because an unencoded `/` or
  `?` in a password is exactly what makes a naive scan conclude there is
  no credential to hide.
- **`API_ORIGIN` in `src/app.rs` is baked into the wasm** at build time
  from `MCPM_API_ORIGIN`, defaulting to loopback. A hosted console built
  without it points every visitor's browser at THEIR own loopback: the
  page renders perfectly, every call fails on the visitor's machine, and
  the server logs show a healthy host that nobody is talking to.
- **`mcpm-web` must reference the `api` crate** (`api::Board::default()`)
  or the linker dead-strips its route inventory and every `/_srv/` path
  404s with no build error.
- **The host binary lives inside `crates/api`** behind
  `required-features = ["server"]`. As its own workspace member it turns
  on `server/server` workspace-wide and strips the client half out from
  under the console.
