//! Main pane: the want pool, the all-features screen, the knowledge
//! base, or one feature — header (title, stats, tabs) and the active
//! view. Rebuilt via one coarse `switch` whenever the pane, selection
//! or view tab change, which keeps every view a plain function of
//! state.

use std::rc::Rc;

use idea_ui::{typography_kind, Button, Field, IdeaThemeRef, Popover, Spacer, Tab, Tabs, Typography};
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide};
use runtime_core::{
    component, effect, memo, rx, signal, stylesheet, switch, ui, AlignItems, Easing, Element,
    FlexDirection, FlexWrap, FontWeight, IdealystSchema, IntoElement, JustifyContent,
    PresenceAnim, PresenceState, PressableHandle, Ref, StyleApplication,
};

use crate::components::attachments::FilesView;
use crate::components::discussion::Discussion;
use crate::components::bits::{Mono, StatusBadge, StatusDot, tappable};
use crate::components::document::DocumentView;
use crate::components::edits::{feature_entries, ActionMenu};
use crate::components::feature_list::FeaturesView;
use crate::components::feed::FeedView;
use crate::components::graph::GraphView;
use crate::components::knowledge::KnowledgeView;
use crate::components::overview::OverviewView;
use crate::components::plan_editor::PlanEditor;
use crate::components::wants::{CaptureView, WantsView};
use crate::model::Status;
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
    let data = console.data;
    switch(
        move || {
            let pane = console.pane.get();
            let fi = console.feature.get();
            // Keyed on WHICH feature and view are showing, and on
            // whether that feature exists yet — never on its data.
            // Everything inside reads the feature live, so a poll
            // moves a count in the header and a ring on a card
            // without a node here being rebuilt. Deliberately not
            // keyed on the drawer target either: opening a module used
            // to rebuild the whole pane behind the drawer, which threw
            // away the very node the selection highlight wanted to
            // animate. Every surface that draws a selection reads
            // `Console::selected` itself.
            let id = data.features.get().get(fi).map(|f| f.id.clone());
            (pane, fi, console.view.get(), id)
        },
        move |state: &(String, usize, String, Option<String>)| {
            let (pane, fi, active_view, id) = state.clone();
            if pane == "overview" {
                return ui! { OverviewView(console = console) };
            }
            if pane == "wants" {
                return ui! { WantsView(console = console) };
            }
            if pane == "capture" {
                return ui! { CaptureView(console = console) };
            }
            if pane == "plan" {
                return ui! { PlanEditor(console = console) };
            }
            if pane == "features" {
                return ui! { FeaturesView(console = console) };
            }
            if pane == "knowledge" {
                return ui! { KnowledgeView(console = console) };
            }
            let Some(feature_id) = id else {
                return empty_pane(console);
            };
            pane_body(console, fi, feature_id, active_view)
        },
    )
}

/// Shown before the first snapshot lands, or when the store holds no
/// features yet.
fn empty_pane(console: Console) -> Element {
    let loaded = console.data.loaded.get();
    let (title, body) = if loaded {
        (
            "No features yet",
            "Plan the first one here, or connect an agent to the `mcpm` MCP server and call plan_feature.",
        )
    } else {
        (
            "Connecting to mcpm…",
            "Start the API host: cargo run -p api --bin mcpm-web --features server",
        )
    };
    let new_plan: Rc<dyn Fn()> = Rc::new(move || console.show_plan_editor());
    ui! {
        view(style = EmptyPane()) {
            Typography(content = title, kind = typography_kind::H2, weight = Some(FontWeight::SemiBold))
            Typography(content = body, kind = typography_kind::Body, muted = true)
            if loaded {
                Button(label = "New plan", on_click = new_plan)
            }
        }
    }
}

