# Control Center

The console for **mcpm** (Model Context Project Management): MCP-first
project management for agent crews. One deployed instance is tied to one
project, and the entire agent-facing surface is an MCP server.

    Want ─┐
    Want ─┼─► Feature → Module ──depends_on──► Module → Task
    Want ─┘             ↑                        ↑
                        │                        └ one worker subagent each,
                        │                          concurrent unless an edge
                        │                          says otherwise
                        └ a graph, not a ladder: a module is claimable
                          once every prerequisite it names is done

A **manager agent** owns a feature: it plans the graph and writes the
whitepaper, dispatches one worker per ready module, watches the ledger,
and closes the feature. A **worker agent** owns one module: it claims,
works the checklist, writes a handoff for whoever continues, records what
it learned, and exits through exactly one door.

The server is a **gatekeeper, not an orchestrator**. It never spawns an
agent. It enforces the invariants so a bad plan fails loudly at the tool
boundary.

## Prerequisites

- Docker (for Postgres, and MinIO if you want attachments locally)
- Rust stable
- The [Idealyst](https://github.com/IdealystIO/idealyst-native) CLI, for
  the console only: `cargo install --git https://github.com/IdealystIO/idealyst-native idealyst-cli`

The Idealyst framework crates come from **crates.idealyst.io**, not
crates.io. Nothing to install: `.cargo/config.toml` names the index and
is checked in, so `cargo build` resolves them on a fresh clone. Keep the
CLI's version in step with the crates — it post-processes the wasm the
crates produce, and a mismatch surfaces in the browser as a wasm
init failure rather than a build error.

## Setup

**1. Start Postgres** (and MinIO, for attachments). Published on host
ports 55432 and 59000/59001 to avoid colliding with other projects on
5432/5433 and 9000.

```bash
docker compose -p control-center-devc \
  -f .devcontainer/docker-compose.yml \
  -f .devcontainer/docker-compose.idealyst.yml \
  -f .devcontainer/docker-compose.host-db.yml \
  up -d database minio
```

**2. Start the API host.** Migrations run automatically on connect, so
this step also creates the schema. The `MCPM_S3_*` variables point it at
MinIO; leave them off and everything but attaching files still works.

```bash
MCPM_S3_ENDPOINT=http://localhost:59000 \
MCPM_S3_ACCESS_KEY=minioadmin MCPM_S3_SECRET_KEY=minioadmin \
cargo run -p api --bin mcpm-web --features server     # http://127.0.0.1:3210
```

It binds `127.0.0.1:3210`; `HOST` and `PORT` override the two halves. In
a container, published ports only reach a process listening on the
container's external interface, so run it with `HOST=0.0.0.0` there.

On loopback the host runs open — no key, permissive CORS — which is what
you want for local development. Setting `HOST` to anything else turns the
key gate on automatically, so a host that the network can reach is never
an unauthenticated one. See [Deploying it as a service](#deploying-it-as-a-service).

**3. Start the console.**

```bash
idealyst dev --web --local --port 8090                # http://127.0.0.1:8090
```

All of these ports are deliberate. 5432, 5433, 8080 and 9000 are assumed
to belong to other projects.

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
        "MCPM_PROJECT_NAME": "control-center",
        "MCPM_S3_ENDPOINT": "http://localhost:59000",
        "MCPM_S3_ACCESS_KEY": "minioadmin",
        "MCPM_S3_SECRET_KEY": "minioadmin"
      }
    }
  }
}
```

The server is launched per connection and shares one database, so claims
and gates stay race-safe across concurrent agents. Restart your MCP
client after changing this file.

Stdio carries no authentication, and does not need any: the process is a
local pipe that your own agent runner started. Agents on *other* machines
connect over HTTP instead — see below.

### Verifying

```bash
curl -s -X POST http://127.0.0.1:3210/_srv/load_board \
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

Beyond the 28 tools there are:

- **Prompts**, which brief a fresh agent for a role:
  `manager_briefing`, `worker_briefing`, `compose_wants`.
- **Resources**, read-only: `project://status`, `project://wants`,
  `project://events`, and `project://features/<id>`.

Ids are prefixed by kind: `feat_`, `mod_`, `tsk_`, `doc_`, `want_`.

## The gate

