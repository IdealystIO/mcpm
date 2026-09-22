//! The roadmap screen: where the product is going, and the two ship
//! doors that order it.
//!
//! Everything else in this console answers "how is the work going".
//! This screen answers the question that shapes the work before it is
//! planned, and it is read far more often than it is edited — by
//! people here, and by every agent through `get_context`. So the card
//! is a reading surface: the item's name, its state, and the paragraph
//! of intent in full. Edges and the long form are properties and open
//! in the drawer (rule 20).
//!
//! One thing it deliberately does NOT do: draw the graph. A roadmap
//! has tens of items and paragraphs of text on each, so a board of
//! columns would be a wide surface whose cards are unreadable at the
//! width that fits them (rule 23's corollary). Depth is expressed as
//! "waits on" and "unlocks" chips, which is what a reader actually
//! asks of a roadmap edge.

use idea_ui::{tone, typography_kind, Badge, Button, IdeaThemeRef, Progress, ProgressCap, Spacer,
    Tag, Typography};
use runtime_core::{
    component, memo, rx, stylesheet, switch, ui, AlignItems, Cursor, Element, FlexDirection,
    FlexWrap, FontWeight, IdealystSchema, IntoElement, Position, StyleApplication,
};
use std::rc::Rc;

use crate::components::bits::{tappable, StatusDot};
use crate::components::drawer::{close_box_style, panel_motion, CloseGlyph};
use crate::components::edits::{ActionMenu, MenuEntry};
use crate::model::{self, RoadFeature, RoadState};
use crate::state::{Console, Edit};
use crate::styles::{status_tone, SectionLabel};

/// Props for [`RoadmapView`].
#[derive(Default, IdealystSchema)]
pub struct RoadmapViewProps {
    /// Console state handles.
    pub console: Console,
}

/// The roadmap screen.
///
/// Keyed on the SHAPE of the roadmap — which items there are, in which
/// horizons, and whether shelved ones are showing — never on their
/// data. A feature completing four levels down moves a card's state
/// tag and its progress bar without a node here being rebuilt.
#[component]
pub fn RoadmapView(props: &RoadmapViewProps) -> Element {
    let console = props.console;
    let data = console.data;

    let meta = rx!({
        let road = data.roadmap.get();
        let shipped = road
            .items
            .iter()
            .filter(|i| i.state == RoadState::Shipped)
            .count();
        let ready = road.ready_count();
        let loose = road.loose_features.len();
        let items = road.items.iter().filter(|i| !i.shelved).count();
        format!(
            "{items} item{} \u{b7} {shipped} shipped \u{b7} {ready} ready to ship \u{b7} \
             {loose} loose feature{}",
            if items == 1 { "" } else { "s" },
            if loose == 1 { "" } else { "s" },
        )
    });
    let new_item: Rc<dyn Fn()> =
        Rc::new(move || console.open_edit(Edit::EditRoadmapItem { item: String::new() }));

    let body = switch(
        move || {
            let road = data.roadmap.get();
            let shelved = console.road_shelved.get();
            // The shape: the horizons, and the ids under each. Not the
            // items themselves — a poll that ships something moves the
            // tag inside a card that stays mounted.
            let groups: Vec<(String, Vec<String>)> = road
                .by_horizon(false)
                .into_iter()
                .map(|(h, idx)| (h, idx.into_iter().map(|i| road.items[i].id.clone()).collect()))
                .collect();
            let shelf: Vec<String> = if shelved {
                road.by_horizon(true).into_iter().flat_map(|(_, i)| i).map(|i| road.items[i].id.clone()).collect()
            } else {
                Vec::new()
            };
            let loose: Vec<String> = road.loose_features.iter().map(|f| f.id.clone()).collect();
            (groups, shelf, loose, road.items.is_empty())
        },
        move |state: &(Vec<(String, Vec<String>)>, Vec<String>, Vec<String>, bool)| {
            let (groups, shelf, loose, empty) = state.clone();
            let group_count = groups.len();
            let shelf_count = shelf.len();
            let loose_count = loose.len();
            ui! {
                view(style = RoadmapBody()) {
                    if empty {
                        BlankRoadmap(console = console)
                    }
                    for g in 0..group_count {
                        HorizonGroup(
                            console = console,
                            label = groups[g].0.clone(),
                            items = groups[g].1.clone(),
                        )
                    }
                    if shelf_count > 0 {
                        HorizonGroup(
                            console = console,
                            label = "Shelved".to_string(),
                            items = shelf.clone(),
                        )
                    }
                    if loose_count > 0 {
                        view(style = HorizonBlock()) {
                            text(style = SectionLabel()) { "Loose features" }
                            view(style = SurfaceCard()) {
                                for i in 0..loose_count {
                                    LooseRow(console = console, feature = loose[i].clone())
                                }
                            }
                        }
                    }
                }
            }
        },
    );

    ui! {
        view(style = RoadmapScreen()) {
            view(style = ScreenHead()) {
                view(style = HeadText()) {
                    Typography(
                        content = "Roadmap",
                        kind = typography_kind::H2,
                        weight = Some(FontWeight::SemiBold),
                    )
                    Typography(content = meta, kind = typography_kind::BodySm, muted = true)
                }
                Spacer()
                ShelfToggle(console = console)
                Button(label = "New item", on_click = new_item)
            }
            scroll_view(style = RoadmapScroll()) {
                body
            }
        }
    }
}

