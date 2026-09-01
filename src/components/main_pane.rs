//! Main pane: the want pool, the all-features screen, or one feature —
//! header (title, stats, tabs) and the active view. Rebuilt via one
//! coarse `switch` whenever the pane, selection, view tab, tree toggles,
//! or drawer target change, which keeps every view a plain function of
//! state.

use std::rc::Rc;

use idea_ui::{typography_kind, IdeaThemeRef, Stack, StackAlign, StackAxis, StackGap,
    Tab, Tabs, Typography};
use runtime_core::{
    component, pressable, signal, stylesheet, switch, ui, AlignItems, Element, FlexDirection,
    FlexWrap, FontWeight, IdealystSchema, IntoElement, JustifyContent, StyleApplication,
};

use crate::components::bits::{Mono, StatusBadge};
use crate::components::board::BoardView;
use crate::components::feature_list::FeaturesView;
use crate::components::feed::FeedView;
use crate::components::graph::GraphView;
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
            let rev = if pane == "wants" || pane == "features" {
                0
            } else {
                console.rev.get()
            };
            (
                pane,
                console.feature.get(),
                console.view.get(),
                console.toggled.get(),
                console.selected.get(),
                rev,
            )
        },
        move |state: &(String, usize, String, Vec<String>, Option<(usize, usize)>, u64)| {
            let (pane, fi, active_view, toggled, selected, _rev) = state.clone();
            if pane == "wants" {
                return ui! { WantsView(console = console) };
            }
            if pane == "features" {
                return ui! { FeaturesView(console = console) };
            }
            if features().get(fi).is_none() {
                return empty_pane();
            }
            pane_body(console, fi, active_view, toggled, selected)
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
    selected: Option<(usize, usize)>,
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
                BoardView(console = console, feature = fi, selected = selected)
            }
            if is_tree {
                scroll_view(style = PaneScroll()) {
                    view(style = PanePad()) {
                        TreeView(
                            console = console,
                            feature = fi,
                            toggled = toggled.clone(),
                            selected = selected,
                        )
                    }
                }
            }
            if is_feed {
                FeedView(feature = fi)
            }
            if is_graph {
                GraphView(console = console, feature = fi, selected = selected)
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

/// Feature title, rollup stats, and the view tab strip.
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
    let (tasks_done, tasks_total, tasks_added) = f.task_count();
    let elapsed = f.elapsed.to_string();
    let sources = f.sources.len();

    let tabs = signal(vec![
        Tab::new("board", "Stage pipeline"),
        Tab::new("tree", "Hierarchy"),
        Tab::new("feed", "Live feed"),
        Tab::new("graph", "Dependency graph"),
        Tab::new("origin", "Composed from"),
    ]);
    let on_change: Rc<dyn Fn(String)> = Rc::new(move |id| console.view.set(id));

    ui! {
        view(style = HeadBox()) {
            view(style = HeadLeft()) {
                Stack(axis = StackAxis::Row, gap = StackGap::Md, align = StackAlign::Center) {
                    Typography(
                        content = name,
                        kind = typography_kind::H2,
                        weight = Some(FontWeight::SemiBold),
                    )
                    StatusBadge(status = status)
                    Mono(content = agent, size = MonoTextSize::Overline)
                }
                Tabs(tabs = tabs, active = console.view, on_change = on_change)
            }
            view(style = StatRow()) {
                HeadStat(value = format!("{stages_done}/{stages_total}"), label = "stages")
                HeadStat(value = format!("{mods_done}/{mods_total}"), label = "modules")
                HeadStat(value = format!("{tasks_done}/{tasks_total}"), label = "tasks")
                HeadStat(value = format!("{tasks_added}"), label = "agent-added")
                if sources > 0 {
                    HeadStat(value = format!("{sources}"), label = "from wants")
                }
                HeadStat(value = elapsed, label = "elapsed")
            }
        }
    }
}

/// Props for [`HeadStat`].
#[derive(Default, IdealystSchema)]
pub struct HeadStatProps {
    /// Stat value.
    pub value: String,
    /// Uppercase stat label.
    pub label: &'static str,
}

/// One rollup stat in the feature header.
#[component]
pub fn HeadStat(props: &HeadStatProps) -> Element {
    let value = props.value.clone();
    let label = props.label;
    ui! {
        view(style = StatCol()) {
            Typography(
                content = value,
                kind = typography_kind::BodyLg,
                weight = Some(FontWeight::SemiBold),
            )
            text(style = crate::styles::SectionLabel()) { label }
        }
    }
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
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexEnd,
            justify_content: JustifyContent::SpaceBetween,
            flex_wrap: FlexWrap::Wrap,
            gap: t.spacing.lg(),
            padding_top: t.spacing.lg(),
            padding_horizontal: t.spacing.xl(),
            border_bottom_width: 1.0,
            border_color: t.color.border(),
            background: t.color.surface(),
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
