//! Live feed view: the feature's event ledger as a timeline, with the
//! agent roster alongside.

use std::rc::Rc;

use idea_ui::{size, typography_kind, variant, Badge, Button, IdeaThemeRef, Spacer, Stack,
    StackAlign, StackAxis, StackGap, Typography};
use runtime_core::{
    component, memo, rx, stylesheet, switch, ui, AlignItems, Element, FlexDirection, FlexWrap,
    FontWeight, IdealystSchema, Reactive,
};

use crate::components::bits::{Mono, StatusBadge, StatusDot};
use crate::model::{AgentRow, EventItem, Status};
use crate::state::Console;
use crate::styles::{status_tone, MonoTextSize, SectionLabel};

/// Props for [`FeedView`].
#[derive(Default, IdealystSchema)]
pub struct FeedViewProps {
    /// Console state handles.
    pub console: Console,
    /// Feature index.
    pub feature: usize,
}

/// The live-feed view for one feature (the roster beside it is
/// project-wide: every agent still in the picture — holding a claim,
/// on a box that answers, or heard from today — not every name ever
/// registered).
///
/// The ledger is paged: the newest page arrives when the tab opens
/// and refreshes on a tick, and the control at the bottom reads the
/// next older page. A feed is the one part of a feature that grows
/// without bound, so nothing here ever asks for all of it.
///
/// The rows are a keyed list over the feed's signal: an event that
/// lands is one row inserted at the top, and the rows below it — and
/// the scroll position — stay where they were. The roster is keyed
/// the same way on agent names, and each card reads its agent live.
#[component]
pub fn FeedView(props: &FeedViewProps) -> Element {
    let console = props.console;
    let data = console.data;
    let fi = props.feature;
    // The feature's id is fixed for the life of this view (the pane it
    // sits in is keyed on it), but the closures below re-read it so
    // they stay `Copy`.
    let feature_id = data.feature(fi)().map(|f| f.id.clone()).unwrap_or_default();
    let held = move || {
        let feats = data.features.get();
        let fid = &feats.get(fi)?.id;
        data.feeds.get().get(fid).cloned()
    };
    let events = memo(move || held().map(|f| f.events.clone()).unwrap_or_default());
    let blank = move || match held() {
        Some(f) if f.events.is_empty() => Some("No events yet for this feature."),
        None => Some("Loading the ledger\u{2026}"),
        Some(_) => None,
    };
    let last_seq = move || held().and_then(|f| f.events.last().map(|e| e.seq));
    let has_older = move || held().is_some_and(|f| !f.exhausted && !f.events.is_empty());
    let roster = memo(move || data.agents.get().iter().map(|a| a.id.clone()).collect::<Vec<String>>());
    let older_id = feature_id.clone();
    let on_older: Rc<dyn Fn()> = Rc::new(move || console.load_older_events(&older_id));
    ui! {
        scroll_view(style = FeedScroll()) {
            view(style = FeedRow()) {
                view(style = EventsCol()) {
                    match blank() {
                        Some(word) => {
                            Typography(content = word.to_string(), kind = typography_kind::BodySm, muted = true)
                        }
                        None => {}
                    }
                    for e in events, key = e.seq {
                        EventRow(event = e.clone(), last = rx!(last_seq() == Some(e.seq)))
                    }
                    if has_older() {
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
                    text(style = SectionLabel()) { "Agents" }
                    for name in roster, key = name.clone() {
                        RosterCard(console = console, agent = name.clone())
                    }
                }
            }
        }
    }
}

/// Props for [`EventRow`].
#[derive(IdealystSchema)]
pub struct EventRowProps {
    /// The event.
    pub event: EventItem,
    /// Whether this is the last row (no trailing rail). Live: the
    /// row that was last grows a rail when an older page lands under
    /// it.
    pub last: Reactive<bool>,
}

impl Default for EventRowProps {
    fn default() -> Self {
        Self { event: EventItem::default(), last: Reactive::Static(false) }
    }
}

/// One event ledger entry.
#[component]
pub fn EventRow(props: &EventRowProps) -> Element {
    let e = &props.event;
    let time = e.time.to_string();
    let kind = e.kind.to_string();
    let status = e.status;
    let title = e.title.clone();
    let body = e.body.clone();
    let has_body = !body.is_empty();
    let agent = e.agent.to_string();
    let tool = e.tool.to_string();
    let last = props.last.clone();
    ui! {
        view(style = EventGrid()) {
            view(style = TimeCell()) {
                Mono(content = time, size = MonoTextSize::Overline)
            }
            view(style = RailCell()) {
                StatusDot(status = status)
                if !last.get() {
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
    /// Console state handles.
    pub console: Console,
    /// The agent's name.
    pub agent: String,
}

/// One agent card in the roster column. Built once per name on the
/// roster; everything on it is read live, so an announcement moves
/// the words and the ring keeps turning.
#[component]
pub fn RosterCard(props: &RosterCardProps) -> Element {
    let data = props.console.data;
    let a = data.agent(props.agent.clone());
    let state = {
        let a = a.clone();
        memo(move || a().map(|a| a.state).unwrap_or_default())
    };
    // With a health check registered the tag answers "is the box
    // there?" — the question the roster exists for — and the claims
    // move to the lines under it. Without one, the tag is what the
    // ledger can say: running while it holds a claim.
    let dot = {
        let a = a.clone();
        memo(move || a().map(|a| a.health.as_ref().map(|h| h.tone).unwrap_or(a.state)).unwrap_or_default())
    };
    let live = {
        let a = a.clone();
        memo(move || {
            let now = data.clock.get();
            a().is_some_and(|a| a.live_at(now))
        })
    };
    let field = |pick: fn(&AgentRow) -> String| {
        let a = a.clone();
        rx!(a().map(|a| pick(&a)).unwrap_or_default())
    };
    let id = field(|a| a.id.clone());
    let scope = field(|a| a.scope.clone());
    let level = field(|a| a.level.clone());
    let uptime = field(|a| a.uptime.clone());
    let seen = field(|a| format!("seen {}", a.last_seen));
    let health_line = field(|a| a.health.as_ref().map(|h| h.line.clone()).unwrap_or_default());
    let word_at = field(|a| a.last_word.as_ref().map(|w| format!("{} on {}", w.at, w.subject)).unwrap_or_default());
    // The health tag: remade when the verdict changes, and only then.
    let health = {
        let a = a.clone();
        move || a().and_then(|a| a.health.as_ref().map(|h| (h.label.clone(), h.tone)))
    };
    let checked = {
        let health = health.clone();
        move || health().is_some()
    };
    // The last thing it said, in its own words — the pulse the roster
    // exists for once a box is known to be up.
    let word = {
        let a = a.clone();
        move || a().and_then(|a| a.last_word.as_ref().map(|w| format!("\u{201c}{}\u{201d}", w.text)))
    };
    // Held-but-quiet says so in words here as on the card (rule 30).
    let quiet = {
        let a = a.clone();
        move || {
            let now = data.clock.get();
            a().and_then(|a| a.quiet_at(now))
        }
    };
    let tag = switch(
        move || (health(), state.get()),
        move |(health, state): &(Option<(String, Status)>, Status)| match health {
            Some((label, tone)) => {
                let (label, tone) = (label.clone(), status_tone(*tone));
                ui! { Badge(label = label, tone = tone) }
            }
            None => {
                let state = *state;
                ui! { StatusBadge(status = state) }
            }
        },
    );
    ui! {
        view(style = RosterBox()) {
            Stack(axis = StackAxis::Row, gap = StackGap::Sm, align = StackAlign::Center) {
                StatusDot(status = dot, live = live)
                Mono(content = id)
                Spacer()
                tag
            }
            Typography(content = scope, kind = typography_kind::Caption, muted = true)
            if checked() {
                Mono(content = health_line.clone(), size = MonoTextSize::Overline)
            }
            match word() {
                Some(word) => {
                    view(style = WordBox()) {
                        text(style = WordText()) { word.clone() }
                        Mono(content = word_at.clone(), size = MonoTextSize::Overline)
                    }
                }
                None => {}
            }
            Stack(axis = StackAxis::Row, gap = StackGap::Sm, align = StackAlign::Center) {
                Mono(content = level, size = MonoTextSize::Overline)
                Spacer()
                match quiet() {
                    Some(quiet) => {
                        Mono(content = quiet.clone(), size = MonoTextSize::Overline, tone = crate::styles::MonoTextTone::Warning)
                    }
                    None => {}
                }
                Mono(content = uptime, size = MonoTextSize::Overline)
                Mono(content = seen, size = MonoTextSize::Overline)
            }
        }
    }
}

stylesheet! {
    pub WordBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: 2,
            padding_left: t.spacing.sm(),
            border_left_width: 2.0,
            border_color: t.color.border(),
        }
    }
}

stylesheet! {
    pub WordText<IdeaThemeRef> {
        base(t) {
            font_size: t.typography.caption_size(),
            color: t.color.text(),
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