The one coordination rule the server owns: **a module may be claimed
only when every module it `depends_on` is done.** It is checked in
exactly one place, `claim_module`, inside the same transaction that
takes the claim.

When a manager dispatches a worker too early, the rejection travels two
independent paths. The worker gets a `PREREQS_OPEN` error whose `hint`
tells it to stop and report back, and the server appends a
`premature_claim` event the manager sees on its next `feature_status`
poll. Neither path depends on the other working.

Everything else about ordering is derived from the graph at read time,
like every other status here: a module is *ready* when it is unclaimed
and its prerequisites are all done; its *depth* is the longest path from
a root, which is the column the console draws it in. `next_work` returns
the ready frontier, and a ready set is an antichain by construction —
two ready modules cannot depend on each other — so "they can run
concurrently" is a fact about the graph rather than a hope about the
plan. Completing a module emits `module_unlocked` for every dependent
whose gate it opened.

## The plan is validated whole

`plan_feature` refuses, naming the offender and writing nothing: an
unknown prerequisite, a self-dependency, a cycle, and two modules whose
declared `owns` paths overlap without one depending on the other. That
last check is the one that catches two writers on one file — the plan
that says "concurrent" about modules that both edit `store.rs`. `owns`
is optional; an undeclared module is exempt. `revise_plan` re-runs the
same checks after every op, and refuses to add a prerequisite to a
module that has already started (that rewrites the decision that let it
start).

The old `stages` shape is still accepted and lowered to edges — each
module depends on every module of the preceding stage — which is also
how migration 0012 backfilled every feature that existed before the
graph. The gate it produces is identical to the old one.

## Documents

A memory is a fact, retrieved by relevance. A document is read whole, by
identity, and there are two kinds:

- **The whitepaper** is the feature's: the plan as prose, what the
  manager would say to a new hire. Given in `plan_feature`, revised with
  `write_document`, carried into every worker's claim briefing.
- **A handoff** is a module's: how to use what the module built — the
  component or function and where it lives, its parameters, the call,
  what it refuses and why. Written by the claim holder with
  `write_document` as the work takes shape, or with `complete_module`'s
  `handoff` on the way out. The claim briefing carries the handoffs of
  the module's transitive prerequisites first, then the rest of the
  feature's completed modules, so a worker reads how to call the thing
  before it reads the code.

Revisions are append-only; the newest is current.

## Attachments

A document is prose. An attachment is a **file** — a design, a
screenshot, a spec somebody sent, a sample of the data — pinned to a
feature or a want with a **description written for the agent that will
read it**: what the file is and what to take from it. The description
is the part that travels: a worker's claim briefing lists the feature's
attachments by name and description, and the worker fetches only what
its module needs.

- **People attach from the console; agents attach with a tool.** A
  person picks a file on the feature's *Files* tab or in a want's
  drawer (its menu: *Attach file*), writes the description, and the
  console POSTs it to the host. An agent calls `attach_file` with the
  file inline — text as `content`, anything else as `content_base64`,
  up to 8 MiB decoded per call — which is how a worker leaves a results
  CSV or a screenshot of a failing state on the record. Either way the
  description is the contract, and `describe_attachment` lets any
  agent sharpen one once it has read the file and can say what matters.
- **A feature inherits its wants' files.** A file attached to an idea
  reaches every feature the idea is composed into, marked *via* that
  want, without being copied — un-link the want and the file goes with
  it. On the feature's list those rows open the file and nothing else;
  their edits are on the want's own drawer.
- **The bytes live in an object store** — S3, or MinIO locally — and
  the record (name, type, size, hash, who, when, the description) in
  Postgres. `read_attachment` returns text-like files inline and every
  file as a link good for an hour; the console opens a file through the
  host, which 302s to the same kind of link.

Configure the store with `MCPM_S3_ENDPOINT`, `MCPM_S3_ACCESS_KEY` and
`MCPM_S3_SECRET_KEY` (the devcontainer's `MINIO_*` variables and the
`AWS_*` pair are honoured too), and `MCPM_S3_BUCKET` to name the bucket
(default `mcpm-attachments`; created on a custom endpoint, expected to
exist on AWS). `MCPM_S3_PUBLIC_ENDPOINT` is for the case where the store
is reached by one name from the server and another from a browser — a
devcontainer's `http://minio:9000` versus the host's `localhost:59000` —
because a presigned link signs the host it was minted for. Both the
console host and the MCP server take the same variables; a server with
none set serves the records and refuses only the bytes, saying why.

