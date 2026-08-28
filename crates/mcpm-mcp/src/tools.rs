//! The 22 tool definitions (architecture doc §5). Descriptions are the
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
                    "role": { "type": "string", "enum": ["manager", "worker", "observer"], "description": "manager = owns a feature end to end; worker = owns one module." }
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
            "name": "claim_module",
            "description": "WORKER. Take the exclusive claim on your module — the gate check \
                happens here, in the same transaction. A legal claim returns your full \
                briefing: the checklist, memories from the stage and feature above you, and \
                completed-module summaries from earlier stages. On STAGE_LOCKED: do not \
                work; report to your manager and end your turn (the rejection is already on \
                the event record).",
            "inputSchema": {
                "type": "object",
                "properties": { "module_id": { "type": "string", "description": "Module id (`mod_...`), from next_work or your claim briefing." } },
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
                    "note": { "type": "string", "description": "REQUIRED when outcome is 'skipped': why it was not done. Becomes part of the record." }
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
                    "note": { "type": "string", "description": "Why the plan missed this work." }
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
                    "summary": { "type": "string", "description": "What you built, decisions made, interfaces exposed, gotchas." }
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
                    "description": { "type": "string", "description": "What is blocking you, concretely enough for the manager to act." }
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
                    "reason": { "type": "string", "description": "Why you are handing the module back unfinished." }
                },
                "required": ["module_id", "reason"]
            }
        },
        {
            "name": "commit_memory",
            "description": "ANY AGENT. Pin a searchable note to one node of the tree: \
                decisions, gotchas, interfaces, conventions. Workers commit at module scope; \
                managers commit conventions at feature scope. Attribution is automatic.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "scope": {
                        "type": "object",
                        "properties": {
                            "level": { "type": "string", "enum": ["task", "module", "stage", "feature"] },
                            "id": { "type": "string" }
                        },
                        "required": ["level", "id"]
                    },
                    "content": { "type": "string" },
                    "tags": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["scope", "content"]
            }
        },
        {
            "name": "search_memory",
            "description": "ANY AGENT. Full-text search over memories. Unscoped it searches \
                the whole project. Scoped, direction picks the slice: 'up' = anchor + \
                ancestors (a worker reading the conventions above its module), 'down' = \
                anchor + descendants (a manager reading everything that happened inside its \
                feature), 'here' = the anchor only. Empty query = newest first in scope.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "scope": {
                        "type": "object",
                        "properties": {
                            "level": { "type": "string", "enum": ["task", "module", "stage", "feature"] },
                            "id": { "type": "string" }
                        },
                        "required": ["level", "id"]
                    },
                    "direction": { "type": "string", "enum": ["here", "up", "down", "all"] },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "limit": { "type": "integer" }
                }
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