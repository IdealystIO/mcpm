//! Main pane: the want pool, the all-features screen, the knowledge
//! base, or one feature — header (title, stats, tabs) and the active
//! view. Rebuilt via one
//! coarse `switch` whenever the pane, selection, view tab, tree toggles,
//! or drawer target change, which keeps every view a plain function of
//! state.

use std::rc::Rc;

use idea_ui::{typography_kind, Field, IdeaThemeRef, Popover, Spacer, Tab, Tabs, Typography};
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide};
use runtime_core::{
    component, presence, pressable, signal, stylesheet, switch, ui, AlignItems, Easing, Element,
    FlexDirection, FlexWrap, FontWeight, IdealystSchema, IntoElement, JustifyContent,
    PresenceAnim, PresenceState, PressableHandle, Ref, StyleApplication,
};

use crate::components::bits::{Mono, StatusBadge, StatusDot};
use crate::components::board::BoardView;
use crate::components::feature_list::FeaturesView;
use crate::components::feed::FeedView;
use crate::components::graph::GraphView;
use crate::components::knowledge::KnowledgeView;
use crate::components::overview::OverviewView;
use crate::components::tree::TreeView;
use crate::components::wants::WantsView;
use crate::model::features;
use crate::state::Console;
use crate::styles::MonoTextSize;

/// Props for [`MainPane`].
#[derive(Default, IdealystSchema)]
pub struct MainPaneProps {
    /// Console state handles.
    pub console: Console,
}

/// Everything right of the sidebar.
#[component]
pub fn MainPane(props: &MainPaneProps) -> Element {
    let console = props.console;
    switch(
        move || {
            // The wants pane deliberately does NOT read `rev`: it owns
            // its own data-keyed switches, and rebuilding it here would
            // recreate the capture editor under the user's cursor every
            // time a poll landed.
            let pane = console.pane.get();
            // Neither the pool nor the features screen reads `rev`
            // here: both own their own data-keyed switches, and
            // rebuilding them from this one would recreate the field
            // the user is typing into every time a poll landed.
            let rev = if pane == "wants" || pane == "features" || pane == "knowledge"
                || pane == "overview"
            {
                0
            } else {
                console.rev.get()
            };
            // Deliberately NOT keyed on the drawer target: opening a
            // module used to rebuild the whole pane behind the drawer,
            // which threw away the very node the selection highlight
            // wanted to animate. Every surface that draws a selection
            // now reads `Console::selected` itself.
            (
                pane,
                console.feature.get(),
                console.view.get(),
                console.toggled.get(),
                rev,
            )
        },
        move |state: &(String, usize, String, Vec<String>, u64)| {
            let (pane, fi, active_view, toggled, _rev) = state.clone();
            if pane == "overview" {
                return ui! { OverviewView(console = console) };
            }
            if pane == "wants" {
                return ui! { WantsView(console = console) };
            }
            if pane == "features" {
                return ui! { FeaturesView(console = console) };
            }
            if pane == "knowledge" {
                return ui! { KnowledgeView(console = console) };
            }
            if features().get(fi).is_none() {
                return empty_pane();
            }
            pane_body(console, fi, active_view, toggled)
        },
    )
}

/// Shown before the first snapshot lands, or when the store holds no
/// features yet.
fn empty_pane() -> Element {
    let (title, body) = if crate::model::loaded() {
        (
            "No features yet",
            "Connect to the `mcpm` MCP server and call plan_feature to plan the first one.",
        )
    } else {
        (
            "Connecting to mcpm…",
            "Start the API host: cargo run -p api --bin mcpm-web --features server",
        )
    };
    ui! {
        view(style = EmptyPane()) {
            Typography(content = title, kind = typography_kind::H2, weight = Some(FontWeight::SemiBold))
            Typography(content = body, kind = typography_kind::Body, muted = true)
        }
    }
}

