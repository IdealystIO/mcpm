//! Live feed view: the feature's event ledger as a timeline, with the
//! agent roster alongside.

use std::rc::Rc;

use idea_ui::{size, typography_kind, variant, Badge, Button, IdeaThemeRef, Spacer, Stack,
    StackAlign, StackAxis, StackGap, Typography};
use runtime_core::{
    component, stylesheet, ui, AlignItems, Element, FlexDirection, FlexWrap, FontWeight,
    IdealystSchema,
};

use crate::components::bits::{Mono, StatusBadge, StatusDot};
use crate::model::{feed, features};
use crate::state::Console;
use crate::styles::{MonoTextSize, SectionLabel};

/// Props for [`FeedView`].
#[derive(Default, IdealystSchema)]
pub struct FeedViewProps {
    /// Console state handles.
    pub console: Console,
    /// Feature index.
    pub feature: usize,
}

/// The live-feed view for one feature (roster is the project-wide
/// agent registry).
///
/// The ledger is paged: the newest page arrives when the tab opens
/// and refreshes on a tick, and the control at the bottom reads the
/// next older page. A feed is the one part of a feature that grows
/// without bound, so nothing here ever asks for all of it.
#[component]
pub fn FeedView(props: &FeedViewProps) -> Element {
    let console = props.console;
    let fi = props.feature;
    let feats = features();
    let feature_id = feats[fi].id.clone();
    let held = feed(&feature_id);
    let loaded = held.is_some();
    let event_count = held.as_ref().map(|f| f.events.len()).unwrap_or(0);
    let has_older = held.as_ref().is_some_and(|f| !f.exhausted && !f.events.is_empty());
    let blank = if loaded { "No events yet for this feature." } else { "Loading the ledger\u{2026}" };
    let roster_count = crate::model::agents().len();
    let older_id = feature_id.clone();
    let on_older: Rc<dyn Fn()> = Rc::new(move || console.load_older_events(&older_id));
    ui! {
        scroll_view(style = FeedScroll()) {
            view(style = FeedRow()) {
                view(style = EventsCol()) {
                    if event_count == 0 {
                        Typography(content = blank, kind = typography_kind::BodySm, muted = true)
                    }
                    for i in 0..event_count {
                        EventRow(feature = feature_id.clone(), index = i, last = i + 1 == event_count)
                    }
                    if has_older {
                        view(style = OlderRow()) {
                            Button(
                                label = "Load older",
                                on_click = on_older.clone(),
                                size = size::Sm,
                                variant = variant::Ghost,
                            )
                        }
                    }
                }
                view(style = RosterCol()) {
                    text(style = SectionLabel()) { "Registered agents" }
                    for i in 0..roster_count {
                        RosterCard(index = i)
                    }
                }
            }
        }
    }
}

/// Props for [`EventRow`].
#[derive(Default, IdealystSchema)]
pub struct EventRowProps {
    /// The feature whose feed this row is in.
    pub feature: String,
    /// Event index (newest first).
    pub index: usize,
    /// Whether this is the last row (no trailing rail).
    pub last: bool,
}

/// One event ledger entry.
#[component]
pub fn EventRow(props: &EventRowProps) -> Element {
    let Some(held) = feed(&props.feature) else {
        return ui! { view {} };
    };
    let Some(e) = held.events.get(props.index) else {
        return ui! { view {} };
    };
    let time = e.time.to_string();
    let kind = e.kind.to_string();
    let status = e.status;
    let title = e.title.clone();
    let body = e.body.clone();
    let has_body = !body.is_empty();
    let agent = e.agent.to_string();
    let tool = e.tool.to_string();
    let last = props.last;
    ui! {
        view(style = EventGrid()) {
            view(style = TimeCell()) {
                Mono(content = time, size = MonoTextSize::Overline)
            }
            view(style = RailCell()) {
                StatusDot(status = status)
                if !last {
                    view(style = RailLine()) {}
                }
            }
            view(style = EventBody()) {
                Stack(axis = StackAxis::Row, gap = StackGap::Sm, align = StackAlign::Center) {
                    Badge(label = kind, tone = crate::styles::status_tone(status))
                    Typography(
                        content = title,
                        kind = typography_kind::Body,
                        weight = Some(FontWeight::SemiBold),
                    )
                }
                if has_body {
                    Typography(content = body, kind = typography_kind::BodySm, muted = true)
                }
                Stack(axis = StackAxis::Row, gap = StackGap::Sm, align = StackAlign::Center) {
                    Mono(content = agent, size = MonoTextSize::Overline)
                    view(style = ToolChip()) {
                        Mono(
                            content = tool,
                            size = MonoTextSize::Overline,
                            tone = crate::styles::MonoTextTone::Info,
                        )
                    }
                }
            }
        }
    }
}