## Announcements: the crew's own words

An agent that is quiet between one checked-off task and the next is
indistinguishable from one that is stuck, and the checklist cannot say
why — "the e2e run went red and I am fixing it" is not a task. So any
agent may `announce(subject_id, text)`: one line, in its own words, on
the module it holds or on the feature (for the work between modules:
the merge, the suite, a deploy). It is a ledger event and nothing else —
no status changes, nothing is held — and the newest one that names a
subject is derived from the ledger at read time, the way readiness is
derived from the graph: the module card shows it under the ticks, the
feature's header and its row on the home screen carry the newest word
anywhere in the feature, the agent roster shows each agent's last word
beside its claims, and the *Activity* feed lists them all with a `says`
badge. Worker briefings ask for it whenever the checklist alone would
not tell a reader why the box is quiet.

Two neighbours of that on the checklist itself. `complete_task` is meant
to be called the moment a task lands, never as a batch at the end — a
batch reads as silence until the module closes, and a spot replacement
sees the checklist, not the intentions. And `add_task` is for work the
plan did not name, added *before* it is done: a red e2e run, a broken
build, a framework gap. Such a task lands with `origin: discovered` and
the console shows it as **ad hoc** (a badge on the row, a count on the
module and the feature), so where a plan was thin is readable from the
checklist without a transcript.

The roster on the *Activity* tab lists agents still in the picture — a
live claim, a registered health check that has not answered 404, or a
voice heard in the last day — not every name that ever registered.

## Discussion, and questions that hold the work

Every feature, want and module has a discussion — the *Discussion* tab
on a feature's board, a section in the module and want drawers — and
agents read and write the same thread through `list_comments` and
`add_comment`. A comment is Markdown and may carry files (the same
attachments as above; a comment's files list on the feature's Files tab
too). This is where a person and an agent talk about the work beside
the work, and it is the human-in-the-loop surface:

