//! The project home screen: what needs a person, what is moving, and
//! what just happened.
//!
//! The console used to open on a feature board, which answered "how is
//! this one feature going" before anyone had asked it — and never
//! answered "is anything stuck", the question a manager actually opens
//! the console with. This screen is project-wide by construction: every
//! row here is derived across all features at once, and each is a
//! handle on something (rule 20) that opens the same drawer the row's
//! own screen would.

use idea_ui::{typography_kind, Badge, IdeaThemeRef, Progress, ProgressCap, Typography};
use runtime_core::{
    component, pressable, stylesheet, switch, ui, AlignItems, Element, FlexDirection, FontWeight,
    IdealystSchema, IntoElement, StyleApplication,
};

use crate::components::bits::{Mono, StatusDot};
use crate::model::{
    active_agent_count, attention, features, in_play, project_name, recent, want_counts,
    AttentionTarget,
};
use crate::state::Console;
use crate::styles::{status_tone, MonoTextSize, MonoTextTone, SectionLabel};

/// Props for [`OverviewView`].
#[derive(Default, IdealystSchema)]
pub struct OverviewViewProps {
    /// Console state handles.
    pub console: Console,
}

/// The project home screen.
#[component]
pub fn OverviewView(props: &OverviewViewProps) -> Element {
    let console = props.console;
    switch(
        move || console.rev.get(),
        move |_: &u64| overview_body(console),
    )
}