fn pane_body(
    console: Console,
    fi: usize,
    active_view: String,
    toggled: Vec<String>,
) -> Element {
    let feats = features();
    let f = &feats[fi];
    let sources = f.sources.len();
    let has_sources = sources > 0;
    let is_board = active_view == "board";
    let is_tree = active_view == "tree";
    let is_feed = active_view == "feed";
    let is_graph = active_view == "graph";
    let is_origin = active_view == "origin";

    ui! {
        view(style = PaneBox()) {
            FeatureHead(console = console, feature = fi)
            if is_origin {
                scroll_view(style = PaneScroll()) {
                    view(style = PanePad()) {
                        view(style = OriginBox()) {
                            if has_sources {
                                for i in 0..sources {
                                    OriginRow(console = console, feature = fi, index = i)
                                }
                            }
                            if !has_sources {
                                Typography(
                                    content = "No wants recorded.",
                                    kind = typography_kind::BodySm,
                                    muted = true,
                                )
                            }
                        }
                    }
                }
            }
            if is_board {
                BoardView(console = console, feature = fi)
            }
            if is_tree {
                scroll_view(style = PaneScroll()) {
                    view(style = PanePad()) {
                        TreeView(console = console, feature = fi, toggled = toggled.clone())
                    }
                }
            }
            if is_feed {
                FeedView(feature = fi)
            }
            if is_graph {
                GraphView(console = console, feature = fi)
            }
        }
    }
}

/// Props for [`OriginRow`].
#[derive(Default, IdealystSchema)]
pub struct OriginRowProps {
    /// Console state handles.
    pub console: Console,
    /// Feature index.
    pub feature: usize,
    /// Index into that feature's want sources.
    pub index: usize,
}

/// One loose idea this feature was composed from — the idea in the
/// author's own words, and nothing else.
///
/// Its id, tags, provenance and the composing agent's rationale all
/// live one click away in the want drawer; spelled out here they
/// tripled the height of the list and buried the ideas themselves.
#[component]
pub fn OriginRow(props: &OriginRowProps) -> Element {
    let console = props.console;
    let feats = features();
    let source = &feats[props.feature].sources[props.index];
    let id = source.id.clone();
    let body = source.body.clone();
    // Rows share one card surface (rule 13), so they are separated by a
    // rule rather than each carrying a border of its own.
    let first = if props.index == 0 { "yes" } else { "no" };

    let inner: Element = ui! {
        view(style = OriginRowInner()) {
            view(style = OriginCol()) {
                Typography(content = body, kind = typography_kind::BodySm)
            }
            text(style = OriginChevron()) { "\u{203a}" }
        }
    };

    pressable(vec![inner], move || console.open_want(&id))
        .with_style(StyleApplication::new(origin_row_box_style()).with("first", first.to_string()))
        .into_element()
}

/// Props for [`FeatureHead`].
#[derive(Default, IdealystSchema)]
pub struct FeatureHeadProps {
    /// Console state handles.
    pub console: Console,
    /// Feature index.
    pub feature: usize,
}

/// Feature title, its one-line rollup, and the view tab strip.
///
/// The rollup is a line and not the six-stat row it replaced: every
/// number there was a fraction of the same work, and spelled out as
/// six labelled columns they pushed the tabs — the only controls in
/// the header — most of a screen down (rule 16).
#[component]
pub fn FeatureHead(props: &FeatureHeadProps) -> Element {
    let console = props.console;
    let feats = features();
    let f = &feats[props.feature];
    let name = f.name.clone();
    let status = f.status;
    let agent = f.agent.to_string();
    let (stages_done, stages_total) = f.stage_count();
    let (mods_done, mods_total) = f.module_count();
    let (_tasks_done, _tasks_total, tasks_added) = f.task_count();
    let elapsed = f.elapsed.to_string();
    let pct = f.pct_label();
    let mut meta = format!(
        "{stages_done}/{stages_total} stages \u{b7} {mods_done}/{mods_total} modules \u{b7} \
         {pct} of tasks",
    );
    if tasks_added > 0 {
        meta.push_str(&format!(" \u{b7} {tasks_added} agent-added"));
    }
    meta.push_str(&format!(" \u{b7} {elapsed}"));

    let tabs = signal(vec![
        Tab::new("board", "Stage pipeline"),
        Tab::new("tree", "Hierarchy"),
        Tab::new("feed", "Activity"),
        Tab::new("graph", "Dependencies"),
        Tab::new("origin", "Composed from"),
    ]);
    let on_change: Rc<dyn Fn(String)> = Rc::new(move |id| console.view.set(id));

    ui! {
        view(style = HeadBox()) {
            Crumb(console = console)
            view(style = HeadTitleRow()) {
                view(style = HeadTitleSlot()) {
                    Typography(
                        content = name,
                        kind = typography_kind::H2,
                        weight = Some(FontWeight::SemiBold),
                    )
                }
                view(style = HeadFixed()) {
                    StatusBadge(status = status)
                }
                Spacer()
                FeatureSwitcher(console = console)
            }
            view(style = HeadMetaRow()) {
                view(style = HeadTitleSlot()) {
                    Typography(content = meta, kind = typography_kind::Caption, muted = true)
                }
                view(style = HeadFixed()) {
                    Mono(content = agent, size = MonoTextSize::Overline)
                }
            }
            Tabs(tabs = tabs, active = console.view, on_change = on_change)
        }
    }
}

