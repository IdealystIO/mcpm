//! The roadmap: long-horizon intent, and the two ship doors it gates.
//!
//! Everything below the roadmap answers "is this built?". The roadmap
//! answers "may it go out, and what is it for?" — and the second half
//! is the reason it exists at all. An item's `intent` paragraph travels
//! into every agent's `get_context` and into every claim briefing, so a
//! worker choosing a shape today can account for work nobody has
//! planned yet without reading a single other feature.
//!
//! Three invariants, each in one place, each inside its transaction:
//!
//! - **A feature ships through [`Store::release_feature`]**, and that
//!   is refused while any item its own item waits on is unshipped
//!   (`ROADMAP_LOCKED`). Completing the work is never refused — that is
//!   what makes building ahead of the frontier possible, which is the
//!   whole point of planning against a roadmap.
//! - **An item ships through [`Store::ship_roadmap_item`]**, refused
//!   until every feature bound to it is released and every item it
//!   waits on has shipped. So the ladder is: modules prove the feature,
//!   features prove the item, items gate each other.
//! - **A `hard` edge, and only a hard edge, holds WORK.** It is checked
//!   in `claim_module` beside the module gate and produces the same
//!   `PREREQS_OPEN` refusal, so a worker needs no new reaction for it.
//!
//! State is derived here at read time ([`derive_state`]) from the edges
//! and the bound features. The single held fact is `shipped_at`,
//! because an item can be satisfied by something outside the work tree
//! and an item with no features would otherwise hold its dependents
//! shut for good.

use serde_json::json;
use sqlx::Row;
use std::collections::{BTreeMap, BTreeSet};

use crate::error::{ErrorCode, McpmError};
use crate::ids::new_roadmap_id;
use crate::store::{excerpt, plan_invalid, record_event, topo_order, GraphNode, Store, Tx};
use crate::types::*;

type Result<T> = std::result::Result<T, McpmError>;

/// How much of a shipped item's intent still reaches an agent's
/// context. Shipped items are context ("this already exists, do not
/// rebuild it"), not direction, so they come as one line while
/// everything unshipped comes whole.
const SHIPPED_EXCERPT: usize = 200;

// ---------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------