fn overview_body(console: Console) -> Element {
    let name = project_name();
    let playing = in_play();
    let (loose, _, _) = want_counts();
    let meta = format!(
        "{} features in play \u{b7} {} agents working \u{b7} {loose} loose ideas",
        playing.len(),
        active_agent_count(),
    );
    let attention_rows = attention();
    let attention_count = attention_rows.len();
    let play_count = playing.len();
    let event_count = recent().len().min(6);

    ui! {
        view(style = OverviewBox()) {
            view(style = OverviewHead()) {
                text(style = SectionLabel()) { "Project" }
                Typography(
                    content = name,
                    kind = typography_kind::H2,
                    weight = Some(FontWeight::SemiBold),
                )
                Typography(content = meta, kind = typography_kind::Caption, muted = true)
            }
            scroll_view(style = OverviewScroll()) {
                view(style = OverviewPad()) {
                    view(style = Section()) {
                        view(style = SectionHead()) {
                            text(style = SectionLabel()) { "Needs attention" }
                            Mono(
                                content = format!("{attention_count}"),
                                size = MonoTextSize::Overline,
                            )
                        }
                        view(style = SurfaceCard()) {
                            for i in 0..attention_count {
                                AttentionRow(console = console, index = i, first = i == 0)
                            }
                            if attention_count == 0 {
                                view(style = BlankRow()) {
                                    Typography(
                                        content = "Nothing is waiting on you.",
                                        kind = typography_kind::BodySm,
                                        muted = true,
                                    )
                                }
                            }
                        }
                    }
                    view(style = SplitRow()) {
                        view(style = SplitWide()) {
                            text(style = SectionLabel()) { "Work in play" }
                            view(style = SurfaceCard()) {
                                for i in 0..play_count {
                                    PlayRow(
                                        console = console,
                                        feature = playing[i],
                                        first = i == 0,
                                    )
                                }
                                if play_count == 0 {
                                    view(style = BlankRow()) {
                                        Typography(
                                            content = "No features in play.",
                                            kind = typography_kind::BodySm,
                                            muted = true,
                                        )
                                    }
                                }
                            }
                        }
                        view(style = SplitNarrow()) {
                            text(style = SectionLabel()) { "Latest activity" }
                            view(style = SurfaceCard()) {
                                view(style = FeedPad()) {
                                    for i in 0..event_count {
                                        ActivityRow(index = i, last = i + 1 == event_count)
                                    }
                                    if event_count == 0 {
                                        Typography(
                                            content = "No events yet.",
                                            kind = typography_kind::BodySm,
                                            muted = true,
                                        )
                                    }
                                    LedgerLink(console = console)
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Props for [`AttentionRow`].
#[derive(Default, IdealystSchema)]
pub struct AttentionRowProps {
    /// Console state handles.
    pub console: Console,
    /// Index into [`attention`].
    pub index: usize,
    /// Whether this is the first row (no divider above it).
    pub first: bool,
}

/// One stuck thing, and the way to it.
#[component]
pub fn AttentionRow(props: &AttentionRowProps) -> Element {
    let console = props.console;
    let rows = attention();
    let Some(row) = rows.get(props.index) else {
        return ui! { view {} };
    };
    let kind = row.kind;
    let status = row.status;
    let title = row.title.clone();
    let place = row.place.clone();
    let target = row.target.clone();
    let first = if props.first { "yes" } else { "no" };

    let inner: Element = ui! {
        view(style = RowInner()) {
            StatusDot(status = status)
            view(style = KindSlot()) {
                Badge(label = kind, tone = status_tone(status))
            }
            view(style = FlexSlot()) {
                Typography(content = title, kind = typography_kind::BodySm)
            }
            view(style = FixedSlot()) {
                Typography(content = place, kind = typography_kind::Caption, muted = true)
            }
            text(style = RowChevron()) { "\u{203a}" }
        }
    };

    pressable(vec![inner], move || match &target {
        AttentionTarget::Module(fi, id) => console.open_module_in(*fi, id),
        AttentionTarget::Pool => console.show_wants(),
    })
    .with_style(StyleApplication::new(divided_row_style()).with("first", first.to_string()))
    .into_element()
}

/// Props for [`PlayRow`].
#[derive(Default, IdealystSchema)]
pub struct PlayRowProps {
    /// Console state handles.
    pub console: Console,
    /// Index into [`features`].
    pub feature: usize,
    /// Whether this is the first row.
    pub first: bool,
}

/// One feature still being worked, with how far along it is.
#[component]
pub fn PlayRow(props: &PlayRowProps) -> Element {
    let console = props.console;
    let index = props.feature;
    let feats = features();
    let Some(f) = feats.get(index) else {
        return ui! { view {} };
    };
    let name = f.name.clone();
    let status = f.status;
    let fraction = f.fraction();
    let pct = f.pct_label();
    let (mods_done, mods_total) = f.module_count();
    let meta = format!(
        "{mods_done}/{mods_total} modules \u{b7} {} ready \u{b7} {}",
        f.ready_count(),
        f.agent,
    );
    let first = if props.first { "yes" } else { "no" };

    let inner: Element = ui! {
        view(style = RowInner()) {
            view(style = FlexSlot()) {
                view(style = TitleLine()) {
                    StatusDot(status = status)
                    view(style = FlexSlot()) {
                        Typography(content = name, kind = typography_kind::BodySm)
                    }
                }
                Typography(content = meta, kind = typography_kind::Caption, muted = true)
            }
            view(style = ProgressSlot()) {
                Mono(content = pct, size = MonoTextSize::Overline)
                Progress(
                    value = fraction,
                    tone = status_tone(status),
                    cap = ProgressCap::Rounded,
                )
            }
        }
    };

    pressable(vec![inner], move || console.select_feature(index))
        .with_style(StyleApplication::new(divided_row_style()).with("first", first.to_string()))
        .into_element()
}

/// Props for [`ActivityRow`].
#[derive(Default, IdealystSchema)]
pub struct ActivityRowProps {
    /// Index into [`recent`], newest first.
    pub index: usize,
    /// Whether this is the last row — its connector stops here.
    pub last: bool,
}

/// One line of the project-wide ledger.
#[component]
pub fn ActivityRow(props: &ActivityRowProps) -> Element {
    let events = recent();
    let Some(event) = events.get(props.index) else {
        return ui! { view {} };
    };
    let time = event.time.clone();
    let title = event.title.clone();
    let agent = event.agent.clone();
    let status = event.status;
    let arm = if props.last { RailLineTail::Last } else { RailLineTail::More };

    ui! {
        view(style = ActivityBox()) {
            view(style = TimeSlot()) {
                Mono(content = time, size = MonoTextSize::Overline)
            }
            view(style = RailCol()) {
                StatusDot(status = status)
                view(style = RailLine().tail(arm)) {}
            }
            view(style = FlexSlot()) {
                Typography(content = title, kind = typography_kind::BodySm)
                Mono(
                    content = agent,
                    size = MonoTextSize::Overline,
                    tone = MonoTextTone::Muted,
                )
            }
        }
    }
}

/// Props for [`LedgerLink`].
#[derive(Default, IdealystSchema)]
pub struct LedgerLinkProps {
    /// Console state handles.
    pub console: Console,
}

/// The way from the six newest events to all of them.
#[component]
pub fn LedgerLink(props: &LedgerLinkProps) -> Element {
    let console = props.console;
    let target = in_play().first().copied();
    let inner: Element = ui! {
        text(style = LinkText()) { "Open the full ledger \u{2192}" }
    };
    pressable(vec![inner], move || {
        if let Some(fi) = target {
            console.select_feature(fi);
            console.view.set("feed".to_string());
        }
    })
    .with_style(StyleApplication::new(link_box_style()))
    .into_element()
}

stylesheet! {
    pub OverviewBox<IdeaThemeRef> {
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
    pub OverviewHead<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
            padding_vertical: t.spacing.lg(),
            padding_horizontal: t.spacing.xl(),
            border_bottom_width: 1.0,
            border_color: t.color.border(),
            background: t.color.surface(),
        }
    }
}

stylesheet! {
    pub OverviewScroll<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            min_height: 0,
        }
    }
}

// Padding sits on the scroll view's CONTENT, not around the scroller —
// otherwise the scrollbar is inset from the pane edge and the first
// card clips against the pad (rule 3).
stylesheet! {
    pub OverviewPad<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.lg(),
            padding: t.spacing.xl(),
            max_width: 1180,
        }
    }
}

stylesheet! {
    pub Section<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
            min_width: 0,
        }
    }
}

stylesheet! {
    pub SectionHead<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
        }
    }
}