/// Props for [`HorizonGroup`].
#[derive(Default, IdealystSchema)]
pub struct HorizonGroupProps {
    /// Console state handles.
    pub console: Console,
    /// The horizon's label — display only; it orders nothing.
    pub label: String,
    /// The item ids under it, in the roadmap's own order.
    pub items: Vec<String>,
}

/// One horizon's worth of cards.
#[component]
pub fn HorizonGroup(props: &HorizonGroupProps) -> Element {
    let console = props.console;
    let label = props.label.clone();
    let items = props.items.clone();
    let n = items.len();
    ui! {
        view(style = HorizonBlock()) {
            text(style = SectionLabel()) { label }
            for i in 0..n {
                RoadmapCard(console = console, item = items[i].clone())
            }
        }
    }
}

/// Props for [`ShelfToggle`].
#[derive(Default, IdealystSchema)]
pub struct ShelfToggleProps {
    /// Console state handles.
    pub console: Console,
}

/// Show shelved items too. A chip rather than a tab: shelved is a
/// second set to include, not a different screen (rule 28).
#[component]
pub fn ShelfToggle(props: &ShelfToggleProps) -> Element {
    let console = props.console;
    let label = rx!(if console.road_shelved.get() { "Hide shelved" } else { "Show shelved" }
        .to_string());
    tappable(
        vec![ui! { text(style = ToggleLabel()) { label } }],
        move || console.road_shelved.update(|v| !v),
    )
    .with_style(StyleApplication::new(toggle_box_style()))
    .into_element()
}

/// Props for [`RoadmapCard`].
#[derive(Default, IdealystSchema)]
pub struct RoadmapCardProps {
    /// Console state handles.
    pub console: Console,
    /// The item's id. An id and not an index: the roadmap re-sorts
    /// under a poll, and an index would slide onto another item.
    pub item: String,
}