/// Props for [`Crumb`].
#[derive(Default, IdealystSchema)]
pub struct CrumbProps {
    /// Console state handles.
    pub console: Console,
}

/// The way back up to the feature list.
#[component]
pub fn Crumb(props: &CrumbProps) -> Element {
    let console = props.console;
    let inner: Element = ui! {
        text(style = CrumbText()) { "Features" }
    };
    ui! {
        view(style = CrumbRow()) {
            pressable(vec![inner], move || console.show_features())
                .with_style(StyleApplication::new(crumb_box_style()))
                .into_element()
            text(style = CrumbSep()) { "/" }
        }
    }
}

/// Props for [`FeatureSwitcher`].
#[derive(Default, IdealystSchema)]
pub struct FeatureSwitcherProps {
    /// Console state handles.
    pub console: Console,
}

/// Jump straight from one feature's board to another's.
///
/// The rail beside it only carries what is in play, and the Features
/// screen is a round trip through a table — this is the move a reader
/// comparing two features actually makes.
#[component]
pub fn FeatureSwitcher(props: &FeatureSwitcherProps) -> Element {
    let console = props.console;
    let trigger: Ref<PressableHandle> = Ref::new();

    let inner: Element = ui! {
        view(style = SwitchRow()) {
            Typography(content = "Switch feature", kind = typography_kind::Caption, muted = true)
            text(style = SwitchCaret()) { "\u{25be}" }
        }
    };
    let button = pressable(vec![inner], move || console.toggle_switcher())
        .bind(trigger)
        .with_style(StyleApplication::new(switch_box_style()))
        .into_element();

    let panel = presence(move || ui! { SwitcherPanel(console = console, anchor = Some(trigger)) })
        .present(move || console.switcher_open.get())
        .enter(PresenceAnim::new(
            PresenceState::default().opacity(0.0).translate_y(-4.0).scale(0.98),
            140,
            Easing::EaseOut,
        ))
        .exit(PresenceAnim::new(
            PresenceState::default().opacity(0.0).translate_y(-4.0).scale(0.98),
            110,
            Easing::EaseIn,
        ))
        .into_element();

    ui! {
        view(style = SwitchAnchor()) {
            button
            panel
        }
    }
}

/// Props for [`SwitcherPanel`].
#[derive(Default, IdealystSchema)]
pub struct SwitcherPanelProps {
    /// Console state handles.
    pub console: Console,
    /// The trigger to hang off.
    pub anchor: Option<Ref<PressableHandle>>,
}

