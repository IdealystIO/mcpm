//! The roadmap screen: where the product is going, drawn as a graph.
//!
//! One column per depth — every item sits right of everything it waits
//! on — using the same layered router the module graph does
//! ([`crate::components::graph`]). That reuse is the point: the
//! crossing-cost lane placement and the pass-through bus rows took
//! real work to get right, and a second copy beside them would drift.
//!
//! The card is deliberately SMALL. An item's paragraph of intent, its
//! long form, its edges and the features bound to it are properties,
//! and properties open in the drawer (rule 20) — a card that had to be
//! as wide as its intent would not be a card at all (rule 23's
//! corollary), and a canvas of them could not be read at any zoom.
//!
//! Horizon is a chip on the card rather than an axis. Columns mean
//! "waits on"; horizon means nothing of the sort, and laying it along
//! the same axis would draw a claim the data does not make.

use idea_ui::{tone, typography_kind, Badge, Button, IdeaThemeRef, Progress, ProgressCap, Spacer,
    Tag, Typography};
use runtime_core::{
    component, memo, rx, signal, stylesheet, switch, ui, AlignItems, Cursor, Element,
    FlexDirection, FlexWrap, FontWeight, IdealystSchema, IntoElement, Length,
    Overflow, Position, Signal,
    StyleApplication, StyleRules, Tokenized,
};
use std::rc::Rc;

use crate::components::bits::{tappable, Hint, StatusDot};
use crate::components::drawer::{close_box_style, panel_motion, CloseGlyph};
use crate::components::graph::{
    edges, CanvasScroll, CardSize, EdgeSegment, GraphInput, GraphLayout, Hover, LegendRowBox,
    StripScroll,
};
use crate::components::edits::{ActionMenu, MenuEntry};
use crate::model::{self, RoadFeature, RoadState, Status};
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

    // Keyed on the SHAPE of the graph — which items, their columns and
    // their edges — never on their data. A feature completing four
    // levels down moves a card's tag and an edge's tone without a node
    // here being rebuilt.
    let body = switch(
        move || {
            let road = data.roadmap.get();
            let shelved = console.road_shelved.get();
            let shape: Vec<(String, usize, Vec<String>)> = road
                .items
                .iter()
                .filter(|i| shelved || !i.shelved)
                .map(|i| {
                    (
                        i.id.clone(),
                        i.depth.max(1) as usize,
                        i.depends_on.iter().map(|e| e.item_id.clone()).collect(),
                    )
                })
                .collect();
            (shape, road.loose_features.len(), road.items.is_empty())
        },
        move |(shape, loose, empty): &(Vec<(String, usize, Vec<String>)>, usize, bool)| {
            if *empty {
                return ui! {
                    view(style = RoadmapBody()) { BlankRoadmap(console = console) }
                };
            }
            canvas_body(console, shape.clone(), *loose)
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
            body
        }
    }
}

/// One laid-out roadmap: edges under, cards over, both scrollers.
///
/// The geometry, the router and the edge tones are the module graph's
/// ([`crate::components::graph`]); only the card is this screen's.
fn canvas_body(console: Console, shape: Vec<(String, usize, Vec<String>)>, loose: usize) -> Element {
    let data = console.data;
    let ids: Vec<String> = shape.iter().map(|(id, _, _)| id.clone()).collect();
    // Statuses by node index, for the edge tones. Read live, so an item
    // shipping recolours its edges without relaying the graph.
    let tone_ids = ids.clone();
    let statuses = memo(move || {
        let road = data.roadmap.get();
        tone_ids
            .iter()
            .map(|id| road.item(id).map(|i| i.state.status()).unwrap_or_default())
            .collect::<Vec<Status>>()
    });

    let road = data.roadmap.get();
    let inputs: Vec<GraphInput> = shape
        .iter()
        .map(|(id, depth, deps)| GraphInput {
            id: id.clone(),
            depth: *depth,
            // An edge to an item that is not drawn is not in the graph
            // either: the router would otherwise reserve a column for a
            // node nothing renders. This is what shelving looks like
            // from here.
            depends_on: deps.iter().filter(|d| ids.contains(d)).cloned().collect(),
            status: road.item(id).map(|i| i.state.status()).unwrap_or_default(),
        })
        .collect();

    let layout = GraphLayout::compute(&inputs, ROAD_CARD);
    let segments = edges(&layout);
    let cards = layout.cards();
    let hovered: Signal<Option<Hover>> = signal(None);

    let mut canvas = StyleRules::default();
    canvas.position = Some(Position::Relative);
    canvas.width = Some(Tokenized::Literal(Length::Px(layout.width())));
    canvas.height = Some(Tokenized::Literal(Length::Px(layout.height())));
    canvas.flex_shrink = Some(Tokenized::Literal(0.0));
    // The strip between the two scrollers — the pattern is graph.rs's,
    // and the reason is UX_GUIDELINES rule 23.
    let mut strip = StyleRules::default();
    strip.width = Some(Tokenized::Literal(Length::Px(layout.width())));
    strip.min_width = Some(Tokenized::Literal(Length::Percent(100.0)));
    strip.height = Some(Tokenized::Literal(Length::Percent(100.0)));
    strip.min_height = Some(Tokenized::Literal(Length::Px(0.0)));
    strip.flex_shrink = Some(Tokenized::Literal(0.0));
    strip.flex_direction = Some(FlexDirection::Column);

    ui! {
        view(style = CanvasHost()) {
            view(style = LegendRowBox()) {
                Spacer()
                Hint(
                    text = "An item sits right of everything it waits on.\n\
                            Green edge: both ends shipped. Blue: the prerequisite has shipped \
                            and this one may go. Red: it is still waiting.\n\
                            A HARD edge also holds the WORK — the dependent's modules cannot \
                            be claimed until the prerequisite ships.\n\
                            Open a card for its intent, its edges and the features on it.",
                )
            }
            scroll_view(horizontal = true, style = StripScroll()) {
                view(style = strip) {
                    scroll_view(style = CanvasScroll()) {
                        view(style = canvas) {
                            for seg in segments, key = seg.id {
                                EdgeSegment(
                                    x = seg.x, y = seg.y, w = seg.w, h = seg.h,
                                    edge = seg.edge, from = seg.from, to = seg.to,
                                    hovered = hovered, statuses = statuses,
                                )
                            }
                            for card in cards, key = card.module {
                                RoadmapCard(
                                    console = console,
                                    item = ids[card.module].clone(),
                                    index = card.module,
                                    x = card.x,
                                    y = card.y,
                                    hovered = hovered,
                                )
                            }
                        }
                    }
                }
            }
            if loose > 0 {
                LooseStrip(console = console)
            }
        }
    }
}