/// One item. Everything inside reads the item live off `Data::roadmap`,
/// so a state change moves a tag and a bar in place.
#[component]
pub fn RoadmapCard(props: &RoadmapCardProps) -> Element {
    let console = props.console;
    let data = console.data;
    let id = props.item.clone();

    let item = {
        let id = id.clone();
        move || data.roadmap.get().item(&id).cloned()
    };
    let name = {
        let item = item.clone();
        rx!(item().map(|i| i.name.clone()).unwrap_or_default())
    };
    let intent = {
        let item = item.clone();
        rx!({
            let text = item().map(|i| i.intent.clone()).unwrap_or_default();
            if text.trim().is_empty() {
                // Rule 19 keeps mechanism off the screen; this is the
                // opposite — an instruction to the one person who can
                // act on it, on the item that needs it.
                "No intent written. An item without one teaches nobody anything.".to_string()
            } else {
                text
            }
        })
    };
    // Read the ROOT signal in each reading rather than chaining off a
    // memo built in this same body — a memo's first compute is a
    // staged write, so a dependent built beside it reads the empty
    // value and the debug runtime warns `staged-read`.
    let road_state = {
        let id = id.clone();
        move || {
            data.roadmap
                .get()
                .item(&id)
                .map(|i| i.state)
                .unwrap_or(RoadState::Future)
        }
    };
    let status = {
        let road_state = road_state.clone();
        memo(move || road_state().status())
    };
    let state_label = {
        let road_state = road_state.clone();
        rx!(road_state().label().to_string())
    };
    let state_tone = {
        let road_state = road_state.clone();
        rx!(status_tone(road_state().status()))
    };
    let feature_line = {
        let item = item.clone();
        rx!(item().map(|i| i.feature_line()).unwrap_or_default())
    };

    let open = {
        let id = id.clone();
        move || console.open_road(&id)
    };
    let head_id = id.clone();
    let head = tappable(
        vec![ui! {
            view(style = CardHead()) {
                StatusDot(status = status)
                view(style = CardTitle()) {
                    Typography(
                        content = name,
                        kind = typography_kind::Body,
                        weight = Some(FontWeight::SemiBold),
                    )
                }
                Badge(label = state_label, tone = state_tone)
            }
        }],
        open.clone(),
    )
    .with_style(StyleApplication::new(card_head_box_style()))
    .into_element();

    // The edges: what holds this item, and what it holds. Keyed on the
    // edges themselves, so shipping a prerequisite two cards up
    // rebuilds only this strip.
    let edges = {
        let id = id.clone();
        switch(
            move || {
                let road = data.roadmap.get();
                road.item(&id)
                    .map(|i| {
                        (
                            i.blocking().into_iter().map(|e| (e.name.clone(), e.hard)).collect::<Vec<_>>(),
                            i.unlocks.iter().map(|e| e.name.clone()).collect::<Vec<_>>(),
                        )
                    })
                    .unwrap_or_default()
            },
            move |(waiting, unlocks): &(Vec<(String, bool)>, Vec<String>)| {
                let (waiting, unlocks) = (waiting.clone(), unlocks.clone());
                let (wn, un) = (waiting.len(), unlocks.len());
                ui! {
                    view(style = ChipRow()) {
                        for i in 0..wn {
                            Tag(
                                label = if waiting[i].1 {
                                    format!("waits on {} \u{b7} hard", waiting[i].0)
                                } else {
                                    format!("waits on {}", waiting[i].0)
                                },
                                tone = tone::Danger,
                            )
                        }
                        for i in 0..un {
                            Tag(label = format!("unlocks {}", unlocks[i]), tone = tone::Neutral)
                        }
                    }
                }
            },
        )
    };

    // The features bound to it. Keyed on which they are, not on their
    // progress — a module completing moves a bar inside a mounted row.
    let features = {
        let id = id.clone();
        switch(
            move || {
                data.roadmap
                    .get()
                    .item(&id)
                    .map(|i| i.features.iter().map(|f| f.id.clone()).collect::<Vec<_>>())
                    .unwrap_or_default()
            },
            move |ids: &Vec<String>| {
                let ids = ids.clone();
                let n = ids.len();
                ui! {
                    view(style = FeatureList()) {
                        for i in 0..n {
                            RoadFeatureRow(console = console, feature = ids[i].clone())
                        }
                    }
                }
            },
        )
    };

    let entries = item_entries(&id);

    ui! {
        view(style = SurfaceCard()) {
            head
            Typography(content = intent, kind = typography_kind::BodySm, muted = true)
            edges
            features
            view(style = CardFoot()) {
                Typography(content = feature_line, kind = typography_kind::Caption, muted = true)
                Spacer()
                ActionMenu(console = console, id = head_id, entries = entries)
            }
        }
    }
}