/// The switcher's popover: a filter, then every feature.
///
/// The list is behind its own `switch` on the query, so typing
/// re-filters the rows without rebuilding the field the query is being
/// typed into (rule 25).
#[component]
pub fn SwitcherPanel(props: &SwitcherPanelProps) -> Element {
    let console = props.console;
    let Some(anchor) = props.anchor else {
        return ui! { view {} };
    };
    let dismiss: Rc<dyn Fn()> = Rc::new(move || console.switcher_open.set(false));
    let on_query: Rc<dyn Fn(String)> = Rc::new(move |t| console.switcher_query.set(t));

    let rows = switch(
        move || (console.switcher_query.get(), console.feature.get(), console.rev.get()),
        move |(query, sel, _rev): &(String, usize, u64)| {
            let matched = crate::model::filter_features(query, "all");
            let count = matched.len();
            let sel = *sel;
            ui! {
                scroll_view(style = SwitchScroll()) {
                    view(style = SwitchList()) {
                        for i in 0..count {
                            SwitcherRow(
                                console = console,
                                index = matched[i],
                                selected = matched[i] == sel,
                            )
                        }
                        if count == 0 {
                            view(style = SwitchBlank()) {
                                Typography(
                                    content = "No features match.",
                                    kind = typography_kind::Caption,
                                    muted = true,
                                )
                            }
                        }
                    }
                }
            }
        },
    );

    ui! {
        Popover(
            target = Some(AnchorTarget::from(anchor)),
            side = ElementSide::Below,
            align = ElementAlign::End,
            offset = 8.0,
            on_dismiss = Some(dismiss),
        ) {
            view(style = SwitchPanelBox()) {
                Field(
                    value = console.switcher_query,
                    on_change = on_query,
                    placeholder = Some("Find a feature".to_string()),
                )
                rows
            }
        }
    }
}

/// Props for [`SwitcherRow`].
#[derive(Default, IdealystSchema)]
pub struct SwitcherRowProps {
    /// Console state handles.
    pub console: Console,
    /// Index into [`features`].
    pub index: usize,
    /// Whether this is the feature already showing.
    pub selected: bool,
}

/// One feature in the switcher.
#[component]
pub fn SwitcherRow(props: &SwitcherRowProps) -> Element {
    let console = props.console;
    let index = props.index;
    let feats = features();
    let Some(f) = feats.get(index) else {
        return ui! { view {} };
    };
    let name = f.name.clone();
    let status = f.status;
    let pct = f.pct_label();
    let arm = if props.selected { "on" } else { "off" };

    let inner: Element = ui! {
        view(style = SwitchRowInner()) {
            StatusDot(status = status)
            view(style = HeadTitleSlot()) {
                Typography(content = name, kind = typography_kind::Caption)
            }
            view(style = HeadFixed()) {
                Mono(content = pct, size = MonoTextSize::Overline)
            }
        }
    };

    pressable(vec![inner], move || console.select_feature(index))
        .with_style(StyleApplication::new(switch_row_style_style()).with("selected", arm.to_string()))
        .into_element()
}

stylesheet! {
    pub EmptyPane<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
            padding: t.spacing.xxl(),
            max_width: 560,
            background: t.color.background(),
        }
    }
}

stylesheet! {
    pub PaneBox<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            min_width: 0,
            min_height: 0,
            flex_direction: FlexDirection::Column,
            background: t.color.background(),
        }
    }
}

stylesheet! {
    pub HeadBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
            padding_top: t.spacing.md(),
            padding_horizontal: t.spacing.xl(),
            border_bottom_width: 1.0,
            border_color: t.color.border(),
            background: t.color.surface(),
        }
    }
}

stylesheet! {
    pub CrumbRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.xs(),
        }
    }
}

stylesheet! {
    pub CrumbBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            border_radius: t.radius.sm(),
            cursor: runtime_core::Cursor::Pointer,
        }
        transitions {
            opacity: 140ms EaseOut,
        }
        state hovered(_t) {
            opacity: 0.65,
        }
    }
}

stylesheet! {
    pub CrumbText<IdeaThemeRef> {
        base(t) {
            font_size: t.typography.caption_size(),
            color: t.color.text_muted(),
        }
    }
}

stylesheet! {
    pub CrumbSep<IdeaThemeRef> {
        base(t) {
            font_size: t.typography.caption_size(),
            color: t.color.border_strong(),
        }
    }
}

stylesheet! {
    pub HeadTitleRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.md(),
            min_width: 0,
        }
    }
}

stylesheet! {
    pub HeadMetaRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.md(),
            padding_bottom: t.spacing.sm(),
            min_width: 0,
        }
    }
}

// The flexible / fixed pair every row in this header uses (rule 22).
stylesheet! {
    pub HeadTitleSlot<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            min_width: 0,
            flex_grow: 1.0,
            flex_shrink: 1.0,
        }
    }
}

