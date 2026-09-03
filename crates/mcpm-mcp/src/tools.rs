//! The 28 tool definitions (architecture doc §5). Descriptions are the
//! agent UX: they say what the tool does, who calls it, and what the
//! caller should do with the answer.

use serde_json::{json, Value};

/// Every tool, in one array. Split across two `json!` literals purely
/// to stay under the macro recursion limit.
pub fn tool_defs() -> Value {
    let mut tools: Vec<Value> = tree_tools().as_array().cloned().unwrap_or_default();
    tools.extend(want_tools().as_array().cloned().unwrap_or_default());
    Value::Array(tools)
}

/// The Feature → Stage → Module → Task surface.
fn tree_tools() -> Value {
    json!([
        {
            "name": "get_context",
            "description": "The mandatory first call for every agent. Registers your identity \
                for this session and returns the project, every feature with rolled-up \
                progress, any module claims you already hold, and a suggested next step. \
                Built so a restarted agent can re-orient from this single call.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_name": { "type": "string", "description": "Your stable agent name, e.g. 'agent.feature.invoicing' or 'agent.mod.schema'." },
                    "role": { "type": "string", "enum": ["manager", "worker", "observer"], "description": "manager = owns a feature end to end; worker = owns one module." },
                    "delegation_token": { "type": "string", "description": "Only if you are a subagent that was given one. Identifies you as the minted worker rather than as the machine's key; the server records YOUR name and confines you to the module the token was minted for." }
                },
                "required": ["agent_name", "role"]
            }
        },
        {
            "name": "plan_feature",
            "description": "MANAGER. Create a feature and its entire Stage → Module → Task tree \
                in one atomic call — a half-written plan is never visible to workers. Stages \
                run strictly in order (the server enforces the gate); modules within a stage \
                run concurrently, one worker each; tasks are the module's checklist.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "description": { "type": "string" },
                    "stages": {
                        "type": "array",
                        "description": "In pipeline order.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string" },
                                "modules": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "name": { "type": "string" },
                                            "description": { "type": "string" },
                                            "tasks": { "type": "array", "items": { "type": "string" } }
                                        },
                                        "required": ["name"]
                                    }
                                }
                            },
                            "required": ["name", "modules"]
                        }
                    }
                },
                "required": ["name", "stages"]
            }
        },
        {
            "name": "revise_plan",
            "description": "MANAGER. Batch plan surgery, applied atomically or not at all. The \
                server refuses ops that rewrite history: removing anything claimed or \
                completed, or adding modules behind a gate that already opened.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "feature_id": { "type": "string", "description": "Feature id (`feat_...`), from get_context, plan_feature, or promote_wants." },
                    "ops": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "op": { "type": "string", "enum": ["add_stage", "add_module", "add_task", "rename", "remove"] },
                                "name": { "type": "string" },
                                "after": { "type": "string", "description": "add_stage: stage id to insert after (omit to append)." },
                                "stage_id": { "type": "string", "description": "add_module: target stage." },
                                "module_id": { "type": "string", "description": "add_task: target module." },
                                "description": { "type": "string" },
                                "tasks": { "type": "array", "items": { "type": "string" } },
                                "note": { "type": "string" },
                                "id": { "type": "string", "description": "rename/remove: the tree id (feat_/stg_/mod_/tsk_)." }
                            },
                            "required": ["op"]
                        }
                    }
                },
                "required": ["feature_id", "ops"]
            }
        },
        {
            "name": "complete_feature",
            "description": "MANAGER. Close the feature. Refuses (STAGES_INCOMPLETE) while any \
                module is unfinished. The summary is committed as a feature-scope memory.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "feature_id": { "type": "string", "description": "Feature id (`feat_...`), from get_context, plan_feature, or promote_wants." },
                    "summary": { "type": "string", "description": "What the feature delivered. Committed as a feature-scope memory." }
                },
                "required": ["feature_id", "summary"]
            }
        },
        {
            "name": "next_work",
            "description": "MANAGER. The dispatch decision, computed server-side: every module \
                that is ready right now (todo, unclaimed, stage unlocked), each with its \
                checklist — spawn one worker per entry; they can run concurrently. Never \
                compute stage gating yourself. An empty list with an unfinished feature \
                means wait: the response says what's in flight or blocked.",
            "inputSchema": {
                "type": "object",
                "properties": { "feature_id": { "type": "string", "description": "Feature id (`feat_...`), from get_context, plan_feature, or promote_wants." } },
                "required": ["feature_id"]
            }
        },
        {
            "name": "feature_status",
            "description": "The manager's poll: the feature's full tree with derived stage \
                locks, plus every event after your cursor — completions, blockers, premature \
                claims, discovered tasks, stage unlocks. Pass the returned events_cursor back \
                as events_since next time; nothing is missed between polls.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "feature_id": { "type": "string", "description": "Feature id (`feat_...`), from get_context, plan_feature, or promote_wants." },
                    "events_since": { "type": "integer", "description": "Event cursor from the previous poll (default 0 = from the beginning)." }
                },
                "required": ["feature_id"]
            }
        },
        {
            "name": "mint_worker",
            "description": "MANAGER. Mint one worker identity for one module, so a subagent \
                sharing your machine's key stops being recorded as the machine. Returns a \
                delegation_token: put it in the subagent's prompt and tell it to pass \
                delegation_token on get_context and on every write. The token is a worker \
                whatever key minted it, works only against that one module, only alongside \
                one key, and stops working when the module completes or is released. Mint \
                one per module you dispatch — minting again for the same module retires the \
                previous token.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "module_id": { "type": "string", "description": "The module this identity may work, from next_work." },
                    "agent_name": { "type": "string", "description": "The name the ledger will record for the subagent, e.g. 'agent.mod.schema'." },
                    "ttl_minutes": { "type": "integer", "description": "How long the token lives. Defaults to 4 hours; clamped to 5 minutes .. 24 hours." },
                    "for_key_id": { "type": "string", "description": "ONLY for a worker that runs somewhere else and holds its own key — a remote box, not a subagent of yours. The public id of that key (the `mcpm_<id>_…` middle, or the key_id issue_worker_key returned): the token is then honoured alongside THAT key instead of yours, and is inert on your machine. Omit for subagents sharing your key, which is what this tool is for." }
                },
                "required": ["module_id", "agent_name"]
            }
        },
        {
            "name": "issue_worker_key",
            "description": "MANAGER. Issue a standing worker key so a box that is not your \
                machine has an identity of its own. Use this when you dispatch to remote \
                boxes: without it every box shares one key, so the ledger cannot tell them \
                apart and any of them can complete another's module. Returns the token \
                ONCE — it cannot be recovered, so put it straight into that box's \
                environment. The key is always a WORKER (the role is not yours to choose), \
                one live key per agent_name, and the deployment is capped — reuse the key a \
                box already has rather than issuing per dispatch. For subagents that share \
                YOUR key, use mint_worker instead: a delegation token is scoped and \
                expiring where this is neither.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_name": { "type": "string", "description": "The name the ledger records for every write that box makes. Make it identify the box — the branch slug it runs, for instance. One live key per name." },
                    "label": { "type": "string", "description": "Human note for --list-keys and the console. Defaults to the agent name." }
                },
                "required": ["agent_name"]
            }
        },
        {
            "name": "claim_module",
            "description": "WORKER. Take the exclusive claim on your module — the gate check \
                happens here, in the same transaction. A legal claim returns your full \
                briefing: the checklist, memories from the stage and feature above you, and \
                completed-module summaries from earlier stages. On STAGE_LOCKED: do not \
                work; report to your manager and end your turn (the rejection is already on \
                the event record).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "module_id": { "type": "string", "description": "Module id (`mod_...`), from next_work or your claim briefing." },
                    "delegation_token": { "type": "string", "description": "Only if you are a subagent that was given one. Identifies you as the minted worker rather than as the machine's key; the server records YOUR name and confines you to the module the token was minted for." }
                },
                "required": ["module_id"]
            }
        },
        {
            "name": "complete_task",
            "description": "WORKER. Check off one checklist task as you finish it — not in a \
                batch at the end. Skipping requires a reason in note; the reason becomes part \
                of the record.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task_id": { "type": "string", "description": "Task id (`tsk_...`), from your claim briefing's checklist." },
                    "outcome": { "type": "string", "enum": ["done", "skipped"] },
                    "note": { "type": "string", "description": "REQUIRED when outcome is 'skipped': why it was not done. Becomes part of the record." },
                    "delegation_token": { "type": "string", "description": "Only if you are a subagent that was given one. Identifies you as the minted worker rather than as the machine's key; the server records YOUR name and confines you to the module the token was minted for." }
                },
                "required": ["task_id", "outcome"]
            }
        },
        {
            "name": "add_task",
            "description": "WORKER. Extend your own module's checklist when reality reveals \
                work the plan missed. The task lands with origin 'discovered' so the manager \
                can audit plan quality later.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "module_id": { "type": "string", "description": "Module id (`mod_...`), from next_work or your claim briefing." },
                    "name": { "type": "string" },
                    "note": { "type": "string", "description": "Why the plan missed this work." },
                    "delegation_token": { "type": "string", "description": "Only if you are a subagent that was given one. Identifies you as the minted worker rather than as the machine's key; the server records YOUR name and confines you to the module the token was minted for." }
                },
                "required": ["module_id", "name"]
            }
        },
        {
            "name": "complete_module",
            "description": "WORKER. The exit interview. Refuses (TASKS_OPEN) until every task \
                is done or skipped-with-reason. Your summary is committed as a module-scope \
                memory — it is what the next stage's workers read — and completing the \
                stage's last module unlocks the next stage automatically.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "module_id": { "type": "string", "description": "Module id (`mod_...`), from next_work or your claim briefing." },
                    "summary": { "type": "string", "description": "What you built, decisions made, interfaces exposed, gotchas." },
                    "delegation_token": { "type": "string", "description": "Only if you are a subagent that was given one. Identifies you as the minted worker rather than as the machine's key; the server records YOUR name and confines you to the module the token was minted for." }
                },
                "required": ["module_id", "summary"]
            }
        },
        {
            "name": "report_blocker",
            "description": "WORKER. You cannot proceed: flag the module blocked. Keeps your \
                claim, records the description as a memory, and raises blocker_reported for \
                the manager's next poll. Stop work after calling this.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "module_id": { "type": "string", "description": "Module id (`mod_...`), from next_work or your claim briefing." },
                    "description": { "type": "string", "description": "What is blocking you, concretely enough for the manager to act." },
                    "delegation_token": { "type": "string", "description": "Only if you are a subagent that was given one. Identifies you as the minted worker rather than as the machine's key; the server records YOUR name and confines you to the module the token was minted for." }
                },
                "required": ["module_id", "description"]
            }
        },
        {
            "name": "release_module",
            "description": "WORKER. Hand the module back unfinished with task states intact — \
                the honorable exit when you must stop. The next claimant inherits the \
                checklist plus everything committed to memory. Never exit silently.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "module_id": { "type": "string", "description": "Module id (`mod_...`), from next_work or your claim briefing." },
                    "reason": { "type": "string", "description": "Why you are handing the module back unfinished." },
                    "delegation_token": { "type": "string", "description": "Only if you are a subagent that was given one. Identifies you as the minted worker rather than as the machine's key; the server records YOUR name and confines you to the module the token was minted for." }
                },
                "required": ["module_id", "reason"]
            }
        },
        {
            "name": "commit_memory",
            "description": "ANY AGENT. Write to the project's knowledge base — the shared, \
                searchable record of what this crew knows. Pin it at the narrowest scope it \
                is actually true at: 'project' for anything that outlives one feature (a \
                convention, a tool choice, a standing gotcha), 'feature'/'stage'/'module'/'task' \
                for what is only true there. Scope is what makes it findable later: a \
                project-wide practice filed under one feature reads like a fact about that \
                feature and nobody finds it again. Attribution is automatic.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "scope": {
                        "type": "object",
                        "description": "Where this is true. Project scope needs no id.",
                        "properties": {
                            "level": {
                                "type": "string",
                                "enum": ["project", "feature", "stage", "module", "task"]
                            },
                            "id": { "type": "string", "description": "Omit for project level." }
                        },
                        "required": ["level"]
                    },
                    "kind": {
                        "type": "string",
                        "enum": ["convention", "decision", "gotcha", "outcome", "reference", "note"],
                        "description": "What this IS. convention = a standing rule for how we \
                            do something. decision = a choice and the reasoning that settled \
                            it. gotcha = a trap that looks fine and is not. outcome = what \
                            actually happened. reference = a pointer to a doc, ticket or \
                            dashboard. note = none of those. Pick the honest one: it is a \
                            filter every later reader uses."
                    },
                    "content": { "type": "string" },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "supersedes": {
                        "type": "array",
                        "description": "Memories this one replaces. THERE IS NO UPDATE TOOL: \
                            memories are immutable, and correcting one means committing the \
                            corrected version here with the old id listed. The old entry is \
                            never deleted — it drops out of default results and stays \
                            readable as history, because what the project used to believe is \
                            part of what it knows.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "memory_id": { "type": "string" },
                                "kind": {
                                    "type": "string",
                                    "enum": ["replaces", "refutes", "revises", "consolidates"],
                                    "description": "replaces = the fact changed (the world \
                                        moved). refutes = it was wrong when written (we were \
                                        wrong). revises = same fact, better words. \
                                        consolidates = several folded into one. The first two \
                                        are what 'what did we used to believe' returns, so \
                                        picking honestly is what makes history readable."
                                },
                                "rationale": {
                                    "type": "string",
                                    "description": "Why this replaces that. Rides the link, \
                                        and is often the only place the reason survives."
                                }
                            },
                            "required": ["memory_id"]
                        }
                    }
                },
                "required": ["scope", "content"]
            }
        },
        {
            "name": "search_memory",
            "description": "ANY AGENT. Query the project's knowledge base. Ask in your own \
                words — matching is fuzzy: terms are stemmed, expanded through a synonym \
                table, and matched against near-spellings, and a note answering three of \
                your four words still comes back (ranked below one answering all four). \
                Every other argument NARROWS. Unscoped it searches everything; scoped, \
                direction picks the slice: 'up' = anchor + ancestors + the project's \
                standing knowledge (what a worker should read before starting), 'down' = \
                anchor + descendants (what a manager reads to see what happened inside a \
                feature), 'here' = the anchor alone. Empty query = newest first in scope.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Free text. A question is fine." },
                    "kinds": {
                        "type": "array",
                        "description": "Restrict to these kinds. Omit for all of them.",
                        "items": {
                            "type": "string",
                            "enum": ["convention", "decision", "gotcha", "outcome", "reference", "note"]
                        }
                    },
                    "scope": {
                        "type": "object",
                        "properties": {
                            "level": {
                                "type": "string",
                                "enum": ["project", "feature", "stage", "module", "task"]
                            },
                            "id": { "type": "string", "description": "Omit for project level." }
                        },
                        "required": ["level"]
                    },
                    "direction": { "type": "string", "enum": ["here", "up", "down", "all"] },
                    "tags": {
                        "type": "array",
                        "description": "Every tag must be present (AND, not OR).",
                        "items": { "type": "string" }
                    },
                    "author": { "type": "string", "description": "Only this agent's writes." },
                    "since": { "type": "string", "description": "RFC 3339 instant; written at or after it." },
                    "until": { "type": "string", "description": "RFC 3339 instant; written before it." },
                    "include_superseded": {
                        "type": "boolean",
                        "description": "Include entries that have been replaced or are \
                            disputed. Off by default — you almost always want the current \
                            answer. On, this is how you read what the project used to think."
                    },
                    "limit": { "type": "integer" }
                }
            }
        },
        {
            "name": "touch_memory",
            "description": "ANY AGENT. Say which memories you actually USED. Bulk — pass \
                every id you leaned on in one call. This is the only thing that records use: \
                nothing is counted when a search merely returns something to you, because \
                that would measure what the ranker chose to show rather than what helped. \
                Touching keeps a memory ranking well and is how the crew learns which \
                knowledge is load-bearing, so be honest in both directions — touching \
                everything you were shown is the same as touching nothing.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "memory_ids": { "type": "array", "items": { "type": "string" } },
                    "note": { "type": "string", "description": "Optional: what you used it for." }
                },
                "required": ["memory_ids"]
            }
        },
        {
            "name": "confirm_memory",
            "description": "ANY AGENT. You checked this against reality and it holds. A \
                stronger claim than touch_memory — use it when you verified, not when you \
                merely relied on it. You cannot confirm a memory you wrote: corroboration \
                needs independence.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "memory_id": { "type": "string" },
                    "note": { "type": "string", "description": "How you checked." }
                },
                "required": ["memory_id"]
            }
        },
        {
            "name": "dispute_memory",
            "description": "ANY AGENT. This memory is wrong. It is NOT deleted — nothing here \
                ever is — but it drops out of default search results until the dispute is \
                resolved, and it stays readable as a record of what the project used to \
                believe. You cannot dispute your own memory: commit a corrected one with \
                supersedes[kind='refutes'] instead, which records what changed as well as \
                that something did.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "memory_id": { "type": "string" },
                    "reason": {
                        "type": "string",
                        "description": "Required. A dispute nobody can evaluate cannot be \
                            resolved, and it holds the memory out of circulation meanwhile."
                    }
                },
                "required": ["memory_id", "reason"]
            }
        },
        {
            "name": "relate_memories",
            "description": "ANY AGENT. Declare a standing relation between two existing \
                memories. This describes how they sit together and retires neither — \
                superseding is done at commit time through commit_memory's `supersedes`, \
                which is what keeps the history acyclic. Use it when you notice two entries \
                are connected and nothing says so: the graph grows by declared and confirmed \
                links, never by inference.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "from_memory_id": { "type": "string" },
                    "to_memory_id": { "type": "string" },
                    "kind": {
                        "type": "string",
                        "enum": ["refines", "depends_on", "contradicts", "relates_to"],
                        "description": "refines = FROM is a narrower case of TO. depends_on = \
                            FROM is only true because TO is (so changing TO puts FROM at \
                            risk). contradicts = they disagree and neither has won — a flag \
                            for a human, not a vote. relates_to = plain association, the \
                            weakest claim."
                    },
                    "rationale": {
                        "type": "string",
                        "description": "Why they are connected. Rides the link and is often \
                            the only place the reason survives."
                    }
                },
                "required": ["from_memory_id", "to_memory_id", "kind"]
            }
        },
        {
            "name": "memory_history",
            "description": "ANY AGENT. Everything the graph knows about one memory: what it \
                replaced and what replaced it, the standing relations declared on it, and \
                SUGGESTIONS — entries agents have repeatedly used in the same breath as this \
                one but that nobody has linked. Use it when a current rule looks arbitrary \
                (the reason usually lives in the edge, not in either entry), and act on a \
                suggestion by calling relate_memories if it is real.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "memory_id": { "type": "string" },
                    "belief_only": {
                        "type": "boolean",
                        "description": "Default true: show only the steps where the belief \
                            changed, collapsing rewordings. False gives every edit."
                    }
                },
                "required": ["memory_id"]
            }
        },
        {
            "name": "get_events",
            "description": "The raw append-only ledger, for orchestrators and debugging. \
                Managers normally get their events bundled into feature_status instead.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "feature_id": { "type": "string", "description": "Feature id (`feat_...`), from get_context, plan_feature, or promote_wants." },
                    "since": { "type": "integer", "description": "Return events with seq greater than this (default 0)." },
                    "limit": { "type": "integer" }
                }
            }
        }
    ])
}