/// The verbs an item offers. `Ship` is offered whenever the item is not
/// already shipped — the store says no, in its own words, when the
/// features below it are not out; a menu that hid the verb would leave
/// the reader guessing why (rule 16's disabled-state corollary).
fn item_entries(id: &str) -> Vec<MenuEntry> {
    let road = model::roadmap();
    let Some(item) = road.item(id) else { return Vec::new() };
    let owned = id.to_string();
    let mut entries = vec![
        MenuEntry::new("Edit", Edit::EditRoadmapItem { item: owned.clone() }),
        MenuEntry::new("Add prerequisite", Edit::AddRoadmapDependency { item: owned.clone() }),
    ];
    if item.state != RoadState::Shipped && !item.shelved {
        entries.push(MenuEntry::new("Ship it", Edit::ShipRoadmapItem { item: owned.clone() }));
    }
    entries.push(MenuEntry::new(
        if item.shelved { "Unshelve" } else { "Shelve" },
        Edit::ShelveRoadmapItem { item: owned.clone(), shelve: !item.shelved },
    ));
    entries.push(MenuEntry::danger("Remove", Edit::RemoveRoadmapItem { item: owned }));
    entries
}

/// Props for [`RoadFeatureRow`].
#[derive(Default, IdealystSchema)]
pub struct RoadFeatureRowProps {
    /// Console state handles.
    pub console: Console,
    /// The feature's id.
    pub feature: String,
}

/// One feature under an item: a handle on it (rule 15 — the row click
/// is the verb), with how far it has got and whether it is out.
#[component]
pub fn RoadFeatureRow(props: &RoadFeatureRowProps) -> Element {
    let console = props.console;
    let data = console.data;
    let id = props.feature.clone();

    let row = {
        let id = id.clone();
        move || {
            let road = data.roadmap.get();
            road.items
                .iter()
                .flat_map(|i| i.features.iter())
                .chain(road.loose_features.iter())
                .find(|f| f.id == id)
                .cloned()
        }
    };
    let name = {
        let row = row.clone();
        rx!(row().map(|f| f.name.clone()).unwrap_or_default())
    };
    let fraction = {
        let row = row.clone();
        rx!(row().map(feature_fraction).unwrap_or(0.0))
    };
    let released = {
        let row = row.clone();
        move || row().map(|f| f.released).unwrap_or(false)
    };
    // Three states, and the badge only speaks for two of them: it is
    // OUT, or the work is done and the ship door has not opened. A
    // feature that is merely unfinished says so with its bar — calling
    // that "held" would borrow the roadmap's word for a lock that does
    // not exist yet.
    let ship_state = {
        let row = row.clone();
        move || match row() {
            Some(f) if f.released => "out",
            Some(f) if f.status == "done" => "unreleased",
            _ => "",
        }
    };
    let counts = {
        let row = row.clone();
        rx!(row()
            .map(|f| format!("{}/{}", f.modules_done, f.modules_total))
            .unwrap_or_default())
    };

    let bar_tone = {
        let released = released.clone();
        rx!(if released() { tone::Success.into() } else { tone::Info.into() })
    };
    // The badge's own slot, keyed on the one word it says — so the
    // pill appears and disappears without the row around it moving.
    let badge = {
        let ship_state = ship_state.clone();
        switch(
            move || ship_state().to_string(),
            move |word: &String| {
                let word = word.clone();
                if word.is_empty() {
                    return ui! { view {} };
                }
                let badge_tone: idea_ui::ToneRef =
                    if word == "out" { tone::Success.into() } else { tone::Warning.into() };
                ui! { Badge(label = word, tone = badge_tone) }
            },
        )
    };
    let go = {
        let id = id.clone();
        move || console.select_feature_by_id(&id)
    };
    tappable(
        vec![ui! {
            view(style = FeatureRow()) {
                view(style = FeatureName()) {
                    Typography(content = name, kind = typography_kind::BodySm)
                }
                Typography(content = counts, kind = typography_kind::Caption, muted = true)
                view(style = FeatureBar()) {
                    Progress(value = fraction, tone = bar_tone, cap = ProgressCap::Rounded)
                }
                badge
            }
        }],
        go,
    )
    .with_style(StyleApplication::new(feature_row_box_style()))
    .into_element()
}