/// The roadmap's card geometry.
///
/// This is the size the ROUTER lays out with, so it is the size the
/// card must actually be. A card that paints past it covers the
/// pass-through rows reserved for long edges in the columns beside it
/// — which reads as an edge cutting straight through a card, and no
/// test catches it because the arithmetic was right and the box lied.
/// Sized for the three rows below at the longest item name we have.
const ROAD_CARD: CardSize = CardSize { w: 248.0, h: 108.0 };

/// Props for [`LooseStrip`].
#[derive(Default, IdealystSchema)]
pub struct LooseStripProps {
    /// Console state handles.
    pub console: Console,
}

/// The features the roadmap does not account for. A footer rather than
/// a node: they sit on no item, so they are nowhere on the graph, and a
/// reader still has to be able to see that they exist.
#[component]
pub fn LooseStrip(props: &LooseStripProps) -> Element {
    let console = props.console;
    let data = console.data;
    switch(
        move || {
            data.roadmap
                .get()
                .loose_features
                .iter()
                .map(|f| f.id.clone())
                .collect::<Vec<String>>()
        },
        move |ids: &Vec<String>| {
            let ids = ids.clone();
            let n = ids.len();
            ui! {
                view(style = LooseBar()) {
                    text(style = SectionLabel()) {
                        format!("{n} feature{} on no roadmap item", if n == 1 { "" } else { "s" })
                    }
                    view(style = ChipRow()) {
                        for i in 0..n {
                            LooseRow(console = console, feature = ids[i].clone())
                        }
                    }
                }
            }
        },
    )
}

/// Props for [`ShelfToggle`].
#[derive(Default, IdealystSchema)]
pub struct ShelfToggleProps {
    /// Console state handles.
    pub console: Console,
}

/// Draw shelved items too. A chip rather than a tab: shelved is a
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
#[derive(IdealystSchema)]
pub struct RoadmapCardProps {
    /// Console state handles.
    pub console: Console,
    /// The item's id. An id and not an index: the roadmap re-sorts as
    /// items ship, and an index would slide onto another item.
    pub item: String,
    /// Its node index, which is what the hover and the edges key on.
    pub index: usize,
    /// Canvas-relative left edge, px.
    pub x: f32,
    /// Canvas-relative top edge, px.
    pub y: f32,
    /// What the canvas's pointer is on, shared by every card and edge.
    pub hovered: Signal<Option<Hover>>,
}

impl Default for RoadmapCardProps {
    fn default() -> Self {
        Self {
            console: Console::default(),
            item: String::new(),
            index: 0,
            x: 0.0,
            y: 0.0,
            hovered: signal(None),
        }
    }
}