stylesheet! {
    pub SplitRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: runtime_core::FlexWrap::Wrap,
            align_items: AlignItems::FlexStart,
            gap: t.spacing.lg(),
        }
    }
}

stylesheet! {
    pub SplitWide<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
            flex_grow: 1.35,
            flex_shrink: 1.0,
            flex_basis: 420,
            min_width: 0,
        }
    }
}

stylesheet! {
    pub SplitNarrow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
            flex_grow: 1.0,
            flex_shrink: 1.0,
            flex_basis: 280,
            min_width: 0,
        }
    }
}

// One card boundary per section (rule 13): the rows inside are split by
// hairlines, never by a border each.
stylesheet! {
    pub SurfaceCard<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.md(),
            background: t.color.surface(),
            overflow: runtime_core::Overflow::Hidden,
            min_width: 0,
        }
    }
}

stylesheet! {
    pub BlankRow<IdeaThemeRef> {
        base(t) {
            padding_vertical: t.spacing.md(),
            padding_horizontal: t.spacing.lg(),
        }
    }
}

stylesheet! {
    pub FeedPad<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            padding_vertical: t.spacing.md(),
            padding_horizontal: t.spacing.lg(),
            min_width: 0,
        }
    }
}

stylesheet! {
    pub DividedRow<IdeaThemeRef> {
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
            background: 140ms EaseOut,
        }
        state hovered(t) {
            background: t.color.surface_alt(),
        }
    }
}

stylesheet! {
    pub RowInner<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.md(),
            padding_vertical: t.spacing.md(),
            padding_horizontal: t.spacing.lg(),
            min_width: 0,
        }
    }
}

stylesheet! {
    pub TitleLine<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
            min_width: 0,
        }
    }
}

// The flexible half of a row (rule 22): `min_width: 0` is what lets it
// shrink below its intrinsic width instead of pushing its siblings out
// through the card's edge.
stylesheet! {
    pub FlexSlot<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: 2,
            min_width: 0,
            flex_grow: 1.0,
            flex_shrink: 1.0,
        }
    }
}

stylesheet! {
    pub FixedSlot<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            flex_shrink: 0.0,
        }
    }
}

// The kind pill holds its width so a column of them lines up and the
// titles beside them start at the same x (rule 18).
stylesheet! {
    pub KindSlot<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: runtime_core::JustifyContent::FlexStart,
            width: 84,
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    pub ProgressSlot<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::FlexEnd,
            gap: t.spacing.xs(),
            width: 104,
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    pub RowChevron<IdeaThemeRef> {
        base(t) {
            font_size: t.typography.body_size(),
            color: t.color.text_muted(),
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    pub ActivityBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            gap: t.spacing.sm(),
            padding_top: t.spacing.sm(),
            min_width: 0,
        }
    }
}

stylesheet! {
    pub TimeSlot<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            justify_content: runtime_core::JustifyContent::FlexEnd,
            padding_top: 2,
            width: 44,
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    pub RailCol<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            padding_top: 4,
            width: 8,
            flex_shrink: 0.0,
        }
    }
}

// The connector under a timeline dot. It stops on the last row so the
// ledger does not read as continuing into the link below it.
stylesheet! {
    pub RailLine<IdeaThemeRef> {
        base(t) {
            width: 1,
            margin_top: 4,
            background: t.color.border(),
        }
        variant tail {
            #[default]
            more(_t) { flex_grow: 1.0, min_height: 10 }
            last(_t) { flex_grow: 0.0, min_height: 0 }
        }
    }
}

stylesheet! {
    pub LinkBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            align_self: runtime_core::AlignSelf::FlexStart,
            border_radius: t.radius.sm(),
            padding_vertical: t.spacing.sm(),
            margin_top: t.spacing.sm(),
            cursor: runtime_core::Cursor::Pointer,
        }
        transitions {
            opacity: 140ms EaseOut,
        }
        state hovered(_t) {
            opacity: 0.7,
        }
    }
}

stylesheet! {
    pub LinkText<IdeaThemeRef> {
        base(t) {
            font_size: t.typography.caption_size(),
            color: t.intent.primary.fg(),
        }
    }
}