fn feature_fraction(f: RoadFeature) -> f32 {
    if f.modules_total == 0 {
        return 0.0;
    }
    f.modules_done as f32 / f.modules_total as f32
}

/// Props for [`LooseRow`].
#[derive(Default, IdealystSchema)]
pub struct LooseRowProps {
    /// Console state handles.
    pub console: Console,
    /// The feature's id.
    pub feature: String,
}

/// A feature the roadmap does not account for. The row's own verb is
/// binding it — that is the only thing this list is for.
#[component]
pub fn LooseRow(props: &LooseRowProps) -> Element {
    let console = props.console;
    let data = console.data;
    let id = props.feature.clone();

    let name = {
        let id = id.clone();
        rx!(data
            .roadmap
            .get()
            .loose_features
            .iter()
            .find(|f| f.id == id)
            .map(|f| f.name.clone())
            .unwrap_or_default())
    };
    let bind = {
        let id = id.clone();
        move || console.open_edit(Edit::BindFeature { feature: id.clone() })
    };
    tappable(
        vec![ui! {
            view(style = LooseRowBody()) {
                view(style = FeatureName()) {
                    Typography(content = name, kind = typography_kind::BodySm)
                }
                Spacer()
                Typography(content = "Bind\u{2026}", kind = typography_kind::Caption, muted = true)
            }
        }],
        bind,
    )
    .with_style(StyleApplication::new(loose_row_style_style()))
    .into_element()
}

/// Props for [`BlankRoadmap`].
#[derive(Default, IdealystSchema)]
pub struct BlankRoadmapProps {
    /// Console state handles.
    pub console: Console,
}

/// Before the first item exists.
#[component]
pub fn BlankRoadmap(props: &BlankRoadmapProps) -> Element {
    let console = props.console;
    let new_item: Rc<dyn Fn()> =
        Rc::new(move || console.open_edit(Edit::EditRoadmapItem { item: String::new() }));
    ui! {
        view(style = SurfaceCard()) {
            Typography(
                content = "No roadmap yet",
                kind = typography_kind::Body,
                weight = Some(FontWeight::SemiBold),
            )
            Typography(
                content = "An item is one shippable capability and a paragraph of intent. Every \
                           agent reads them, and every feature may be bound to one \u{2014} or to \
                           none, which is normal.",
                kind = typography_kind::BodySm,
                muted = true,
            )
            Button(label = "New item", on_click = new_item)
        }
    }
}

// ---------------------------------------------------------------------
// The drawer
// ---------------------------------------------------------------------

/// Props for [`RoadmapDrawer`].
#[derive(Default, IdealystSchema)]
pub struct RoadmapDrawerProps {
    /// Console state handles.
    pub console: Console,
    /// The item the panel is open onto.
    pub item: String,
}

/// The item's properties: its long form, and both edge lists whole.
#[component]
pub fn RoadmapDrawer(props: &RoadmapDrawerProps) -> Element {
    let console = props.console;
    let id = props.item.clone();
    let backdrop = tappable(Vec::new(), move || console.close_drawer())
        .with_style(StyleApplication::new(road_backdrop_style()))
        .into_element();
    let panel = panel_motion(
        move || ui! { RoadmapPanel(console = console, item = id.clone()) },
        move || console.road_open.get().is_some(),
    );
    ui! {
        view(style = RoadDrawerHost()) {
            backdrop
            panel
        }
    }
}