/// One item at its slot. Everything inside reads the item live off
/// `Data::roadmap`, so a state change moves a tag in place and the card
/// is not rebuilt. Hovering it lights every edge in or out of it; a
/// click opens the drawer, where its properties are.
#[component]
pub fn RoadmapCard(props: &RoadmapCardProps) -> Element {
    let console = props.console;
    let data = console.data;
    let id = props.item.clone();
    let index = props.index;
    let hovered = props.hovered;

    let read = {
        let id = id.clone();
        move || data.roadmap.get().item(&id).cloned()
    };
    let name = {
        let read = read.clone();
        rx!(read().map(|i| i.name.clone()).unwrap_or_default())
    };
    let status = {
        let read = read.clone();
        memo(move || read().map(|i| i.state.status()).unwrap_or_default())
    };
    let state_label = {
        let read = read.clone();
        rx!(read().map(|i| i.state.label().to_string()).unwrap_or_default())
    };
    let state_tone = {
        let read = read.clone();
        rx!(status_tone(read().map(|i| i.state).unwrap_or(RoadState::Future).status()))
    };
    // Its own slot, keyed on the label, so a horizon appearing or being
    // cleared does not rebuild the card around it.
    let horizon = {
        let read = read.clone();
        switch(
            move || read().map(|i| i.horizon.trim().to_string()).unwrap_or_default(),
            move |label: &String| {
                let label = label.clone();
                if label.is_empty() {
                    return ui! { view {} };
                }
                ui! { Tag(label = label, tone = tone::Neutral) }
            },
        )
    };
    let features = {
        let read = read.clone();
        rx!(read().map(|i| i.feature_line()).unwrap_or_default())
    };
    let open = {
        let id = id.clone();
        move || console.open_road(&id)
    };
    let inner = ui! {
        view(style = CardBody()) {
            view(style = CardHead()) {
                StatusDot(status = status)
                view(style = CardTitle()) {
                    Typography(
                        content = name,
                        kind = typography_kind::BodySm,
                        weight = Some(FontWeight::SemiBold),
                    )
                }
            }
            view(style = ChipRow()) {
                Badge(label = state_label, tone = state_tone)
                horizon
            }
            Typography(content = features, kind = typography_kind::Caption, muted = true)
        }
    };
    let pressable = tappable(vec![inner], open)
        .with_style(StyleApplication::new(card_box_style()))
        .into_element();

    let mut rules = StyleRules::default();
    rules.position = Some(Position::Absolute);
    rules.left = Some(Tokenized::Literal(Length::Px(props.x)));
    rules.top = Some(Tokenized::Literal(Length::Px(props.y)));
    rules.width = Some(Tokenized::Literal(Length::Px(ROAD_CARD.w)));
    rules.height = Some(Tokenized::Literal(Length::Px(ROAD_CARD.h)));
    rules.flex_direction = Some(FlexDirection::Column);

    // LEFT HAND-BUILT for the same reason graph.rs's card is: `on_hover`
    // is a builder-only channel on `view` and the `ui!` tag form has no
    // prop for it. Leaving clears the name only if it is still ours —
    // the pointer may already be on a line.
    // idealyst-lint-disable-next-line prefer-ui-macro
    runtime_core::view(vec![pressable])
        .with_style(std::rc::Rc::new(runtime_core::StyleSheet::r#static(rules)))
        .on_hover(move |entering| {
            if entering {
                hovered.set(Some(Hover::Module(index)));
            } else if hovered.get() == Some(Hover::Module(index)) {
                hovered.set(None);
            }
        })
        .into_element()
}

/// The verbs an item offers./// The verbs an item offers. `Ship` is offered whenever the item is not
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
    // The features moved off the card and in here: they are properties
    // of the item, and a canvas card that listed them could not be a
    // card (rule 20, and rule 23's corollary about width).
    let feature_ids: Vec<String> = item.features.iter().map(|f| f.id.clone()).collect();
    let fn_count = feature_ids.len();
    let released_line = item.feature_line();
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
                    text(style = SectionLabel()) { "Features" }
                    if fn_count == 0 {
                        Typography(
                            content = "No features bound. Bind one from its own board, or from the strip under the canvas.",
                            kind = typography_kind::Caption,
                            muted = true,
                        )
                    }
                    for i in 0..fn_count {
                        RoadFeatureRow(console = console, feature = feature_ids[i].clone())
                    }
                    if fn_count > 0 {
                        Typography(content = released_line.clone(), kind = typography_kind::Caption, muted = true)
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


stylesheet! {
    // The canvas's host: chrome that never gives, and a scroller that
    // takes the rest (rule 27).
    pub CanvasHost<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            min_height: 0,
        }
    }
}

stylesheet! {
    // The card's own surface, inside the absolutely-positioned slot.
    pub CardBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            cursor: Cursor::Pointer,
            padding: t.spacing.sm(),
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.md(),
            background: t.color.surface(),
        }
        state hovered(t) { border_color: t.color.text_muted() }
        transitions { border_color: 160ms EaseOut }
    }
}

stylesheet! {
    pub CardBody<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            gap: t.spacing.xs(),
            // The card is a fixed box on a canvas: anything that would
            // not fit is clipped rather than allowed to paint over the
            // lanes its neighbours' edges run in.
            overflow: Overflow::Hidden,
        }
    }
}

stylesheet! {
    // The loose-feature footer. Pinned, so it can never be the thing
    // that gives when the canvas is tall (rule 27).
    pub LooseBar<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
            flex_shrink: 0.0,
            padding_left: t.spacing.lg(),
            padding_right: t.spacing.lg(),
            padding_top: t.spacing.sm(),
            padding_bottom: t.spacing.sm(),
            border_top_width: 1.0,
            border_color: t.color.border(),
        }
    }
}
