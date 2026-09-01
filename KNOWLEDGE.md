# The memory graph

How knowledge is stored, corrected and retrieved in mcpm. `UX_GUIDELINES.md`
is the standing rulebook for screens; this is the one for what the crew
knows.

## The one rule

**Memories are immutable. There is no update and no delete.** A
correction is a *new* memory that supersedes the old one.

Three reasons it has to work this way:

- **The ledger already does.** Events are append-only and stage status
  is derived, never stored. A knowledge base that can be edited in place
  cannot be reconciled against a ledger that cannot.
- **A superseded memory is evidence, not garbage.** It records that this
  project once believed something else, and when it stopped. An agent
  reading only the current rule cannot tell a decision that was argued
  from one nobody has revisited; the chain behind it can.
- **Update and delete are the only two operations that lose
  information**, and this is the one store whose entire value is
  cumulative.

An inaccurate memory is therefore not a defect to be removed. It is the
fossil of a concept that got replaced, and the edge that replaced it is
where the explanation lives.

## Node state is derived

Like stage status, never stored — there is no `status` column to drift.

| State | Means |
| --- | --- |
| `current` | Nothing supersedes it. |
| `superseded` | Something does. Still searchable; out of default results. |
| `disputed` | An open dispute and no supersessor yet. Out of default results, and a manager's problem. |

## Edges

Supersession is the spine of the graph, and it is **typed** — an
untyped "replaces" edge cannot answer the question the history is for.

| Edge | Means | The world, or us? |
| --- | --- | --- |
| `replaces` | The fact changed. | The world moved. |
| `refutes` | It was wrong when written. | We were wrong. |
| `revises` | Same fact, better words or corrected tags. | Clerical. |
| `consolidates` | Several entries folded into one. | Housekeeping. |

The distinction earns its place immediately: *"what did we used to
believe?"* is `replaces` + `refutes`, and it is a different question
from *"show me the clean lineage"*, which ignores `revises`. One
untyped edge answers neither.

Supersession is a **DAG, not a chain**: one memory may supersede several
(consolidation) and several may supersede one (a rule that split).

Beyond the spine, edges that describe standing relations rather than
history: `refines` (a narrower case of a broader rule), `depends_on`
(true only because something else is), `contradicts` (a flag for a
human, never averaged away), `relates_to` (plain association, the
weakest claim).

Both kinds share one table, so the graph is one graph. Exactly one place
distinguishes them: the `ed` CTE that derives "superseded" lists the
four supersession kinds **by name**. That is deliberate — a kind added
to the schema and forgotten there fails safe, because the new relation
simply does not withdraw anything, where the opposite default would
silently retire memories nobody meant to.

Supersession stays **commit-time only** and standing relations can be
declared between two existing memories. That asymmetry is what keeps the
history acyclic: an edge that can only be created alongside its newer
end always points backwards in time.

**Every edge carries its own rationale**, like a want→feature link: the
reason a connection exists is a property of the connection, and a graph
of unexplained edges is a graph nobody trusts.

## Retrieval

Decay changes **rank**, never **existence**.

- **Default**: current heads only, ranked by relevance × freshness.
- **`include_superseded`**: the whole graph, each entry carrying its
  state.
- **`history(id)`**: walk the chain in both directions.

**Decay has a floor.** It must never reach zero and never reach a
threshold that filters a memory out of an explicit search. A memory that
cannot be found is deleted, whatever the schema says — and that is the
one thing this design forbids.

## Signals are stored; scores are computed

**No score is ever written to a row.** The table holds *facts* — counts,
timestamps, edges — and the scoring function runs at read time over
them. Every heuristic is tracked independently, so the function that
combines them stays something we can change our minds about.

The signals, each recorded on its own:

| Signal | Stored as | Answers |
| --- | --- | --- |
| Age | `created_at` | How long has this stood? |
| Subject churn | events on the subject since `created_at` | Has the thing it describes moved? |
| Corroboration | confirmations, by distinct agent | Who else has checked it? |
| Dispute | disputes, by distinct agent | Does anyone say it is wrong? |
| Supersession | typed edges | Has it been replaced, and how? |
| Use | touches, by distinct agent, with recency | Who deliberately leaned on this? |

Three things fall out, and they are the reason this shape is worth the
discipline:

- **Reweighting is free and retroactive.** Change the function and the
  whole base re-ranks — no migration, no backfill, no rows carrying a
  number computed under rules that no longer exist. A stored score would
  have to be recomputed everywhere, and until it was, two entries'
  scores would not be comparable.