/// Props for [`RoadmapPanel`].
#[derive(Default, IdealystSchema)]
pub struct RoadmapPanelProps {
    /// Console state handles.
    pub console: Console,
    /// The item's id.
    pub item: String,
}

/// The panel itself.
#[component]
pub fn RoadmapPanel(props: &RoadmapPanelProps) -> Element {
    let console = props.console;
    let data = console.data;
    let id = props.item.clone();
    let Some(item) = model::roadmap().item(&id).cloned() else {
        return ui! { view {} };
    };

    let road_state = {
        let id = id.clone();
        move || {
            data.roadmap
                .get()
                .item(&id)
                .map(|i| i.state)
                .unwrap_or(RoadState::Future)
        }
    };
    let state_label = {
        let road_state = road_state.clone();
        rx!(road_state().label().to_string())
    };
    let state_tone = {
        let road_state = road_state.clone();
        rx!(status_tone(road_state().status()))
    };
    let shipped = if item.shipped_at.is_empty() {
        String::new()
    } else {
        format!("Shipped {} by {}", item.shipped_at, item.shipped_by)
    };
    let waits: Vec<String> = item
        .depends_on
        .iter()
        .map(|e| {
            format!(
                "{}{}{}",
                e.name,
                if e.hard { " \u{b7} hard" } else { "" },
                if e.shipped { " \u{b7} shipped" } else { "" }
            )
        })
        .collect();
    let has_shipped = !shipped.is_empty();
    let unlocks: Vec<String> = item.unlocks.iter().map(|e| e.name.clone()).collect();
    let (wn, un) = (waits.len(), unlocks.len());
    let vision = item.vision.clone();
    let has_vision = !vision.trim().is_empty();
    let intent_text = item.intent.clone();
    let item_name = item.name.clone();
    let entries = item_entries(&id);
    // The same × the other two drawers carry, in the same slot. A
    // footer "Close" beside a corner affordance is UX_GUIDELINES rule
    // 14's exact case, and the drawer already closes on the backdrop.
    let close = tappable(
        vec![ui! { text(style = CloseGlyph()) { "\u{d7}" } }],
        move || console.close_drawer(),
    )
    .with_style(StyleApplication::new(close_box_style()))
    .into_element();

    ui! {
        view(style = RoadPanelBox()) {
            view(style = PanelHead()) {
                view(style = CardTitle()) {
                    Typography(
                        content = item_name.clone(),
                        kind = typography_kind::H3,
                        weight = Some(FontWeight::SemiBold),
                    )
                }
                Badge(label = state_label, tone = state_tone)
                ActionMenu(console = console, id = format!("panel-{id}"), entries = entries)
                close
            }
            scroll_view(style = PanelScroll()) {
                view(style = PanelBody()) {
                    if has_shipped {
                        Typography(content = shipped.clone(), kind = typography_kind::Caption, muted = true)
                    }
                    text(style = SectionLabel()) { "Intent" }
                    Typography(content = intent_text.clone(), kind = typography_kind::BodySm)
                    if has_vision {
                        text(style = SectionLabel()) { "Detail" }
                    }
                    if has_vision {
                        Typography(content = vision.clone(), kind = typography_kind::BodySm, muted = true)
                    }
                    text(style = SectionLabel()) { "Waits on" }
                    if wn == 0 {
                        Typography(content = "Nothing.", kind = typography_kind::Caption, muted = true)
                    }
                    for i in 0..wn {
                        PrereqLine(console = console, item = id.clone(), index = i, label = waits[i].clone())
                    }
                    text(style = SectionLabel()) { "Unlocks" }
                    if un == 0 {
                        Typography(content = "Nothing waits on it.", kind = typography_kind::Caption, muted = true)
                    }
                    for i in 0..un {
                        Typography(content = unlocks[i].clone(), kind = typography_kind::BodySm)
                    }
                }
            }
        }
    }
}