/// Props for [`RosterCard`].
#[derive(Default, IdealystSchema)]
pub struct RosterCardProps {
    /// Roster index into [`crate::model::agents`].
    pub index: usize,
}

/// One agent card in the roster column.
#[component]
pub fn RosterCard(props: &RosterCardProps) -> Element {
    let agents = crate::model::agents();
    let a = &agents[props.index];
    let id = a.id.to_string();
    let state = a.state;
    let scope = a.scope.clone();
    let level = a.level.to_string();
    let uptime = a.uptime.to_string();
    ui! {
        view(style = RosterBox()) {
            Stack(axis = StackAxis::Row, gap = StackGap::Sm, align = StackAlign::Center) {
                StatusDot(status = state)
                Mono(content = id)
                Spacer()
                StatusBadge(status = state)
            }
            Typography(content = scope, kind = typography_kind::Caption, muted = true)
            Stack(axis = StackAxis::Row, align = StackAlign::Center) {
                Mono(content = level, size = MonoTextSize::Overline)
                Spacer()
                Mono(content = uptime, size = MonoTextSize::Overline)
            }
        }
    }
}

stylesheet! {
    pub FeedScroll<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            min_height: 0,
        }
    }
}

stylesheet! {
    pub FeedRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexStart,
            flex_wrap: FlexWrap::Wrap,
            gap: t.spacing.xl(),
            padding: t.spacing.xl(),
        }
    }
}

stylesheet! {
    pub EventsCol<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            min_width: 420,
            flex_direction: FlexDirection::Column,
        }
    }
}

stylesheet! {
    pub OlderRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            padding_top: t.spacing.md(),
            padding_left: 88,
        }
    }
}

stylesheet! {
    pub RosterCol<IdeaThemeRef> {
        base(t) {
            width: 300,
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            gap: t.spacing.md(),
        }
    }
}

stylesheet! {
    pub EventGrid<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexStart,
            gap: t.spacing.md(),
        }
    }
}

stylesheet! {
    pub TimeCell<IdeaThemeRef> {
        base(t) {
            width: 70,
            flex_shrink: 0.0,
            padding_top: 14,
            align_items: AlignItems::FlexEnd,
        }
    }
}

stylesheet! {
    pub RailCell<IdeaThemeRef> {
        base(t) {
            width: 18,
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            align_self: runtime_core::AlignSelf::Stretch,
            padding_top: 14,
        }
    }
}

stylesheet! {
    pub RailLine<IdeaThemeRef> {
        base(t) {
            width: 1,
            flex_grow: 1.0,
            min_height: 10,
            margin_top: 4,
            background: t.color.border(),
        }
    }
}

stylesheet! {
    pub EventBody<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            min_width: 0,
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
            padding_top: 10,
            padding_bottom: t.spacing.lg(),
        }
    }
}

stylesheet! {
    pub ToolChip<IdeaThemeRef> {
        base(t) {
            border_width: 1.0,
            border_color: t.intent.info.border(),
            border_radius: t.radius.sm(),
            background: t.intent.info.soft_bg(),
            padding_vertical: 1,
            padding_horizontal: 6,
        }
    }
}

stylesheet! {
    pub RosterBox<IdeaThemeRef> {
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