- **The heuristic becomes falsifiable.** With the raw signals on record
  you can replay: fix a set of (question, the entry that should win)
  pairs, and measure a candidate function against it. Without that, weight
  tuning is taste with no way to be wrong — which is how a scoring
  function ossifies at whatever its first author guessed.
- **It stays explicable.** A band with a reason ("stale — the module it
  describes has changed 12 times since") is reconstructible from the
  signals on demand. A stored `0.73` can only ever be reported.

Note what is *not* in that table: a retrieval count. See **Use is
attested, not observed** below.

The weights live in a table alongside `knowledge_synonyms`, for the same
reason that one is a table: editable with plain SQL, in effect on the
next query, with the code's defaults seeded by the migration so the
starting point is version-controlled.

**Caching is allowed; storing is not.** If churn counting gets expensive
the answer is a materialized view refreshed on a schedule — a cache of a
derived value, discardable and rebuildable. What must never happen is a
signal's *derived* form becoming the source of truth for it.

## Use is attested, not observed

**Nothing is recorded when a memory is returned by a search.** Use is
recorded only when an agent says it used something:
`touch_memory(memory_ids)` — bulk, because an agent that leaned on five
entries should say so once, not five times.

The distinction is telemetry versus attestation, and it is the whole
point:

- A **retrieval count** records what the ranker chose to show. The agent
  had no say in it, so the number measures the ranker's past behaviour
  and calls it evidence about the memory. Feed that back into ranking
  and it is a closed loop: shown → counted → ranked higher → shown. Good
  entries that never got an early break stay buried, and the number that
  buried them looks like data.
- A **touch** is a claim by an agent that read the entry and used it.
  It can be wrong, but it is *someone's* wrong, and it is about the
  memory rather than about the ranker.

It also removes a write from the hottest read path. Counting retrievals
means every `search_memory` writes, against a pool that starves
visibly under load; a touch is one insert on a call the agent chose to
make.

Three rules keep the signal honest:

- **Count distinct agents, not events.** One agent touching the same
  entry in a loop is one agent's opinion however many times it fires.
- **Weight by recency, and let old touches fade.** Lifetime popularity
  accumulates without bound and is exactly the thing that buries newer
  and better entries. "Is this in active use *now*" does not, and it has
  to keep being earned.
- **A touch is not a confirmation.** "I used this" and "I checked this
  and it holds" are different claims; `touch` and `confirm` stay
  separate tools. An agent that used a memory and was then blocked has
  weakly *dis*confirmed it.

What touch is genuinely good for, beyond ranking: blast radius. *"If
this decision changes, fourteen modules leaned on it"* is a question
only an attested signal can answer, and it is the one an agent revising
a convention actually needs.

## The scoring function

Implemented in `MEMORY_FIELDS` in `store.rs`, with every constant read
from `knowledge_weights` at query time.

```
raw(m)      = Σ over DISTINCT agents:  w_touch·touches
                                     + w_confirm·confirms
                                     + w_dispute·disputes
evidence(m) = tanh(k · raw)                                    ∈ [−1, 1]
newer(m)    = fraction of same-KIND memories created after m   ∈ [0, 1]
penalty(m)  = pen_superseded if replaced,
              pen_disputed   if disputes > confirms,
              else 0
standing(m) = clamp(evidence − decay·newer − penalty, −1, 1)
mult(m)     = floor + (1 − floor)·(standing + 1)/2             ∈ [floor, 1]
score(m)    = textual relevance(m) × mult(m)
```

Four things in there are load-bearing, and each was chosen against a
specific failure:

- **`tanh`, and `k = 0.25`.** Saturation is what stops evidence
  compounding without bound. `k` decides *where* it saturates, and it
  matters more than the weights: at `k = 1`, three confirms and twenty
  both score 1.00, so the number stops carrying information exactly
  where a real project starts producing it. At 0.25 the 1–10 range stays
  legible.
- **`newer` is a fraction, within kind.** A fraction is self-normalizing
  — it cannot run away as the base grows, and there is no per-insert
  constant to tune. *Within kind* because most of this table is
  machine-written outcomes (every module completion commits one), and
  counting globally would bury curated conventions under activity that
  could never have superseded them.
- **`penalty`.** Evidence a memory earned while it was true does not
  disappear when it stops being true. Without this, a well-confirmed old
  belief outranks the entry that corrected it, and the history view
  leads with the thing that is no longer the case. Found by testing, not
  by reasoning.
- **The map onto `[floor, 1]`.** Standing is signed, and a signed
  multiplier would invert the ordering — a refuted entry would sort
  below things with no relevance at all. A zero multiplier would be
  worse: a delete performed by arithmetic. The floor is what makes
  "never delete" true at query time and not just in the schema.

## Freshness

The first function over those signals. Three inputs, kept separate,
shown decomposed with their reasons — never summed into one number the
reader has to take on faith.

- **Age**, modulated by kind. An `outcome` never becomes false (it
  happened); a `convention` goes false silently; a `reference` rots
  fastest.
- **Churn on its subject** since it was written, counted off the event
  ledger. This is the real signal: a note about a module is stale
  because *the module changed*, not because a quarter passed.
- **Corroboration** — how many *distinct* agents confirmed it.

Freshness does not have to model "wrong", because superseded and
disputed entries are already out of the default results.

## What the store must enforce

In the transaction that writes the edge, never in a caller:

- **No cycles** in supersession.
- **No self-supersession**, and no superseding with byte-identical
  content — a no-op that would pollute the lineage with a step that
  changed nothing.
- **No disputing your own memory.** Corroboration needs independence,
  and an agent grading its own work is not evidence.
- **Idempotent commit.** Identical content at the same scope from the
  same author collapses to the existing memory rather than a duplicate.
  Agents retry; a retry is not a new belief.

## Anti-patterns

Named because each one is a plausible next step that quietly breaks the
model:

- **Usage feeding trust.** How often a memory is used measures how
  load-bearing it is, not how true. Even attested touches only say
  agents relied on it — which a wrong memory attracts just as easily,
  right up until something breaks.
- **Observing use instead of asking for it.** Counting retrievals loops
  the ranker's output back into its own input, and buries anything that
  did not rank early. See **Use is attested, not observed**.
- **A single confidence number.** `0.73` is unactionable and falsely
  precise; the number reads as verified when nothing verified it. Bands
  with reasons ("stale — the module it describes has changed 12 times
  since") are actionable.
- **Averaging a dispute.** Dropping a score from 0.8 to 0.6 hides the
  dispute and leaves the memory in circulation. A dispute removes it
  from default results until someone resolves it.
- **Decay to zero.** A soft delete wearing a schema's clothes.
- **Storing a computed score.** It freezes one version of the heuristic
  into the data, makes rows written under different weights
  incomparable, and turns every tuning change into a backfill. Store
  what happened; compute what it is worth.
- **Inferring edges silently.** Co-use is the weakest evidence a
  relation exists. It may *propose* an edge for confirmation; it may not
  assert one. A suggestion that has been declared stops being a
  suggestion — a list that keeps proposing what you already answered is
  one people stop reading.
- **Trigram-matching short words.** A three-letter function word scores
  a *perfect* similarity against any text containing it, so fuzzy
  matching on unfiltered query terms ranks every entry by whichever one
  happened to contain "the". Trigrams need a minimum term length; the
  tsquery path handles short words properly by stemming and
  stop-wording them.
- **Synonyms that cross word senses.** `schema → table` looks obviously
  correct and returned a UI note about the table SDK for a database
  question. The riskiest entries are the ones most plainly right in the
  abstract — table, interface, order, unit. A missing synonym costs one
  recall; a wrong one poisons every query containing that word.

## Staging

1. ~~`supersedes` edges (typed, with rationale), `touch`/`confirm`/
   `dispute`, derived node state, standing. No graph traversal beyond
   the spine.~~ **Done** — migration `0008_memory_graph.sql`, the
   `touch_memory` / `confirm_memory` / `dispute_memory` /
   `memory_history` tools, and `tests/memory_graph.rs`.
2. ~~Standing edges declared at write time; a memory's neighbours on
   its console card.~~ **Done** — `relate_memories`, and the knowledge
   drawer, which is where the edge rationale becomes visible at all.
3. ~~Suggested edges.~~ **Partly done** — a bulk `touch_memory` stamps
   a batch, and `memory_history` returns co-used entries as
   suggestions. Still open: edges derived from structure (shared
   subject, shared tags), and a visual graph, which could reuse the
   machinery in `src/components/graph.rs`.

## Tuning

`tests/retrieval_eval.rs` is the fixture: fixed questions, the entry
that should win each, and the reason the case exists. It prints the
margin over the runner-up for every case, so a weight is swept by
changing one row in `knowledge_weights` and reading the column — which
is how `penalty_machine` got its value.

Two things it is not. It is not a claim the ranking is good; it is a
claim that specific, argued cases come out right. And it is not
something to chase 100% on: a case that passes only under a contorted
weight is telling you the case is wrong. One originally asserted that
"what landed in the store module" should return a particular completion
summary; it failed at every weight including zero, because two summaries
both mentioned landing and the store and the expected winner was
arbitrary. The fixture was right and the case was wrong.