/// Props for [`PrereqLine`].
#[derive(Default, IdealystSchema)]
pub struct PrereqLineProps {
    /// Console state handles.
    pub console: Console,
    /// The item that waits.
    pub item: String,
    /// Which of its prerequisites this is.
    pub index: usize,
    /// The rendered label.
    pub label: String,
}

/// One prerequisite, with the one verb it carries.
#[component]
pub fn PrereqLine(props: &PrereqLineProps) -> Element {
    let console = props.console;
    let item = props.item.clone();
    let label = props.label.clone();
    let depends_on = model::roadmap()
        .item(&item)
        .and_then(|i| i.depends_on.get(props.index).map(|e| e.item_id.clone()))
        .unwrap_or_default();
    let drop = {
        let (item, depends_on) = (item.clone(), depends_on.clone());
        move || console.open_edit(Edit::RemoveRoadmapDependency {
            item: item.clone(),
            depends_on: depends_on.clone(),
        })
    };
    tappable(
        vec![ui! {
            view(style = PrereqRowBody()) {
                view(style = FeatureName()) {
                    Typography(content = label, kind = typography_kind::BodySm)
                }
                Spacer()
                Typography(content = "Remove", kind = typography_kind::Caption, muted = true)
            }
        }],
        drop,
    )
    .with_style(StyleApplication::new(prereq_row_style_style()))
    .into_element()
}

// ---------------------------------------------------------------------
// Styles
// ---------------------------------------------------------------------

stylesheet! {
    pub RoadmapScreen<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            // Rule 27: every ancestor between the viewport and the
            // scroller must be able to shrink below its content, or
            // the scroller never clamps and never shows a bar.
            min_height: 0,
            gap: t.spacing.md(),
        }
    }
}

stylesheet! {
    pub ScreenHead<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
            // Chrome never gives (rule 27): an over-tall body is paid
            // for by the scroller, not by squashing the header.
            flex_shrink: 0.0,
            padding_left: t.spacing.lg(),
            padding_right: t.spacing.lg(),
            padding_top: t.spacing.lg(),
        }
    }
}

stylesheet! {
    pub HeadText<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
            // Rule 22: the flexible side yields, the chrome does not.
            min_width: 0,
            flex_shrink: 1.0,
        }
    }
}

stylesheet! {
    pub RoadmapScroll<IdeaThemeRef> {
        base(_t) {
            flex_grow: 1.0,
            min_height: 0,
        }
    }
}

stylesheet! {
    // Rule 3: the padding is the content's, so the scrollbar hugs the
    // pane's edge and the first card does not clip against a pad.
    pub RoadmapBody<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.lg(),
            padding: t.spacing.lg(),
        }
    }
}

stylesheet! {
    pub HorizonBlock<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
        }
    }
}

stylesheet! {
    // One card boundary per item (rule 13): the feature rows and the
    // chips inside are bands and chips, never nested cards.
    pub SurfaceCard<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
            padding: t.spacing.md(),
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.md(),
            background: t.color.surface(),
        }
    }
}

stylesheet! {
    pub CardHead<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
        }
    }
}

stylesheet! {
    pub CardTitle<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            // Rule 22 again: a long item name wraps rather than
            // pushing the state badge through the card's border.
            min_width: 0,
            flex_shrink: 1.0,
            flex_grow: 1.0,
        }
    }
}

stylesheet! {
    pub ChipRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            gap: t.spacing.xs(),
        }
    }
}

stylesheet! {
    pub FeatureList<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
        }
    }
}

stylesheet! {
    pub FeatureRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
        }
    }
}

stylesheet! {
    pub FeatureName<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            min_width: 0,
            flex_shrink: 1.0,
            flex_grow: 1.0,
        }
    }
}

stylesheet! {
    pub FeatureBar<IdeaThemeRef> {
        base(_t) {
            width: 80,
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    pub CardFoot<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
        }
    }
}