stylesheet! {
    pub HeadFixed<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    pub SwitchAnchor<IdeaThemeRef> {
        base(_t) {
            position: runtime_core::Position::Relative,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    pub SwitchBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.sm(),
            padding_vertical: t.spacing.xs(),
            padding_horizontal: t.spacing.sm(),
            cursor: runtime_core::Cursor::Pointer,
        }
        transitions {
            border_color: 160ms EaseOut,
            background: 160ms EaseOut,
        }
        state hovered(t) {
            border_color: t.color.border_hover(),
            background: t.color.surface_alt(),
        }
    }
}

stylesheet! {
    pub SwitchRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
        }
    }
}

stylesheet! {
    pub SwitchCaret<IdeaThemeRef> {
        base(t) {
            font_size: t.typography.overline_size(),
            color: t.color.text_muted(),
        }
    }
}

// Tight, per rule 2.
stylesheet! {
    pub SwitchPanelBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
            padding: t.spacing.sm(),
            width: 320,
            min_height: 0,
        }
    }
}

stylesheet! {
    pub SwitchScroll<IdeaThemeRef> {
        base(_t) {
            max_height: 236,
            min_height: 0,
        }
    }
}

stylesheet! {
    pub SwitchList<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 1,
        }
    }
}

stylesheet! {
    pub SwitchBlank<IdeaThemeRef> {
        base(t) {
            padding: t.spacing.sm(),
        }
    }
}

stylesheet! {
    pub SwitchRowInner<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
            padding_vertical: t.spacing.sm(),
            padding_horizontal: t.spacing.sm(),
            min_width: 0,
        }
    }
}

stylesheet! {
    pub SwitchRowStyle<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            border_radius: t.radius.sm(),
            cursor: runtime_core::Cursor::Pointer,
            overflow: runtime_core::Overflow::Hidden,
        }
        variant selected {
            #[default]
            off(_t) { background: runtime_core::Color("#00000000".into()) }
            on(t) { background: t.intent.primary.soft_bg() }
        }
        transitions {
            background: 160ms EaseOut,
        }
        state hovered(t) {
            background: t.color.surface_alt(),
        }
    }
}

stylesheet! {
    pub HeadLeft<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.md(),
            min_width: 0,
        }
    }
}

stylesheet! {
    pub StatRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            justify_content: JustifyContent::FlexEnd,
            gap: t.spacing.xl(),
            padding_bottom: t.spacing.md(),
        }
    }
}

stylesheet! {
    pub StatCol<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: 2,
            align_items: AlignItems::FlexEnd,
        }
    }
}






stylesheet! {
    pub PaneScroll<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            min_height: 0,
        }
    }
}

stylesheet! {
    pub PanePad<IdeaThemeRef> {
        base(t) {
            padding: t.spacing.xl(),
        }
    }
}

stylesheet! {
    pub OriginBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
            padding_vertical: t.spacing.md(),
            padding_horizontal: t.spacing.lg(),
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.md(),
            background: t.color.surface(),
        }
    }
}

stylesheet! {
    pub OriginRowBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            border_color: t.color.border(),
            cursor: runtime_core::Cursor::Pointer,
        }
        variant first {
            #[default]
            no(t) { border_top_width: 1.0 }
            yes(t) { border_top_width: 0.0 }
        }
        transitions {
            background: 160ms EaseOut,
            border_color: 160ms EaseOut,
            opacity: 160ms EaseOut,
        }
        state hovered(t) {
            background: t.color.surface_alt(),
        }
    }
}

stylesheet! {
    pub OriginRowInner<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.md(),
            padding_vertical: t.spacing.sm(),
        }
    }
}

stylesheet! {
    pub OriginCol<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: 1,
            min_width: 0,
            flex_grow: 1.0,
            flex_shrink: 1.0,
        }
    }
}

stylesheet! {
    pub OriginChevron<IdeaThemeRef> {
        base(t) {
            font_size: t.typography.body_size(),
            color: t.color.text_muted(),
            flex_shrink: 0.0,
        }
    }
}