impl Store {
    /// The whole roadmap in topological order, with each item's state
    /// derived, plus every feature bound to no item at all.
    pub async fn roadmap(&self) -> Result<Roadmap> {
        let items = sqlx::query(
            "SELECT id, name, intent, vision, horizon, position, shelved,
                    shipped_at, shipped_by, created_by, created_at
             FROM roadmap_items ORDER BY position, name",
        )
        .fetch_all(&self.pool)
        .await?;
        let deps = sqlx::query("SELECT item_id, depends_on, hard FROM roadmap_deps")
            .fetch_all(&self.pool)
            .await?;
        let feats = sqlx::query(
            "SELECT f.id, f.name, f.status, f.roadmap_item_id,
                    (f.released_at IS NOT NULL) AS released,
                    (SELECT COUNT(*) FROM modules m WHERE m.feature_id = f.id) AS modules_total,
                    (SELECT COUNT(*) FROM modules m
                       WHERE m.feature_id = f.id AND m.status = 'done') AS modules_done
             FROM features f ORDER BY f.created_at",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut by_item: BTreeMap<String, Vec<RoadmapFeatureRef>> = BTreeMap::new();
        let mut loose_features: Vec<RoadmapFeatureRef> = Vec::new();
        for r in &feats {
            let f = RoadmapFeatureRef {
                id: r.get("id"),
                name: r.get("name"),
                status: r.get("status"),
                released: r.get("released"),
                modules_done: r.get("modules_done"),
                modules_total: r.get("modules_total"),
            };
            match r.get::<Option<String>, _>("roadmap_item_id") {
                Some(item) => by_item.entry(item).or_default().push(f),
                None => loose_features.push(f),
            }
        }

        let name_of: BTreeMap<String, String> = items
            .iter()
            .map(|r| (r.get("id"), r.get("name")))
            .collect();
        let shipped: BTreeSet<String> = items
            .iter()
            .filter(|r| r.get::<Option<chrono::DateTime<chrono::Utc>>, _>("shipped_at").is_some())
            .map(|r| r.get("id"))
            .collect();
        let shelved: BTreeSet<String> = items
            .iter()
            .filter(|r| r.get::<bool, _>("shelved"))
            .map(|r| r.get("id"))
            .collect();

        // Edges both ways: an item's prerequisites, and what it unlocks.
        let mut upstream: BTreeMap<String, Vec<RoadmapEdge>> = BTreeMap::new();
        let mut downstream: BTreeMap<String, Vec<RoadmapEdge>> = BTreeMap::new();
        for d in &deps {
            let item_id: String = d.get("item_id");
            let dep_id: String = d.get("depends_on");
            let hard: bool = d.get("hard");
            upstream.entry(item_id.clone()).or_default().push(RoadmapEdge {
                name: name_of.get(&dep_id).cloned().unwrap_or_default(),
                shipped: shipped.contains(&dep_id),
                item_id: dep_id.clone(),
                hard,
            });
            downstream.entry(dep_id).or_default().push(RoadmapEdge {
                name: name_of.get(&item_id).cloned().unwrap_or_default(),
                shipped: shipped.contains(&item_id),
                item_id,
                hard,
            });
        }
        for v in upstream.values_mut().chain(downstream.values_mut()) {
            v.sort_by(|a, b| a.name.cmp(&b.name));
        }

        // Depth, and the display order, come from the graph — the same
        // way a module's column does. A cycle cannot exist here (both
        // writers refuse one), so an Err falls back to input order
        // rather than failing a read.
        let nodes: Vec<GraphNode> = items
            .iter()
            .map(|r| {
                let id: String = r.get("id");
                GraphNode {
                    deps: upstream
                        .get(&id)
                        .map(|e| e.iter().map(|x| x.item_id.clone()).collect())
                        .unwrap_or_default(),
                    key: r.get("name"),
                    id,
                }
            })
            .collect();
        let order = topo_order(&nodes).unwrap_or_else(|_| {
            nodes.iter().map(|n| (n.id.clone(), 1)).collect()
        });
        let depth: BTreeMap<&str, i32> = order.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        let rank: BTreeMap<&str, usize> =
            order.iter().enumerate().map(|(i, (id, _))| (id.as_str(), i)).collect();

        let mut out: Vec<RoadmapItemView> = items
            .iter()
            .map(|r| {
                let id: String = r.get("id");
                let ups = upstream.get(&id).cloned().unwrap_or_default();
                let features = by_item.get(&id).cloned().unwrap_or_default();
                let shipped_at: Option<chrono::DateTime<chrono::Utc>> = r.get("shipped_at");
                let is_shelved: bool = r.get("shelved");
                RoadmapItemView {
                    state: derive_state(shipped_at.is_some(), is_shelved, &ups, &shelved, &features),
                    depth: depth.get(id.as_str()).copied().unwrap_or(1),
                    depends_on: ups,
                    unlocks: downstream.get(&id).cloned().unwrap_or_default(),
                    features,
                    name: r.get("name"),
                    intent: r.get("intent"),
                    vision: r.get("vision"),
                    horizon: r.get("horizon"),
                    position: r.get("position"),
                    shelved: is_shelved,
                    shipped_at,
                    shipped_by: r.get("shipped_by"),
                    created_by: r.get("created_by"),
                    created_at: r.get("created_at"),
                    id,
                }
            })
            .collect();
        out.sort_by_key(|i| rank.get(i.id.as_str()).copied().unwrap_or(usize::MAX));
        Ok(Roadmap { items: out, loose_features })
    }

    /// The roadmap as one line per unshelved item — what every agent
    /// gets on `get_context`.
    pub async fn roadmap_digest(&self) -> Result<Vec<RoadmapLine>> {
        Ok(self
            .roadmap()
            .await?
            .items
            .iter()
            .filter(|i| !i.shelved)
            .map(line_of)
            .collect())
    }

    /// Where a feature sits on the roadmap, for its workers' briefings.
    /// `None` when the feature is loose, which is the normal case.
    pub async fn roadmap_context(&self, feature_id: &str) -> Result<Option<RoadmapContext>> {
        let bound: Option<String> =
            sqlx::query_scalar("SELECT roadmap_item_id FROM features WHERE id = $1")
                .bind(feature_id)
                .fetch_optional(&self.pool)
                .await?
                .flatten();
        let Some(item_id) = bound else { return Ok(None) };
        let road = self.roadmap().await?;
        let by_id: BTreeMap<&str, &RoadmapItemView> =
            road.items.iter().map(|i| (i.id.as_str(), i)).collect();
        let Some(item) = by_id.get(item_id.as_str()) else { return Ok(None) };

        let waiting_on: Vec<RoadmapLine> = item
            .depends_on
            .iter()
            .filter(|e| !e.shipped)
            .filter_map(|e| by_id.get(e.item_id.as_str()).map(|i| line_of(i)))
            .collect();
        let unlocks: Vec<RoadmapLine> = item
            .unlocks
            .iter()
            .filter_map(|e| by_id.get(e.item_id.as_str()))
            .filter(|i| !i.shelved && i.shipped_at.is_none())
            .map(|i| line_of(i))
            .collect();

        // The guidance is the point of carrying any of this. A worker
        // already knows what it is building; what it does not know is
        // which decisions somebody downstream will have to live with.
        let mut guidance = format!(
            "This feature is part of '{}' on the roadmap: {}",
            item.name,
            one_line(&item.intent)
        );
        if unlocks.is_empty() {
            guidance.push_str(
                " Nothing on the roadmap waits on it, so optimize for the work in front of you.",
            );
        } else {
            guidance.push_str(&format!(
                " {} roadmap item(s) wait on it: {}. You are NOT building those — do not add \
                 abstraction for them, do not widen scope. Read them once and only avoid the \
                 choices that would make them expensive: a value hardcoded where they need a \
                 column, a shape that assumes one of a thing they need many of. Where you do \
                 leave such a seam, say so in your handoff.",
                unlocks.len(),
                unlocks
                    .iter()
                    .map(|l| l.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !waiting_on.is_empty() {
            guidance.push_str(&format!(
                " Note that '{}' cannot SHIP until {} — build and complete normally; release is \
                 held until then.",
                item.name,
                waiting_on
                    .iter()
                    .map(|l| l.name.as_str())
                    .collect::<Vec<_>>()
                    .join(" and ")
            ));
        }

        Ok(Some(RoadmapContext {
            item: line_of(item),
            vision: (!item.vision.trim().is_empty()).then(|| item.vision.clone()),
            waiting_on,
            unlocks,
            guidance,
        }))
    }

    /// One item by id, for `read_roadmap_item` and the console drawer.
    pub async fn roadmap_item(&self, item_id: &str) -> Result<RoadmapItemView> {
        self.roadmap()
            .await?
            .items
            .into_iter()
            .find(|i| i.id == item_id)
            .ok_or_else(|| McpmError::not_found("roadmap item", item_id))
    }
}

/// An item's state, from its edges and the features bound to it.
///
/// Ordering matters: `shelved` and `shipped` are facts about the item
/// itself and win; `held` outranks anything about the work, because a
/// feature being finished does not matter while a prerequisite is out;
/// and `ready` is the one that asks for an action — everything bound to
/// it is released, so somebody owes it a `ship_roadmap_item`.
fn derive_state(
    shipped: bool,
    shelved: bool,
    deps: &[RoadmapEdge],
    shelved_items: &BTreeSet<String>,
    features: &[RoadmapFeatureRef],
) -> String {
    if shelved {
        return "shelved".into();
    }
    if shipped {
        return "shipped".into();
    }
    // A shelved prerequisite holds nothing: shelving is how a planner
    // says "this is out of the picture", and an edge to it that still
    // held would be a roadmap nobody can unstick.
    if deps
        .iter()
        .any(|d| !d.shipped && !shelved_items.contains(&d.item_id))
    {
        return "held".into();
    }
    let live: Vec<&RoadmapFeatureRef> =
        features.iter().filter(|f| f.status != "shelved").collect();
    if live.is_empty() {
        return "future".into();
    }
    if live.iter().all(|f| f.released) {
        return "ready".into();
    }
    "active".into()
}

fn line_of(i: &RoadmapItemView) -> RoadmapLine {
    RoadmapLine {
        id: i.id.clone(),
        name: i.name.clone(),
        horizon: i.horizon.clone(),
        state: i.state.clone(),
        intent: if i.shipped_at.is_some() {
            excerpt(&i.intent, SHIPPED_EXCERPT)
        } else {
            i.intent.clone()
        },
        features: i.features.len() as i64,
    }
}

fn one_line(intent: &str) -> String {
    let t = intent.trim();
    if t.is_empty() {
        "(no intent written)".into()
    } else {
        t.replace('\n', " ")
    }
}

// ---------------------------------------------------------------------
// Writes
// ---------------------------------------------------------------------

impl Store {
    /// State a roadmap whole: items, their intent, and the edges
    /// between them. Validated whole and refused whole — an unknown
    /// prerequisite, a self-dependency or a cycle writes nothing.
    ///
    /// An item whose `name` already exists is UPDATED in place rather
    /// than duplicated, and its edges are added to rather than
    /// replaced: a roadmap gets re-stated far more often than it gets
    /// built from nothing, and a planner re-sending last month's
    /// roadmap with one item added should not have to diff it.
    /// Dropping an edge is `revise_roadmap`'s job, where saying so is
    /// deliberate.
    pub async fn plan_roadmap(&self, agent: &str, plan: PlanRoadmap) -> Result<Roadmap> {
        if plan.items.is_empty() {
            return Err(plan_invalid(
                "plan_roadmap needs at least one item. An item is one shippable capability: a \
                 name, and one paragraph of intent in product terms.",
            ));
        }
        for i in &plan.items {
            if i.name.trim().is_empty() {
                return Err(plan_invalid("Every roadmap item needs a non-empty name."));
            }
        }
        let mut seen = BTreeSet::new();
        for i in &plan.items {
            if !seen.insert(i.name.trim()) {
                return Err(plan_invalid(format!(
                    "Roadmap item name '{}' appears twice — names are unique, and depends_on \
                     refers to them.",
                    i.name.trim()
                )));
            }
        }

        let mut tx = self.pool.begin().await?;
        // Serialize roadmap writes against each other: the cycle check
        // below reads the whole graph, so two planners racing could
        // each see an acyclic graph and commit an edge that closes one.
        sqlx::query("LOCK TABLE roadmap_items IN SHARE ROW EXCLUSIVE MODE")
            .execute(&mut *tx)
            .await?;

        let existing = sqlx::query("SELECT id, name FROM roadmap_items")
            .fetch_all(&mut *tx)
            .await?;
        let mut id_of_name: BTreeMap<String, String> = existing
            .iter()
            .map(|r| (r.get::<String, _>("name"), r.get::<String, _>("id")))
            .collect();

        // Resolve every plan name to an id first — an edge may name an
        // item that appears later in the list, or one already stored.
        for i in &plan.items {
            let name = i.name.trim().to_string();
            id_of_name
                .entry(name)
                .or_insert_with(new_roadmap_id);
        }
        for i in &plan.items {
            for d in i.depends_on.iter().chain(i.hard_depends_on.iter()) {
                let d = d.trim();
                if d == i.name.trim() {
                    return Err(plan_invalid(format!(
                        "Roadmap item '{}' depends on itself.",
                        i.name.trim()
                    )));
                }
                if !id_of_name.contains_key(d) {
                    return Err(plan_invalid(format!(
                        "Roadmap item '{}' depends on '{d}', which is neither in this plan nor \
                         already on the roadmap. depends_on names items by name.",
                        i.name.trim()
                    )));
                }
            }
        }

        // Cycle check over the MERGED graph — stored edges plus the
        // ones this plan adds — because a plan that is acyclic by
        // itself can still close a loop through what is already there.
        let stored = sqlx::query("SELECT item_id, depends_on FROM roadmap_deps")
            .fetch_all(&mut *tx)
            .await?;
        let mut edges: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for r in &stored {
            edges
                .entry(r.get("item_id"))
                .or_default()
                .insert(r.get("depends_on"));
        }
        for i in &plan.items {
            let id = id_of_name[i.name.trim()].clone();
            for d in i.depends_on.iter().chain(i.hard_depends_on.iter()) {
                edges
                    .entry(id.clone())
                    .or_default()
                    .insert(id_of_name[d.trim()].clone());
            }
        }
        let name_of: BTreeMap<String, String> =
            id_of_name.iter().map(|(n, i)| (i.clone(), n.clone())).collect();
        let nodes: Vec<GraphNode> = id_of_name
            .values()
            .map(|id| GraphNode {
                id: id.clone(),
                key: name_of.get(id).cloned().unwrap_or_default(),
                deps: edges.get(id).map(|s| s.iter().cloned().collect()).unwrap_or_default(),
            })
            .collect();
        if let Err(cycle) = topo_order(&nodes) {
            return Err(plan_invalid(format!(
                "The roadmap has a dependency cycle: {}. An item cannot wait on something that \
                 waits on it.",
                cycle
                    .iter()
                    .map(|id| name_of.get(id).cloned().unwrap_or_else(|| id.clone()))
                    .collect::<Vec<_>>()
                    .join(" -> ")
            )));
        }

        let mut written: Vec<String> = Vec::new();
        for (n, i) in plan.items.iter().enumerate() {
            let name = i.name.trim();
            let id = &id_of_name[name];
            sqlx::query(
                "INSERT INTO roadmap_items (id, name, intent, vision, horizon, position, created_by)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)
                 ON CONFLICT (name) DO UPDATE SET
                     intent  = CASE WHEN EXCLUDED.intent  = '' THEN roadmap_items.intent  ELSE EXCLUDED.intent  END,
                     vision  = CASE WHEN EXCLUDED.vision  = '' THEN roadmap_items.vision  ELSE EXCLUDED.vision  END,
                     horizon = CASE WHEN EXCLUDED.horizon = '' THEN roadmap_items.horizon ELSE EXCLUDED.horizon END",
            )
            .bind(id)
            .bind(name)
            .bind(i.intent.trim())
            .bind(i.vision.trim())
            .bind(i.horizon.trim())
            .bind(n as i32)
            .bind(agent)
            .execute(&mut *tx)
            .await?;
            written.push(name.to_string());
        }
        let mut hard: BTreeSet<(String, String)> = BTreeSet::new();
        for i in &plan.items {
            let from = id_of_name[i.name.trim()].clone();
            for d in &i.hard_depends_on {
                hard.insert((from.clone(), id_of_name[d.trim()].clone()));
            }
        }
        for i in &plan.items {
            let from = &id_of_name[i.name.trim()];
            for d in i.depends_on.iter().chain(i.hard_depends_on.iter()) {
                let to = &id_of_name[d.trim()];
                let is_hard = hard.contains(&(from.clone(), to.clone()));
                sqlx::query(
                    "INSERT INTO roadmap_deps (item_id, depends_on, hard) VALUES ($1, $2, $3)
                     ON CONFLICT (item_id, depends_on) DO UPDATE SET hard = roadmap_deps.hard OR EXCLUDED.hard",
                )
                .bind(from)
                .bind(to)
                .bind(is_hard)
                .execute(&mut *tx)
                .await?;
            }
        }
        record_event(
            &mut tx,
            "roadmap_planned",
            None,
            None,
            Some(agent),
            json!({ "items": written }),
        )
        .await?;
        tx.commit().await?;
        self.roadmap().await
    }

    /// Surgical roadmap changes, applied atomically or not at all.
    pub async fn revise_roadmap(&self, agent: &str, ops: Vec<RoadmapOp>) -> Result<Roadmap> {
        if ops.is_empty() {
            return Err(plan_invalid("revise_roadmap needs at least one operation."));
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("LOCK TABLE roadmap_items IN SHARE ROW EXCLUSIVE MODE")
            .execute(&mut *tx)
            .await?;
        let mut summaries: Vec<String> = Vec::new();
        for op in ops {
            summaries.push(apply_roadmap_op(&mut tx, agent, op).await?);
        }
        record_event(
            &mut tx,
            "roadmap_revised",
            None,
            None,
            Some(agent),
            json!({ "ops": summaries }),
        )
        .await?;
        tx.commit().await?;
        self.roadmap().await
    }

    /// Ship a feature: the second door, and the one the roadmap gates.
    ///
    /// `complete_feature` says the work landed. This says it is out.
    /// The two are separate so a feature can be built ahead of the
    /// frontier and held — which is the reason to plan against a
    /// roadmap at all — and so the roadmap can distinguish what is
    /// live from what is merely merged.
    pub async fn release_feature(&self, agent: &str, feature_id: &str, note: &str) -> Result<Ack> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT id, name, status, roadmap_item_id, released_at
             FROM features WHERE id = $1 FOR UPDATE",
        )
        .bind(feature_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| McpmError::not_found("feature", feature_id))?;
        let name: String = row.get("name");
        let status: String = row.get("status");
        let item_id: Option<String> = row.get("roadmap_item_id");
        let released: Option<chrono::DateTime<chrono::Utc>> = row.get("released_at");

        if released.is_some() {
            return Err(McpmError::new(
                ErrorCode::AlreadyDone,
                format!("Feature '{name}' has already been released."),
                json!({ "feature_id": feature_id, "released_at": released }),
                "Nothing to do. If the roadmap item it belongs to is now complete, \
                 ship_roadmap_item is the next door.",
            ));
        }
        if status != "done" {
            return Err(McpmError::new(
                ErrorCode::ModulesIncomplete,
                format!(
                    "Feature '{name}' is '{status}', not done — releasing is for work that has \
                     already landed."
                ),
                json!({ "feature_id": feature_id, "status": status }),
                "Finish the modules and call complete_feature first; release_feature is the \
                 second door, not a substitute for the first.",
            ));
        }
        if let Some(item_id) = &item_id {
            let blocking = unshipped_prereqs(&mut tx, item_id).await?;
            if !blocking.is_empty() {
                return Err(roadmap_locked(
                    format!(
                        "Feature '{name}' is bound to a roadmap item that waits on {} unshipped \
                         item(s): {}.",
                        blocking.len(),
                        blocking
                            .iter()
                            .map(|(_, n)| n.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    json!({
                        "feature_id": feature_id,
                        "roadmap_item": item_id,
                        "waiting_on": blocking
                            .iter()
                            .map(|(i, n)| json!({ "id": i, "name": n }))
                            .collect::<Vec<_>>(),
                    }),
                    "The work is fine and there is nothing here for you to finish — this is a \
                     hold, not a defect. Leave the feature done-and-unreleased, tell your \
                     operator which items it waits on, and release it after they ship.",
                ));
            }
        }
        sqlx::query("UPDATE features SET released_at = now(), released_by = $2 WHERE id = $1")
            .bind(feature_id)
            .bind(agent)
            .execute(&mut *tx)
            .await?;
        record_event(
            &mut tx,
            "feature_released",
            Some(feature_id),
            Some(feature_id),
            Some(agent),
            json!({ "feature": name, "note": note, "roadmap_item": item_id }),
        )
        .await?;
        tx.commit().await?;
        Ok(Ack::with(
            format!("Feature '{name}' is released."),
            json!({ "feature_id": feature_id, "roadmap_item": item_id }),
        ))
    }

    /// Ship a roadmap item: every feature bound to it is out, and so
    /// is everything it waited on.
    ///
    /// The one held fact on the roadmap, and an ACT rather than a
    /// derivation, because an item can be satisfied by something the
    /// work tree never saw — a migration, a contract, a vendor's
    /// release — and an item with no features would otherwise hold its
    /// dependents shut for good.
    pub async fn ship_roadmap_item(&self, agent: &str, item_id: &str, note: &str) -> Result<Ack> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT id, name, shelved, shipped_at FROM roadmap_items WHERE id = $1 FOR UPDATE",
        )
        .bind(item_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| McpmError::not_found("roadmap item", item_id))?;
        let name: String = row.get("name");
        if row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("shipped_at").is_some() {
            return Err(McpmError::new(
                ErrorCode::AlreadyDone,
                format!("Roadmap item '{name}' has already shipped."),
                json!({ "item_id": item_id }),
                "Nothing to do.",
            ));
        }
        if row.get::<bool, _>("shelved") {
            return Err(plan_invalid(format!(
                "Roadmap item '{name}' is shelved — it holds nothing and ships nothing. \
                 Unshelve it first if it is back in the picture."
            )));
        }

        let blocking = unshipped_prereqs(&mut tx, item_id).await?;
        if !blocking.is_empty() {
            return Err(roadmap_locked(
                format!(
                    "Roadmap item '{name}' waits on {} unshipped item(s): {}.",
                    blocking.len(),
                    blocking.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>().join(", ")
                ),
                json!({
                    "item_id": item_id,
                    "waiting_on": blocking.iter().map(|(i, n)| json!({ "id": i, "name": n })).collect::<Vec<_>>(),
                }),
                "Ship those items first. They are the roadmap's own order, not this item's work.",
            ));
        }
        let open = sqlx::query(
            "SELECT id, name, status FROM features
             WHERE roadmap_item_id = $1 AND status <> 'shelved' AND released_at IS NULL
             ORDER BY name",
        )
        .bind(item_id)
        .fetch_all(&mut *tx)
        .await?;
        if !open.is_empty() {
            let names: Vec<String> = open.iter().map(|r| r.get("name")).collect();
            return Err(roadmap_locked(
                format!(
                    "Roadmap item '{name}' has {} feature(s) that have not been released: {}.",
                    open.len(),
                    names.join(", ")
                ),
                json!({
                    "item_id": item_id,
                    "unreleased_features": open
                        .iter()
                        .map(|r| json!({
                            "id": r.get::<String, _>("id"),
                            "name": r.get::<String, _>("name"),
                            "status": r.get::<String, _>("status"),
                        }))
                        .collect::<Vec<_>>(),
                }),
                "Finish and release each listed feature (complete_feature, then release_feature) \
                 before shipping the item.",
            ));
        }

        sqlx::query(
            "UPDATE roadmap_items SET shipped_at = now(), shipped_by = $2 WHERE id = $1",
        )
        .bind(item_id)
        .bind(agent)
        .execute(&mut *tx)
        .await?;
        record_event(
            &mut tx,
            "roadmap_item_shipped",
            None,
            Some(item_id),
            Some(agent),
            json!({ "item": name, "note": note }),
        )
        .await?;

        // Whose last prerequisite was this? Same courtesy the module
        // graph pays with `module_unlocked`: the ledger says what just
        // became possible, so a manager polling it does not have to
        // re-derive the frontier.
        let freed = sqlx::query(
            "SELECT i.id, i.name FROM roadmap_deps d
             JOIN roadmap_items i ON i.id = d.item_id
             WHERE d.depends_on = $1 AND NOT i.shelved
               AND NOT EXISTS (
                   SELECT 1 FROM roadmap_deps d2
                   JOIN roadmap_items p ON p.id = d2.depends_on
                   WHERE d2.item_id = d.item_id AND p.shipped_at IS NULL AND NOT p.shelved)
             ORDER BY i.name",
        )
        .bind(item_id)
        .fetch_all(&mut *tx)
        .await?;
        for r in &freed {
            record_event(
                &mut tx,
                "roadmap_unlocked",
                None,
                Some(&r.get::<String, _>("id")),
                Some(agent),
                json!({ "item": r.get::<String, _>("name"), "by": name }),
            )
            .await?;
        }
        tx.commit().await?;
        let freed_names: Vec<String> = freed.iter().map(|r| r.get("name")).collect();
        Ok(Ack::with(
            if freed_names.is_empty() {
                format!("Roadmap item '{name}' has shipped.")
            } else {
                format!(
                    "Roadmap item '{name}' has shipped, and unblocks {}.",
                    freed_names.join(", ")
                )
            },
            json!({ "item_id": item_id, "unlocked": freed_names }),
        ))
    }
}

// ---------------------------------------------------------------------
// Helpers shared by the writers and the gate
// ---------------------------------------------------------------------

/// Every item `item_id` waits on that has not shipped. A SHELVED
/// prerequisite is not one: shelving is how a planner takes something
/// out of the picture, and an edge to a shelf that still held would be
/// a roadmap nobody can unstick.
pub(crate) async fn unshipped_prereqs(
    tx: &mut Tx<'_>,
    item_id: &str,
) -> Result<Vec<(String, String)>> {
    let rows = sqlx::query(
        "SELECT p.id, p.name FROM roadmap_deps d
         JOIN roadmap_items p ON p.id = d.depends_on
         WHERE d.item_id = $1 AND p.shipped_at IS NULL AND NOT p.shelved
         ORDER BY p.name",
    )
    .bind(item_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().map(|r| (r.get("id"), r.get("name"))).collect())
}

/// The HARD prerequisites holding a feature's modules shut. Empty for
/// a loose feature, and empty whenever every hard edge has shipped —
/// which is the normal case, because a hard edge is the exception.
pub(crate) async fn hard_block(
    tx: &mut Tx<'_>,
    feature_id: &str,
) -> Result<Vec<(String, String)>> {
    let rows = sqlx::query(
        "SELECT p.id, p.name FROM features f
         JOIN roadmap_deps d ON d.item_id = f.roadmap_item_id AND d.hard
         JOIN roadmap_items p ON p.id = d.depends_on
         WHERE f.id = $1 AND p.shipped_at IS NULL AND NOT p.shelved
         ORDER BY p.name",
    )
    .bind(feature_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().map(|r| (r.get("id"), r.get("name"))).collect())
}

pub(crate) fn roadmap_locked(
    message: impl Into<String>,
    data: serde_json::Value,
    hint: impl Into<String>,
) -> McpmError {
    McpmError::new(ErrorCode::RoadmapLocked, message, data, hint)
}

/// Whether `from` already reaches `to` through roadmap edges — the
/// cycle check for a single added edge. Adding `a -> b` closes a cycle
/// exactly when `b` already reaches `a`.
async fn roadmap_reaches(tx: &mut Tx<'_>, from: &str, to: &str) -> Result<bool> {
    let hit: Option<i32> = sqlx::query_scalar(
        "WITH RECURSIVE r AS (
             SELECT depends_on AS id FROM roadmap_deps WHERE item_id = $1
             UNION
             SELECT d.depends_on FROM roadmap_deps d JOIN r ON d.item_id = r.id
         )
         SELECT 1 FROM r WHERE id = $2 LIMIT 1",
    )
    .bind(from)
    .bind(to)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(hit.is_some())
}

async fn item_name(tx: &mut Tx<'_>, id: &str) -> Result<String> {
    sqlx::query_scalar("SELECT name FROM roadmap_items WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| McpmError::not_found("roadmap item", id))
}

async fn apply_roadmap_op(tx: &mut Tx<'_>, agent: &str, op: RoadmapOp) -> Result<String> {
    match op {
        RoadmapOp::AddItem { name, intent, vision, horizon } => {
            let name = name.trim().to_string();
            if name.is_empty() {
                return Err(plan_invalid("A roadmap item needs a non-empty name."));
            }
            let id = new_roadmap_id();
            let next: i32 = sqlx::query_scalar(
                "SELECT COALESCE(MAX(position), -1) + 1 FROM roadmap_items",
            )
            .fetch_one(&mut **tx)
            .await?;
            sqlx::query(
                "INSERT INTO roadmap_items (id, name, intent, vision, horizon, position, created_by)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(&id)
            .bind(&name)
            .bind(intent.trim())
            .bind(vision.trim())
            .bind(horizon.trim())
            .bind(next)
            .bind(agent)
            .execute(&mut **tx)
            .await
            .map_err(|e| duplicate_name(e, &name))?;
            Ok(format!("added roadmap item '{name}' ({id})"))
        }
        RoadmapOp::EditItem { item_id, name, intent, vision, horizon, position } => {
            let was = item_name(tx, &item_id).await?;
            sqlx::query(
                "UPDATE roadmap_items SET
                     name     = COALESCE($2, name),
                     intent   = COALESCE($3, intent),
                     vision   = COALESCE($4, vision),
                     horizon  = COALESCE($5, horizon),
                     position = COALESCE($6, position)
                 WHERE id = $1",
            )
            .bind(&item_id)
            .bind(name.as_deref().map(str::trim))
            .bind(intent.as_deref().map(str::trim))
            .bind(vision.as_deref().map(str::trim))
            .bind(horizon.as_deref().map(str::trim))
            .bind(position)
            .execute(&mut **tx)
            .await
            .map_err(|e| duplicate_name(e, name.as_deref().unwrap_or(&was)))?;
            Ok(format!("edited roadmap item '{was}' ({item_id})"))
        }
        RoadmapOp::RemoveItem { item_id } => {
            let name = item_name(tx, &item_id).await?;
            // A delete here would silently loosen real features (the FK
            // is ON DELETE SET NULL, which is right for a database and
            // wrong for a decision). Make the planner say so.
            let bound: Vec<String> = sqlx::query_scalar(
                "SELECT name FROM features WHERE roadmap_item_id = $1 ORDER BY name",
            )
            .bind(&item_id)
            .fetch_all(&mut **tx)
            .await?;
            if !bound.is_empty() {
                return Err(plan_invalid(format!(
                    "Roadmap item '{name}' still has {} feature(s) bound to it: {}. Removing it \
                     would silently loosen them — unbind each first (bind_feature with no \
                     item_id), or shelve the item instead of removing it.",
                    bound.len(),
                    bound.join(", ")
                )));
            }
            sqlx::query("DELETE FROM roadmap_items WHERE id = $1")
                .bind(&item_id)
                .execute(&mut **tx)
                .await?;
            Ok(format!("removed roadmap item '{name}' ({item_id})"))
        }
        RoadmapOp::ShelveItem { item_id, shelved } => {
            let name = item_name(tx, &item_id).await?;
            sqlx::query("UPDATE roadmap_items SET shelved = $2 WHERE id = $1")
                .bind(&item_id)
                .bind(shelved)
                .execute(&mut **tx)
                .await?;
            Ok(format!(
                "{} roadmap item '{name}' ({item_id})",
                if shelved { "shelved" } else { "unshelved" }
            ))
        }
        RoadmapOp::AddDependency { item_id, depends_on, hard } => {
            if item_id == depends_on {
                return Err(plan_invalid("A roadmap item cannot depend on itself."));
            }
            let a = item_name(tx, &item_id).await?;
            let b = item_name(tx, &depends_on).await?;
            if roadmap_reaches(tx, &depends_on, &item_id).await? {
                return Err(plan_invalid(format!(
                    "'{b}' already waits on '{a}' — adding this edge would close a cycle."
                )));
            }
            sqlx::query(
                "INSERT INTO roadmap_deps (item_id, depends_on, hard) VALUES ($1, $2, $3)
                 ON CONFLICT (item_id, depends_on) DO UPDATE SET hard = EXCLUDED.hard",
            )
            .bind(&item_id)
            .bind(&depends_on)
            .bind(hard)
            .execute(&mut **tx)
            .await?;
            Ok(format!(
                "'{a}' now waits on '{b}'{}",
                if hard { " (hard — it holds the work, not just the ship)" } else { "" }
            ))
        }
        RoadmapOp::RemoveDependency { item_id, depends_on } => {
            let a = item_name(tx, &item_id).await?;
            let b = item_name(tx, &depends_on).await?;
            sqlx::query("DELETE FROM roadmap_deps WHERE item_id = $1 AND depends_on = $2")
                .bind(&item_id)
                .bind(&depends_on)
                .execute(&mut **tx)
                .await?;
            Ok(format!("'{a}' no longer waits on '{b}'"))
        }
        RoadmapOp::BindFeature { feature_id, item_id } => {
            let feature: String = sqlx::query_scalar("SELECT name FROM features WHERE id = $1")
                .bind(&feature_id)
                .fetch_optional(&mut **tx)
                .await?
                .ok_or_else(|| McpmError::not_found("feature", &feature_id))?;
            match &item_id {
                Some(id) => {
                    let name = item_name(tx, id).await?;
                    let shelved: bool =
                        sqlx::query_scalar("SELECT shelved FROM roadmap_items WHERE id = $1")
                            .bind(id)
                            .fetch_one(&mut **tx)
                            .await?;
                    if shelved {
                        return Err(plan_invalid(format!(
                            "Roadmap item '{name}' is shelved — unshelve it before binding work \
                             to it."
                        )));
                    }
                    sqlx::query("UPDATE features SET roadmap_item_id = $2 WHERE id = $1")
                        .bind(&feature_id)
                        .bind(id)
                        .execute(&mut **tx)
                        .await?;
                    record_event(
                        tx,
                        "feature_bound",
                        Some(&feature_id),
                        Some(&feature_id),
                        Some(agent),
                        json!({ "feature": feature, "item": name, "item_id": id }),
                    )
                    .await?;
                    Ok(format!("bound feature '{feature}' to roadmap item '{name}'"))
                }
                None => {
                    sqlx::query("UPDATE features SET roadmap_item_id = NULL WHERE id = $1")
                        .bind(&feature_id)
                        .execute(&mut **tx)
                        .await?;
                    record_event(
                        tx,
                        "feature_bound",
                        Some(&feature_id),
                        Some(&feature_id),
                        Some(agent),
                        json!({ "feature": feature, "item": serde_json::Value::Null }),
                    )
                    .await?;
                    Ok(format!("loosened feature '{feature}' from the roadmap"))
                }
            }
        }
    }
}

fn duplicate_name(e: sqlx::Error, name: &str) -> McpmError {
    if let sqlx::Error::Database(db) = &e {
        if db.is_unique_violation() {
            return plan_invalid(format!(
                "A roadmap item named '{name}' already exists. Item names are unique — edit that \
                 one, or pick another name."
            ));
        }
    }
    McpmError::from(e)
}

/// The roadmap item a feature row is bound to, as an edge. `shipped`
/// on this edge is the ITEM's own state, not the feature's.
pub(crate) fn roadmap_edge(r: &sqlx::postgres::PgRow) -> Option<RoadmapEdge> {
    let item_id: Option<String> = r.get("roadmap_item_id");
    item_id.map(|item_id| RoadmapEdge {
        name: r.get::<Option<String>, _>("item_name").unwrap_or_default(),
        shipped: r.get::<Option<bool>, _>("item_shipped").unwrap_or(false),
        item_id,
        hard: false,
    })
}

/// The unshipped items standing between a feature and its release, as
/// the three parallel arrays the rollup query aggregates them into.
pub(crate) fn held_edges(r: &sqlx::postgres::PgRow) -> Vec<RoadmapEdge> {
    let ids: Vec<String> = r.get("held_ids");
    let names: Vec<String> = r.get("held_names");
    let hard: Vec<bool> = r.get("held_hard");
    ids.into_iter()
        .enumerate()
        .map(|(i, item_id)| RoadmapEdge {
            name: names.get(i).cloned().unwrap_or_default(),
            hard: hard.get(i).copied().unwrap_or(false),
            shipped: false,
            item_id,
        })
        .collect()
}

/// Resolve a roadmap item by id or by name, for callers that take one
/// from a person or from the digest. Names are unique, so there is no
/// ambiguity to resolve — only a refusal that has to say which of the
/// two spellings it tried.
pub(crate) async fn resolve_roadmap_item(tx: &mut Tx<'_>, key: &str) -> Result<String> {
    let found: Option<String> = sqlx::query_scalar(
        "SELECT id FROM roadmap_items WHERE id = $1 OR name = $1 LIMIT 1",
    )
    .bind(key)
    .fetch_optional(&mut **tx)
    .await?;
    found.ok_or_else(|| {
        plan_invalid(format!(
            "No roadmap item with id or name '{key}'. read_roadmap lists them; plan_roadmap \
             creates one. Omit roadmap_item entirely for a loose feature."
        ))
    })
}