stylesheet! {
    // Absolute on all four sides, like the module and want drawers.
    // A host that only lays out in flow sizes to its panel's content
    // and pins the overlay to the bottom of the shell instead of
    // covering it.
    pub RoadDrawerHost<IdeaThemeRef> {
        base(_t) {
            position: Position::Absolute,
            top: 0,
            left: 0,
            right: 0,
            bottom: 0,
        }
    }
}

stylesheet! {
    pub RoadPanelBox<IdeaThemeRef> {
        base(t) {
            position: Position::Absolute,
            top: 0,
            right: 0,
            bottom: 0,
            width: 420,
            max_width: runtime_core::Length::Percent(92.0),
            flex_direction: FlexDirection::Column,
            background: t.color.surface(),
            border_left_width: 1.0,
            border_color: t.color.border(),
        }
    }
}

stylesheet! {
    pub PanelHead<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
            flex_shrink: 0.0,
            padding: t.spacing.md(),
            border_bottom_width: 1.0,
            border_color: t.color.border(),
        }
    }
}

stylesheet! {
    pub PanelScroll<IdeaThemeRef> {
        base(_t) {
            flex_grow: 1.0,
            min_height: 0,
        }
    }
}

stylesheet! {
    pub PanelBody<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
            padding: t.spacing.md(),
        }
    }
}

stylesheet! {
    // The card's head is the drawer's handle (rule 12: it looks it).
    pub CardHeadBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            cursor: Cursor::Pointer,
            border_radius: t.radius.sm(),
            padding: t.spacing.xs(),
            margin: -4,
        }
        state hovered(t) { background: t.color.surface_alt() }
        transitions { background: 160ms EaseOut }
    }
}

stylesheet! {
    pub ToggleBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            cursor: Cursor::Pointer,
            flex_shrink: 0.0,
            padding_left: t.spacing.sm(),
            padding_right: t.spacing.sm(),
            padding_top: t.spacing.xs(),
            padding_bottom: t.spacing.xs(),
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.pill(),
        }
        state hovered(t) { background: t.color.surface_alt() }
        transitions { background: 160ms EaseOut }
    }
}

stylesheet! {
    pub ToggleLabel<IdeaThemeRef> {
        base(t) {
            color: t.color.text_muted(),
            font_size: 12,
        }
    }
}

stylesheet! {
    pub FeatureRowBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            cursor: Cursor::Pointer,
            border_radius: t.radius.sm(),
            padding_left: t.spacing.xs(),
            padding_right: t.spacing.xs(),
            padding_top: t.spacing.xs(),
            padding_bottom: t.spacing.xs(),
        }
        state hovered(t) { background: t.color.surface_alt() }
        transitions { background: 160ms EaseOut }
    }
}

stylesheet! {
    pub LooseRowBody<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
        }
    }
}

stylesheet! {
    pub LooseRowStyle<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            cursor: Cursor::Pointer,
            border_radius: t.radius.sm(),
            padding_left: t.spacing.xs(),
            padding_right: t.spacing.xs(),
            padding_top: t.spacing.xs(),
            padding_bottom: t.spacing.xs(),
        }
        state hovered(t) { background: t.color.surface_alt() }
        transitions { background: 160ms EaseOut }
    }
}

stylesheet! {
    pub PrereqRowBody<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
        }
    }
}

stylesheet! {
    pub PrereqRowStyle<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            cursor: Cursor::Pointer,
            border_radius: t.radius.sm(),
            padding: t.spacing.xs(),
        }
        state hovered(t) { background: t.color.surface_alt() }
        transitions { background: 160ms EaseOut }
    }
}

stylesheet! {
    pub RoadBackdrop<IdeaThemeRef> {
        base(t) {
            position: Position::Absolute,
            top: 0,
            left: 0,
            right: 0,
            bottom: 0,
            background: t.color.overlay(),
            cursor: Cursor::Pointer,
        }
    }
}