fn pane_body(console: Console, fi: usize, feature_id: String, active_view: String) -> Element {
    let data = console.data;
    let f = data.feature(fi);
    let is_graph = active_view == "graph";
    let is_paper = active_view == "whitepaper";
    let is_feed = active_view == "feed";
    let is_origin = active_view == "origin";
    let is_files = active_view == "files";
    let is_discussion = active_view == "discussion";
    let pending = move || f().map(|f| f.open_questions.len()).unwrap_or(0);
    let pending_line = rx!({
        let n = pending();
        format!(
            "{n} open question{} \u{2014} nothing in this feature is dispatchable until answered.",
            if n == 1 { "" } else { "s" }
        )
    });

    // The whitepaper: remade when the document changes (a new
    // revision, a description edit), which is the one time its text
    // has to be re-laid. Inside the scroller, so the scroll survives
    // everything else.
    let paper = switch(
        move || f().map(|f| (f.description.trim().to_string(), f.whitepaper.clone())),
        move |held: &Option<(String, Option<Rc<crate::model::Document>>)>| {
            let (description, paper) = held.clone().unwrap_or_default();
            let has_description = !description.is_empty();
            let (paper_present, paper_body, paper_meta) = match &paper {
                Some(d) => (true, d.body.clone(), d.meta()),
                None => (false, String::new(), String::new()),
            };
            ui! {
                view(style = PaperCol()) {
                    if has_description {
                        Typography(content = description, kind = typography_kind::Body, muted = true)
                    }
                    DocumentView(
                        console = console,
                        present = paper_present,
                        body = paper_body,
                        meta = paper_meta,
                        empty = "No whitepaper written.",
                    )
                }
            }
        },
    );

    // The origin list: remade when the feature's sources change.
    let origin = switch(
        move || f().map(|f| f.sources.clone()).unwrap_or_default(),
        move |sources: &Rc<Vec<crate::model::WantSource>>| {
            let sources = sources.clone();
            let count = sources.len();
            let has_sources = count > 0;
            ui! {
                view(style = OriginBox()) {
                    if has_sources {
                        for i in 0..count {
                            OriginRow(console = console, source = sources[i].clone(), first = i == 0)
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
        },
    );

    ui! {
        view(style = PaneBox()) {
            FeatureHead(console = console, feature = fi)
            if is_graph {
                GraphView(console = console, feature = fi)
            }
            if is_files {
                FilesView(console = console, feature = fi)
            }
            if is_discussion {
                scroll_view(style = PaneScroll()) {
                    view(style = PanePad()) {
                        view(style = PaperCol()) {
                            if pending() > 0 {
                                Typography(
                                    content = pending_line.clone(),
                                    kind = typography_kind::BodySm,
                                    muted = true,
                                )
                            }
                            Discussion(console = console, subject = feature_id.clone(), compact = false)
                        }
                    }
                }
            }
            if is_paper {
                scroll_view(style = PaneScroll()) {
                    view(style = PanePad()) {
                        paper
                    }
                }
            }
            if is_feed {
                FeedView(console = console, feature = fi)
            }
            if is_origin {
                scroll_view(style = PaneScroll()) {
                    view(style = PanePad()) {
                        origin
                    }
                }
            }
        }
    }
}

/// Props for [`OriginRow`].
#[derive(Default, IdealystSchema)]
pub struct OriginRowProps {
    /// Console state handles.
    pub console: Console,
    /// The want this feature was composed from.
    pub source: crate::model::WantSource,
    /// First of its list — no rule above it.
    pub first: bool,
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
    let id = props.source.id.clone();
    let body = props.source.body.clone();
    // Rows share one card surface (rule 13), so they are separated by a
    // rule rather than each carrying a border of its own.
    let first = if props.first { "yes" } else { "no" };

    let inner: Element = ui! {
        view(style = OriginRowInner()) {
            view(style = OriginCol()) {
                Typography(content = body, kind = typography_kind::BodySm)
            }
            text(style = OriginChevron()) { "\u{203a}" }
        }
    };

    tappable(vec![inner], move || console.open_want(&id))
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
///
/// Built once per feature. Every value reads the feature live: the
/// rollup line moves as tasks land, the ring turns while a box is
/// writing, and the tab labels' counts follow the data through an
/// effect on the tabs signal.
#[component]
pub fn FeatureHead(props: &FeatureHeadProps) -> Element {
    let console = props.console;
    let data = console.data;
    let fi = props.feature;
    let f = data.feature(fi);
    let status = memo(move || f().map(|f| f.status).unwrap_or_default());
    let live = memo(move || {
        let now = data.clock.get();
        f().is_some_and(|f| f.live_at(now))
    });
    let name = rx!(f().map(|f| f.name.clone()).unwrap_or_default());
    let agent = rx!(f().map(|f| f.agent.clone()).unwrap_or_default());
    let meta = rx!(f()
        .map(|f| {
            let (_tasks_done, _tasks_total, tasks_added) = f.task_count();
            let mut meta = f.meta_line();
            if tasks_added > 0 {
                meta.push_str(&format!(" \u{b7} {tasks_added} ad hoc"));
            }
            meta.push_str(&format!(" \u{b7} {}", f.elapsed));
            meta
        })
        .unwrap_or_default());
    let word = move || f().and_then(|f| f.last_word.as_ref().map(|w| w.line()));

    let counts = move || {
        f()
            .map(|f| {
                let pending = f.open_questions.len()
                    + f.modules.iter().map(|m| m.open_questions.len()).sum::<usize>();
                (pending, f.attachments.len())
            })
            .unwrap_or_default()
    };
    let tabs = signal(head_tabs(counts()));
    effect!({
        tabs.set(head_tabs(counts()));
    });
    let on_change: Rc<dyn Fn(String)> = Rc::new(move |id| console.view.set(id));

    // The menu's entries depend on the feature's state (shelved,
    // done); remade when that changes.
    let menu = switch(
        move || f().map(|f| (f.id.clone(), f.status, f.shelved)),
        move |held: &Option<(String, Status, bool)>| {
            let id = held.as_ref().map(|(id, _, _)| id.clone()).unwrap_or_default();
            ui! {
                ActionMenu(console = console, id = "feature".to_string(), entries = feature_entries(&id))
            }
        },
    );

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
                    StatusDot(status = status, live = live)
                    StatusBadge(status = status)
                }
                Spacer()
                FeatureSwitcher(console = console)
                menu
            }
            view(style = HeadMetaRow()) {
                view(style = HeadTitleSlot()) {
                    Typography(content = meta, kind = typography_kind::Caption, muted = true)
                }
                view(style = HeadFixed()) {
                    Mono(content = agent, size = MonoTextSize::Overline)
                }
            }
            match word() {
                Some(word) => {
                    Typography(content = word.clone(), kind = typography_kind::Caption)
                }
                None => {}
            }
            Tabs(tabs = tabs, active = console.view, on_change = on_change)
        }
    }
}

/// The view tabs, with the counts two of them carry.
fn head_tabs((pending, file_count): (usize, usize)) -> Vec<Tab> {
    vec![
        Tab::new("graph", "Graph"),
        Tab::new("whitepaper", "Whitepaper"),
        Tab::new(
            "discussion",
            if pending > 0 { format!("Discussion ({pending} open)") } else { "Discussion".to_string() },
        ),
        Tab::new(
            "files",
            if file_count > 0 { format!("Files ({file_count})") } else { "Files".to_string() },
        ),
        Tab::new("feed", "Activity"),
        Tab::new("origin", "Composed from"),
    ]
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
            tappable(vec![inner], move || console.show_features())
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
    let button = tappable(vec![inner], move || console.toggle_switcher())
        .bind(trigger)
        .with_style(StyleApplication::new(switch_box_style()))
        .into_element();

    let panel: Element = ui! {
        presence(
            present = move || console.switcher_open.get(),
            enter = PresenceAnim::new(
                PresenceState::default().opacity(0.0).translate_y(-4.0).scale(0.98),
                140,
                Easing::EaseOut,
            ),
            exit = PresenceAnim::new(
                PresenceState::default().opacity(0.0).translate_y(-4.0).scale(0.98),
                110,
                Easing::EaseIn,
            ),
        ) {
            SwitcherPanel(console = console, anchor = Some(trigger))
        }
    };

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

    let data = console.data;
    let rows = switch(
        move || {
            let query = console.switcher_query.get();
            let matched = crate::model::filter_features_of(&data.features.get(), &query, "all");
            (matched, console.feature.get())
        },
        move |(matched, sel): &(Vec<usize>, usize)| {
            let matched = matched.clone();
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
    let data = console.data;
    let index = props.index;
    let f = data.feature(index);
    let name = rx!(f().map(|f| f.name.clone()).unwrap_or_default());
    let status = memo(move || f().map(|f| f.status).unwrap_or_default());
    let pct = rx!(f().map(|f| f.pct_label()).unwrap_or_default());
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

    tappable(vec![inner], move || console.select_feature(index))
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

// A reading column: prose wider than this stops being read.
stylesheet! {
    pub PaperCol<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.lg(),
            max_width: 760,
            min_width: 0,
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