/// The want pool: capture loose ideas, then compose them into features.
fn want_tools() -> Value {
    json!([
        {
            "name": "add_want",
            "description": "ANY AGENT. Capture one loose idea in the project's want pool — the \
                cheapest write in this API. A want is NOT a small feature: it is what someone \
                wanted, in whatever words they said it, with no structure demanded. Structure \
                comes later, when several wants are composed into a feature by promote_wants. \
                Capture ideas one per call, verbatim; do not merge or interpret them here.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "body": { "type": "string", "description": "The idea, as loosely as it was expressed." },
                    "tags": { "type": "array", "items": { "type": "string" }, "description": "Optional filing labels, e.g. 'ux', 'perf', 'billing'." }
                },
                "required": ["body"]
            }
        },
        {
            "name": "list_wants",
            "description": "ANY AGENT. Read the want pool — full-text search, tag and status \
                filters, plus pool-wide counts. This is the composition input: read the open \
                wants for THEMES, not one to one. Several wants usually belong in one feature, \
                and one want can inform several. Default status is 'open' (never promoted, not \
                declined).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Full-text query; omit for the whole pool, newest first." },
                    "status": { "type": "string", "enum": ["open", "promoted", "declined", "all"], "description": "Default 'open'." },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "limit": { "type": "integer" }
                }
            }
        },
        {
            "name": "update_want",
            "description": "ANY AGENT. Sharpen a want's wording, retag it, decline it with a \
                reason, or reopen a declined one. Declining requires a reason — a want dropped \
                without a why is just a lost idea, and the next agent re-proposes it. A want a \
                feature already absorbed is frozen (WANT_PROMOTED): its wording is quoted by a \
                plan, so add a new want instead of rewriting history.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "want_id": { "type": "string", "description": "Want id (`want_...`), from list_wants." },
                    "body": { "type": "string" },
                    "tags": { "type": "array", "items": { "type": "string" }, "description": "Replaces the tag list." },
                    "state": { "type": "string", "enum": ["open", "declined"], "description": "'declined' needs a reason; 'open' reopens a declined want." },
                    "reason": { "type": "string", "description": "REQUIRED when state is 'declined': why this idea was turned down. Stays on the record." }
                },
                "required": ["want_id"]
            }
        },
        {
            "name": "promote_wants",
            "description": "MANAGER. The composition step: turn a GROUP of loose wants into \
                real work. Give the wants plus either `plan` (compose them into a brand-new \
                feature, same shape as plan_feature) or `feature_id` (fold them into a feature \
                that already exists). Give each want a `rationale` saying how you read it into \
                the plan — that becomes the audit trail from idea to work. The feature, every \
                want link, and a feature-scope origin memory commit together, so a feature can \
                never forget which ideas it came from; workers read that memory when they \
                search_memory(direction='up'). A want may inform several features. Declined \
                wants are refused until reopened.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "wants": {
                        "type": "array",
                        "description": "The group being composed. Two or more is the normal case.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string" },
                                "rationale": { "type": "string", "description": "How this want was read into the plan, e.g. 'becomes the CSV export module in stage 2'." }
                            },
                            "required": ["id"]
                        }
                    },
                    "feature_id": { "type": "string", "description": "Fold into this existing feature. Mutually exclusive with `plan`." },
                    "plan": {
                        "type": "object",
                        "description": "Compose into a new feature. Mutually exclusive with `feature_id`.",
                        "properties": {
                            "name": { "type": "string" },
                            "description": { "type": "string" },
                            "stages": {
                                "type": "array",
                                "description": "In pipeline order.",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "name": { "type": "string" },
                                        "modules": {
                                            "type": "array",
                                            "items": {
                                                "type": "object",
                                                "properties": {
                                                    "name": { "type": "string" },
                                                    "description": { "type": "string" },
                                                    "tasks": { "type": "array", "items": { "type": "string" } }
                                                },
                                                "required": ["name"]
                                            }
                                        }
                                    },
                                    "required": ["name", "modules"]
                                }
                            }
                        },
                        "required": ["name", "stages"]
                    }
                },
                "required": ["wants"]
            }
        },
        {
            "name": "add_wants",
            "description": "ANY AGENT. Capture SEVERAL loose ideas in one atomic call — the \
                shape a brain-dump has. Each entry is its own want; nothing is merged. All or \
                nothing: one empty body rejects the batch rather than leaving half a list in \
                the pool.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "wants": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "body": { "type": "string", "description": "The idea, as loosely as it was expressed." },
                                "tags": { "type": "array", "items": { "type": "string" } }
                            },
                            "required": ["body"]
                        }
                    }
                },
                "required": ["wants"]
            }
        },
        {
            "name": "list_tags",
            "description": "ANY AGENT. The tag registry — every tag in the project with how \
                many wants carry it, most-used first. Read this BEFORE tagging a new want and \
                reuse an existing tag where one fits: tags are only useful when the same idea \
                files under the same word twice.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "create_tag",
            "description": "ANY AGENT. Register a tag with no want attached — a preset to file \
                later ideas under. Idempotent. Tagging a want with a new name creates it too, \
                so this is only needed to set up a vocabulary ahead of the ideas.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "label": { "type": "string", "description": "As you would type it; normalized to a lowercase slug (e.g. 'Field Reports' → 'field-reports')." }
                },
                "required": ["label"]
            }
        }
    ])
}