- **A question names who owes the answer, and holds the work until it
  lands.** `ask_question` (or the composer's *Question* mode) takes an
  `assigned_to` — a person's name as the console records it, the
  manager agent, a worker, or nobody for anyone. While a question is
  open on a **module**, its worker sees `blocked` and nobody can claim
  it; on a **feature**, nothing in it is dispatchable (`next_work`
  says why and `claim_module` refuses with `PENDING_RESOLUTION`); on
  a **want**, it cannot be promoted. Nothing about this is stored as
  state: the gate and the tree read open questions at the moment they
  decide, so `dispatchable` and the claim cannot disagree.
- **An answer is what releases it.** `answer_question` (or *Answer* on
  the question's row) may be given by the assignee, by the asker
  (withdrawing, or answering their own), by anyone if it was assigned
  to nobody — and by a person at the console whoever it names, because
  the person outranks the assignment. A blocked module goes back to
  its worker if one still holds it.
- **A worker's blocker is a question owed by the planner.**
  `report_blocker` opens one, so blockers finally have a resolution
  path: the module is held — for its own worker too — until the answer
  lands, and the answer is on the record beside the problem.
- **Agents are told what they owe.** `get_context` lists the open
  questions assigned to the caller and puts answering them first in
  `suggested_next`; the console's home screen lists every open question
  and who it waits on. A claim briefing carries the newest of the
  module's and feature's discussion.

Notes can be edited and deleted by their author; an open question can
be withdrawn by its asker; an answer, and an answered question, are
history and stay.

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
modules downstream calling `search_memory(direction='up')` reads the raw
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

The Capture screen is a `code_editor` where **every line is its own
want**. It highlights `#tag`
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

### Editing from the console

A person at the console can do by hand what an agent does over MCP:
plan a feature (the New plan screen — name, description, whitepaper,
modules with tasks, prerequisites and owned paths), revise one (the
`⋯` menu on a feature's header and in a module's drawer: rename,
describe, add or remove modules, tasks and prerequisites, shelve),
delete a plan, and edit, decline, reopen or delete a want (the `⋯` in
the want drawer). Every one of these calls the same store method the
matching tool does, so the rules are the store's: a plan with a cycle
or overlapping owners is refused whole, a module that has been claimed
cannot be removed, a feature with work in it cannot be deleted (shelve
it), and a want a feature was composed from keeps its wording and cannot
be declined or deleted. The refusal comes back in the store's own words
and is shown in the form. A worker key cannot edit from the console;
a console key, a manager key, or an open loopback host can.

The pool itself is paged server-side and filtered by a menu: any set of
states (loose by default — it is the inbox) and any set of tags,
searched rather than listed.

### Tags

Tags are first-class rows, so one can exist before any want uses it.
Typing `#anything` creates it on the way through, and `create_tag`
registers one up front. Either way the name normalizes to a slug
(`Field Reports` becomes `field-reports`) while the label keeps the
spelling it was first typed with. Agents read the list through
`list_tags` and should reuse an existing tag rather than coin a synonym.

## Staying live

The console does not poll for changes. Every committed row in the event
ledger fires the `events_notify` trigger, which `pg_notify`s the row's
`seq`, type and feature. `mcpm-web` holds a `PgListener` on that channel
and streams a `Tick` to each connected console over a WebSocket
(`#[subscription] watch_events`).

The console reads in tiers, and a tick says which tier to refetch.
`load_board` is the one global read — a row of counts per feature, the
attention list, the newest few events, the roster, the tags, the pool's
counts — and every tick refetches it, because it never grows past the
feature count. The selected feature's tree (`load_feature`), the open
module's handoff and history (`load_module`) and the visible activity
feed (`load_events`, newest first, paged) are refetched only when the
tick names that feature; the want pool is paged by `search_wants` and
refetched on a want or tag event. The tick carries nothing the console
renders — it re-reads the store rather than trusting a payload.

The route matters. The MCP server writes in a *different process* from
the web host, so the notification has to cross the database. An
in-process broadcast would only ever carry the console's own captures.

A 30 second poll of everything on screen remains as the fallback for a
dropped socket.

## Deploying it as a service

One shared instance for a cluster of agents running on their own
machines. Two things change from the local setup: the MCP server gets an
HTTP transport, and every caller presents an **API key**.

### A managed database

Point `DATABASE_URL` at it and add `?sslmode=require`. RDS, Cloud SQL,
Azure Database, Neon and Supabase all refuse an unencrypted connection,
and the client is built with TLS for that reason.

**Percent-encode the password.** A generated master password is drawn
from a wide punctuation alphabet, and `:` `/` `?` `#` `[` `]` `@` are
all structural in a URL — an unencoded one makes the URL parse as
something else entirely rather than fail as a bad password. mcpm names
this cause when it sees it, but encoding the value is what fixes it.

No binary here ever prints a `DATABASE_URL`: banners and connect
failures alike go through `mcpm_core::redact_url`, so a log or a pasted
error carries the host and database without the credential.

### Keys are identities, not passwords

A key is issued *for* an agent name and a role, and that is what the
server uses — not what the agent says about itself:

```bash
mcpm-mcp --issue-key --agent planner-01 --role manager --label "planning crew"
mcpm-mcp --issue-key --agent worker-03  --role worker
mcpm-mcp --issue-key --agent console    --role console
```

Each prints one token on stdout. That is the only time its secret
exists: only a SHA-256 of it is stored, so the `api_keys` table is not a
set of usable credentials. Three consequences:

- **`get_context` takes no arguments over HTTP.** The key already said
  who is calling, so the `agent` column in the event ledger stops being
  self-reported — an agent cannot attribute its writes to someone else.
- **Roles are enforced.** A `worker` key is refused by `plan_feature`,
  `revise_plan`, `complete_feature` and `promote_wants` with a
  `FORBIDDEN` envelope. A `console` key is refused by the MCP endpoint
  entirely, so a leaked dashboard key cannot claim a module.
- **Revocation is immediate.** `mcpm-mcp --revoke-key <id>` locks the
  bearer out on its next request; the key stays in `--list-keys` with the
  date, because "what did we withdraw, and when" is what that list gets
  asked.

Issuing and revoking both land in the event ledger — who may act, and as
whom — without the token or its hash.

### Many agents, one key

A key belongs to a MACHINE, and a subagent inherits its parent's MCP
configuration wholesale — it cannot present a different key, or set a
header of its own. So on a laptop holding a manager key, every subagent
is that laptop and is a manager; on a branch box holding a worker key,
every subagent is that box and can never plan. Concurrency survives
this (claims are exclusive per module, and one identity may hold
several), but attribution, mutual exclusion between siblings, and the
manager/worker split do not.

`mint_worker` is the way out, and it is deliberately not an `as_agent`
argument the caller asserts:

```
mint_worker(module_id='mod_4f1c88ae', agent_name='agent.mod.schema')
→ { "delegation_token": "dlg_9a3f…", "expires_at": "…" }
```

The manager puts the token in the subagent's prompt; the subagent passes
`delegation_token` on `get_context` and on every write. The server then
records the minted name instead of the machine's. Five rules make that
safe enough to paste into a prompt:

- **Token + key.** A token is honoured only alongside the key that
  minted it, so off that machine it is inert.
- **Always a worker.** A manager key mints it, but it never inherits
  manager authority — which is what finally lets planning and working
  coexist in one process tree.
- **One module.** The token names its module and is refused against any
  other, so a confused subagent cannot reach into a sibling's work.
- **One level deep.** A delegated identity cannot mint another.
- **It dies with the work.** `complete_module` and `release_module`
  retire it in the same transaction that ends the module, and it expires
  on a TTL regardless (4 hours by default).

Minting lands in the event ledger with the name and the module. The
token itself never does.

This imposes an order on an automated deployment: `--issue-key` writes
to the database, and a network-facing `mcpm-web` refuses to start with
no keys on file. So the sequence is **database up → migrate → mint keys
→ enable the services**, not the other way round.

```bash
mcpm-mcp --list-keys
KEY            ROLE       AGENT       LABEL             LAST USED    STATE
179422e65c3d   manager    planner-01  planning crew     2026-09-01   live
1a4a78a5ce04   worker     worker-03   worker-03         never        revoked 2026-09-01
```

### Is the box actually there?

A fleet of boxes each holding a key has a failure the ledger cannot
see: a box that holds its claims and has stopped. A quota-parked agent,
a reclaimed spot instance and a dev server mid-rebuild are one silence
from here, and `last_seen` only says when it last spoke. So an agent
may register a URL for its machine — the branch's dev server, the page
a person would open to look at it — and the MCP server probes it on a
timer:

```
get_context(agent_name=…, role=worker, health_url='https://box-a.dev.example.com/')
issue_worker_key(agent_name='box-a', health_url='https://box-a.dev.example.com/')
```

The second form is for the manager that provisions the box: it knows
the hostname before the box has ever spoken, and the box appears on the
roster — registered, not yet probed — from the moment its key exists.
For a box that already holds a key, the manager-only
`set_agent_health(agent_name, health_url)` names its row directly; a
worker may register only its own.
The verdict is one of four, because they are four different things to
do next:

| state | what came back | what it means |
| --- | --- | --- |
| `up` | 2xx, 3xx, or a 4xx other than 404 | something at that name answered — a server that refuses us is still a server |
| `down` | 5xx | the front door answered and nothing behind it is serving: a stopped instance behind a load-balancer rule that survives, a dev server rebuilding |
| `gone` | 404 | nothing is registered at that name any more — a shared load balancer's or proxy's "no such host", which is what a removed or scaled-to-zero box looks like |
| `unreachable` | no HTTP answer at all | the path from the server to the URL, not the box; kept apart from `down` so a broken tunnel does not read as a fleet outage |

Only a CHANGE of state is an event (`agent_health`, with `from`, `to`
and the status line), so a healthy fleet is quiet, the ledger says
when a box went down, and the console's roster — which shows the state
as the box's tag and "down since 14:02 · HTTP 503" under it — hears
about it the way it hears about everything else. The header's "N
agents live" counts a box with a check only when it is up.

**It is off until the operator turns it on, and that is the safety
argument.** A URL an agent hands us is a URL the server will fetch from
inside the deployment's network, and a server that fetches whatever it
is told is a proxy into that network for anyone holding a worker key.
So `MCPM_HEALTH_HOSTS` names the host suffixes the server may probe
(`.dev.example.com`; comma-separated; a bare hostname matches only
itself), a registration under any other host is refused with the
variable's name, redirects are never followed, hosts must be names
rather than addresses, and no response body is ever read — the status
line is the whole observation. `MCPM_HEALTH_INTERVAL_SECS` sets the
sweep interval (30 by default). The prober runs in `mcpm-mcp --http`
and nowhere else; a stdio session is a laptop, and a laptop does not
probe the fleet.

### The MCP server over HTTP

```bash
mcpm-mcp --http --bind 0.0.0.0:3211
```

`POST /mcp` carries one JSON-RPC message and answers with one reply;
`GET /health` is an unauthenticated liveness probe (of THIS server —
the agents' own health checks are the section above). Every request needs
`Authorization: Bearer <token>`, and a bad or missing one gets a `401`
with a `WWW-Authenticate` challenge and the same error envelope the tools
use.

There is deliberately no session state — the key identifies the caller on
every request, so there is no `Mcp-Session-Id` to expire and no way for a
reconnect to land on someone else's identity. `--bind` defaults to
loopback; the listener refuses to start if no live key exists, rather
than becoming a wall every agent bounces off.

An agent connects with an HTTP MCP client:

```json
{
  "mcpServers": {
    "mcpm": {
      "type": "http",
      "url": "https://mcpm.internal:3211/mcp",
      "headers": { "Authorization": "Bearer mcpm_..." }
    }
  }
}
```

### The console host

```bash
HOST=0.0.0.0 \
MCPM_ALLOWED_ORIGIN=https://console.internal \
cargo run -p api --bin mcpm-web --features server
```

A non-loopback `HOST` turns the gate on by itself; `MCPM_REQUIRE_AUTH=1`
turns it on for a loopback host too. Turning it *off* for a reachable
host is not expressible — that combination is the accident this
arrangement exists to prevent. With the gate on, `/_srv/*` needs a bearer
key and CORS narrows to `MCPM_ALLOWED_ORIGIN` (unset means no browser
origin at all: a loud misconfiguration rather than a silent hole).
Console captures are then recorded under the key's agent name instead of
`console`.

The console's API origin is baked into the wasm at build time, so a
console served from anywhere other than the machine running `mcpm-web`
must be built with it:

```bash
MCPM_API_ORIGIN=https://console.internal idealyst build --web
```

Left at its default the page loads perfectly and points every visitor's
browser at *their own* loopback, so the calls fail on the visitor's
machine and the server logs stay clean and empty.

Open the console and it will ask for a key; paste a `console` one. It is
kept in the browser's local storage, and the header's key pill is how you
rotate or forget it.

### What this does not cover

- **The event WebSocket carries its key in the connect URL**, not a
  header, because a browser cannot set headers on a WebSocket handshake.
  A URL is likelier to end up in a proxy log than a header is. It is the
  least-privileged key the system issues and both ends sit inside the
  deployment; if this ever fronts something untrusted, the fix is a
  short-lived ticket minted over the authenticated POST channel.
- **The browser holds the console key in `localStorage`.** The
  `credentials` SDK errors on web rather than pretend the browser has a
  keychain, and web is the console's only target.
- **No TLS of its own, by design** — its own listeners, not its
  database connection, which does use TLS. Both listeners speak plain HTTP and
  expect to sit behind an ingress that terminates TLS — a load balancer
  for a public deployment, nothing at all for a private network. What
  the deployment must not do is carry bearer tokens over plaintext
  across a network it does not trust.
- **No rate limiting** on the key check.

## Crates

| Crate | What it is |
| --- | --- |
| `crates/mcpm-core` | Domain and Postgres store. The prerequisite gate, plan validation (cycles, ownership overlap), exclusive claims, checklist-proven completion, documents, the want pool, the append-only event ledger, and scoped memory search. Every invariant is enforced inside a transaction. |
| `crates/mcpm-mcp` | The MCP server: 30 tools, three briefing prompts, and read-only `project://` resources, over stdio or authenticated HTTP. Also the key CLI. |
| `crates/api` | Wire DTOs, the capture-syntax parser, the `#[server]` functions and `#[subscription]` the console calls, plus the `mcpm-web` host binary (feature-gated). |
| `src/` | The Idealyst console: the want pool and capture screens, the plan editor, the module graph, the whitepaper, the discussion and files tabs, live feed, composed-from, the module and want drawers (each with its discussion), and the edit menus on each. |

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
worked scenario end to end (premature claim, per-module unlocks, the
plan validators, documents, discovered tasks, announcements and the
roster's membership rule, blockers, memory search
directions, every completion guard),
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
