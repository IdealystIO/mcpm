//! The right-hand detail overlays. Two of them share one slot and one
//! shell: [`Drawer`] over a module (stats, task checklist, handoff
//! history, MCP call trace), and [`WantDrawer`] over one loose idea
//! (its tags, provenance, and every feature that absorbed it — with
//! the composing agent's rationale for each).
//!
//! Lists elsewhere in the console stay compact by pushing per-item
//! properties in here rather than spelling them out inline.

use idea_ui::{tone, typography_kind, variant, Badge, Grid, IdeaThemeRef, Spacer, Stack,
    StackAlign, StackAxis, StackGap, Tag, Typography};
use runtime_core::{
    component, presence, pressable, stylesheet, ui, AlignItems, Easing, Element, FlexDirection,
    FlexWrap, FontWeight, IdealystSchema, IntoElement, JustifyContent, Position, PresenceAnim,
    PresenceState, StyleApplication,
};

use crate::components::bits::{Mono, StatusBadge, StatusDot};
use crate::model::{features, wants};
use crate::state::Console;
use crate::styles::{MonoTextSize, MonoTextTone, SectionLabel};

/// Props for [`Drawer`].
#[derive(Default, IdealystSchema)]
pub struct DrawerProps {
    /// Console state handles.
    pub console: Console,
    /// Feature index.
    pub feature: usize,
    /// Stage index of the open module.
    pub stage: usize,
    /// Module index of the open module.
    pub module: usize,
}

/// The module drawer overlay: a backdrop that fades with the host, and
/// a panel that slides in over it.
#[component]
pub fn Drawer(props: &DrawerProps) -> Element {
    let console = props.console;
    let (fi, si, mi) = (props.feature, props.stage, props.module);
    let backdrop = pressable(Vec::new(), move || console.close_drawer())
        .with_style(StyleApplication::new(backdrop_sheet_style()))
        .into_element();
    let panel = panel_motion(
        move || ui! { ModulePanel(console = console, feature = fi, stage = si, module = mi) },
        move || console.selected.get().is_some(),
    );
    ui! {
        view(style = DrawerHostBox()) {
            backdrop
            panel
        }
    }
}

/// Props for [`ModulePanel`]. A props struct binds to exactly one
/// component, so the panel cannot reuse [`DrawerProps`].
#[derive(Default, IdealystSchema)]
pub struct ModulePanelProps {
    /// Console state handles.
    pub console: Console,
    /// Feature index.
    pub feature: usize,
    /// Stage index of the open module.
    pub stage: usize,
    /// Module index of the open module.
    pub module: usize,
}

/// The module panel's own contents. Split out from [`Drawer`] because
/// `presence` rebuilds its child through an `Fn` closure, which can
/// only capture `Copy` values — so the panel's markup has to live
/// behind props rather than in captured locals.
#[component]
pub fn ModulePanel(props: &ModulePanelProps) -> Element {
    let console = props.console;
    let (fi, si, mi) = (props.feature, props.stage, props.module);
    let feats = features();
    let f = &feats[fi];
    let m = &f.stages[si].modules[mi];

    let path = format!("{}  ▸  Stage {:02} {}  ▸  Module", f.name, si + 1, f.stages[si].name);
    let name = m.name.clone();
    let status = m.status;
    let agent = m.agent.to_string();
    let done = m.tasks.iter().filter(|t| t.done).count();
    let total = m.tasks.len();
    let added = m.tasks.iter().filter(|t| t.added).count();
    let in_stage = m.in_stage.to_string();
    let spawned = if m.spawned.is_empty() { "not spawned".to_string() } else { m.spawned.to_string() };
    let task_label = format!("{done} of {total} checked off");
    let block = m.block.clone();
    let tasks: Vec<(String, bool, bool)> =
        m.tasks.iter().map(|t| (t.label.to_string(), t.done, t.added)).collect();
    let handoffs: Vec<(String, String, String, String, bool)> = m
        .handoffs
        .iter()
        .map(|h| {
            (
                h.title.to_string(),
                h.body.to_string(),
                h.at.to_string(),
                h.from.to_string(),
                h.from == "server.gate",
            )
        })
        .collect();
    let trace: Vec<(String, String, String, bool)> = m
        .trace
        .iter()
        .map(|c| {
            let bad = c.result == "denied" || c.result.contains("fail");
            (c.at.to_string(), c.tool.to_string(), c.result.to_string(), bad)
        })
        .collect();
    let has_trace = !trace.is_empty();

    let close = pressable(
        vec![ui! { text(style = CloseGlyph()) { "×" } }],
        move || console.close_drawer(),
    )
    .with_style(StyleApplication::new(close_box_style()))
    .into_element();

    ui! {
        view(style = PanelBox()) {
                scroll_view(style = PanelScroll()) {
                    view(style = PanelCol()) {
                        Stack(axis = StackAxis::Row, gap = StackGap::Md, align = StackAlign::Start) {
                            view(style = HeadCol()) {
                                Mono(content = path, size = MonoTextSize::Overline)
                                Typography(
                                    content = name,
                                    kind = typography_kind::H3,
                                    weight = Some(FontWeight::SemiBold),
                                )
                                Stack(axis = StackAxis::Row, gap = StackGap::Sm, align = StackAlign::Center) {
                                    StatusBadge(status = status)
                                    Mono(content = agent, size = MonoTextSize::Overline)
                                }
                            }
                            Spacer()
                            close
                        }

                        Grid(columns = 2u32, gap = StackGap::Xs) {
                            StatCell(label = "time in stage", value = in_stage)
                            StatCell(label = "spawned", value = spawned)
                            StatCell(label = "tasks done", value = format!("{done} / {total}"))
                            StatCell(label = "agent-added", value = format!("{added}"))
                        }

                        if let Some((title, body)) = block {
                            view(style = BlockBox()) {
                                text(style = BlockTitle()) { title }
                                text(style = BlockBody()) { body }
                            }
                        }

                        view(style = SectionCol()) {
                            Stack(axis = StackAxis::Row, align = StackAlign::Center) {
                                text(style = SectionLabel()) { "Tasks" }
                                Spacer()
                                Typography(content = task_label, kind = typography_kind::Caption, muted = true)
                            }
                            for (label, task_done, task_added) in tasks {
                                TaskRow(label = label, done = task_done, added = task_added)
                            }
                        }

                        view(style = SectionCol()) {
                            text(style = SectionLabel()) { "Handoff history" }
                            for (title, body, at, from, gate) in handoffs {
                                HandoffRow(title = title, body = body, at = at, from = from, gate = gate)
                            }
                        }

                        if has_trace {
                            view(style = SectionCol()) {
                                text(style = SectionLabel()) { "MCP calls" }
                                view(style = TraceBox()) {
                                    for (at, tool, result, bad) in trace {
                                        TraceRow(at = at, tool = tool, result = result, bad = bad)
                                    }
                                }
                            }
                        }
                    }
                }
        }
    }
}

/// Props for [`WantDrawer`].
#[derive(Default, IdealystSchema)]
pub struct WantDrawerProps {
    /// Console state handles.
    pub console: Console,
    /// Index into [`crate::model::wants`], resolved from the held id by
    /// `app()` on every rebuild.
    pub want: usize,
}

/// The want drawer overlay: backdrop plus the sliding panel.
#[component]
pub fn WantDrawer(props: &WantDrawerProps) -> Element {
    let console = props.console;
    let index = props.want;
    let backdrop = pressable(Vec::new(), move || console.close_drawer())
        .with_style(StyleApplication::new(backdrop_sheet_style()))
        .into_element();
    let panel = panel_motion(
        move || ui! { WantPanel(console = console, want = index) },
        move || console.want.get().is_some(),
    );
    ui! {
        view(style = DrawerHostBox()) {
            backdrop
            panel
        }
    }
}

/// Props for [`WantPanel`].
#[derive(Default, IdealystSchema)]
pub struct WantPanelProps {
    /// Console state handles.
    pub console: Console,
    /// Index into [`crate::model::wants`].
    pub want: usize,
}

/// Everything about one loose idea, so the lists that point at it (the
/// pool, a feature's "Composed from") can stay a single line each.
#[component]
pub fn WantPanel(props: &WantPanelProps) -> Element {
    let console = props.console;
    let index = props.want;
    let pool = wants();
    let w = &pool[index];
    let id = w.id.clone();
    let body = w.body.clone();
    let state = w.state;
    let status = state.status();
    let author = w.author.clone();
    let captured = w.captured.clone();
    let note = w.note.clone();
    let has_note = !note.is_empty();
    let tag_count = w.tags.len();
    let link_count = w.features.len();
    let has_links = link_count > 0;
    let state_word = match state {
        crate::model::WantState::Open => "Loose",
        crate::model::WantState::Promoted => "Composed",
        crate::model::WantState::Declined => "Declined",
    };

    let close = pressable(
        vec![ui! { text(style = CloseGlyph()) { "\u{d7}" } }],
        move || console.close_drawer(),
    )
    .with_style(StyleApplication::new(close_box_style()))
    .into_element();

    ui! {
        view(style = PanelBox()) {
                scroll_view(style = PanelScroll()) {
                    view(style = PanelCol()) {
                        Stack(axis = StackAxis::Row, gap = StackGap::Md, align = StackAlign::Start) {
                            view(style = HeadCol()) {
                                Mono(content = id, size = MonoTextSize::Overline)
                                Typography(
                                    content = body,
                                    kind = typography_kind::H3,
                                    weight = Some(FontWeight::SemiBold),
                                )
                                Stack(axis = StackAxis::Row, gap = StackGap::Sm, align = StackAlign::Center) {
                                    StatusDot(status = status)
                                    Mono(content = state_word, size = MonoTextSize::Overline)
                                }
                            }
                            Spacer()
                            close
                        }

                        if tag_count > 0 {
                            view(style = WantTagRow()) {
                                for i in 0..tag_count {
                                    WantDrawerTag(want = index, index = i)
                                }
                            }
                        }

                        Grid(columns = 2u32, gap = StackGap::Xs) {
                            StatCell(label = "captured", value = captured)
                            StatCell(label = "by", value = author)
                        }

                        if has_note {
                            view(style = BlockBox()) {
                                text(style = BlockTitle()) { "Declined because" }
                                text(style = BlockBody()) { note }
                            }
                        }

                        if has_links {
                            view(style = SectionCol()) {
                                text(style = SectionLabel()) { "Composed into" }
                                for i in 0..link_count {
                                    WantLinkRow(want = index, index = i)
                                }
                            }
                        }
                    }
                }
        }
    }
}

/// Props for [`WantDrawerTag`].
#[derive(Default, IdealystSchema)]
pub struct WantDrawerTagProps {
    /// Index into [`crate::model::wants`].
    pub want: usize,
    /// Index into that want's tag list.
    pub index: usize,
}

/// One filing label, re-read from the model because the `ui!` for-each
/// body is an `Fn` closure and cannot consume a captured `String`.
#[component]
pub fn WantDrawerTag(props: &WantDrawerTagProps) -> Element {
    let pool = wants();
    let tag = pool[props.want].tags[props.index].clone();
    ui! {
        Tag(label = tag, tone = tone::Neutral, variant = variant::Soft)
    }
}

/// Props for [`WantLinkRow`].
#[derive(Default, IdealystSchema)]
pub struct WantLinkRowProps {
    /// Index into [`crate::model::wants`].
    pub want: usize,
    /// Index into that want's feature links.
    pub index: usize,
}

/// One want\u{2192}feature composition: the feature that absorbed the idea,
/// and the rationale the composing agent recorded for reading it in
/// that way. This is the only place that rationale is spelled out.
#[component]
pub fn WantLinkRow(props: &WantLinkRowProps) -> Element {
    let pool = wants();
    let (name, rationale) = &pool[props.want].features[props.index];
    let name = name.clone();
    let rationale = rationale.clone();
    let has_rationale = !rationale.is_empty();
    ui! {
        view(style = WantLinkBox()) {
            Typography(
                content = name,
                kind = typography_kind::BodySm,
                weight = Some(FontWeight::SemiBold),
            )
            if has_rationale {
                text(style = WantLinkRationale()) { rationale }
            }
        }
    }
}

/// Wrap a drawer panel in its slide-in motion. The backdrop is left to
/// the host's fade — a full-bleed sheet that translates would show a
/// bare strip down one edge while it moves.
fn panel_motion(
    build: impl Fn() -> Element + 'static,
    present: impl Fn() -> bool + 'static,
) -> Element {
    presence(build)
        .present(present)
        .enter(PresenceAnim::new(
            PresenceState::rest().translate_x(PANEL_SLIDE_PX),
            crate::app::BACKDROP_IN_MS,
            Easing::EaseOut,
        ))
        .exit(PresenceAnim::new(
            PresenceState::rest().translate_x(PANEL_SLIDE_PX),
            crate::app::BACKDROP_OUT_MS,
            Easing::EaseIn,
        ))
        .into_element()
}

/// How far the panel travels on its way in, in px. Enough to read as
/// motion from the right edge, short enough not to feel like travel.
const PANEL_SLIDE_PX: f32 = 32.0;

/// Props for [`StatCell`].
#[derive(Default, IdealystSchema)]
pub struct StatCellProps {
    /// Uppercase stat label.
    pub label: &'static str,
    /// Stat value.
    pub value: String,
}

/// One cell of the drawer's stat grid.
#[component]
pub fn StatCell(props: &StatCellProps) -> Element {
    let label = props.label;
    let value = props.value.clone();
    ui! {
        view(style = StatBox()) {
            text(style = SectionLabel()) { label }
            Typography(
                content = value,
                kind = typography_kind::Body,
                weight = Some(FontWeight::SemiBold),
            )
        }
    }
}

/// Props for [`TaskRow`].
#[derive(Default, IdealystSchema)]
pub struct TaskRowProps {
    /// Task text.
    pub label: String,
    /// Checked off?
    pub done: bool,
    /// Worker-discovered (`origin: discovered`)?
    pub added: bool,
}

/// One checklist row in the drawer.
#[component]
pub fn TaskRow(props: &TaskRowProps) -> Element {
    let label = props.label.clone();
    let done = props.done;
    let added = props.added;
    let box_style = TaskBoxSheet().done(if done { TaskBoxSheetDone::Yes } else { TaskBoxSheetDone::No });
    let text_style = TaskText().done(if done { TaskTextDone::Yes } else { TaskTextDone::No });
    ui! {
        view(style = TaskRowBox()) {
            view(style = box_style) {
                if done {
                    text(style = TaskMark()) { "✓" }
                }
            }
            view(style = TaskLabelCell()) {
                text(style = text_style) { label }
            }
            if added {
                Badge(label = "agent-added", tone = tone::Info)
            }
        }
    }
}

/// Props for [`HandoffRow`].
#[derive(Default, IdealystSchema)]
pub struct HandoffRowProps {
    /// Handoff title.
    pub title: String,
    /// Handoff body.
    pub body: String,
    /// Timestamp.
    pub at: String,
    /// Originating agent.
    pub from: String,
    /// Whether the origin is the server gate (danger dot).
    pub gate: bool,
}

/// One handoff-history entry.
#[component]
pub fn HandoffRow(props: &HandoffRowProps) -> Element {
    let title = props.title.clone();
    let body = props.body.clone();
    let has_body = !body.is_empty();
    let at = props.at.clone();
    let from = props.from.clone();
    let dot = HandoffDot().tone(if props.gate {
        HandoffDotTone::Gate
    } else {
        HandoffDotTone::Agent
    });
    ui! {
        view(style = HandoffGrid()) {
            view(style = HandoffRail()) {
                view(style = dot) {}
                view(style = HandoffLine()) {}
            }
            view(style = HandoffBody()) {
                Typography(
                    content = title,
                    kind = typography_kind::BodySm,
                    weight = Some(FontWeight::SemiBold),
                )
                if has_body {
                    Typography(content = body, kind = typography_kind::Caption, muted = true)
                }
                Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
                    Mono(content = at, size = MonoTextSize::Overline)
                    Mono(content = from, size = MonoTextSize::Overline)
                }
            }
        }
    }
}

/// Props for [`TraceRow`].
#[derive(Default, IdealystSchema)]
pub struct TraceRowProps {
    /// Timestamp.
    pub at: String,
    /// MCP tool invoked.
    pub tool: String,
    /// Result label ("ok", "denied", "2 fail").
    pub result: String,
    /// Whether the result is a failure/denial.
    pub bad: bool,
}

/// One MCP trace row.
#[component]
pub fn TraceRow(props: &TraceRowProps) -> Element {
    let at = props.at.clone();
    let tool = props.tool.clone();
    let result = props.result.clone();
    let tone = if props.bad { MonoTextTone::Danger } else { MonoTextTone::Success };
    ui! {
        view(style = TraceRowBox()) {
            Mono(content = at, size = MonoTextSize::Overline)
            view(style = TraceToolCell()) {
                Mono(content = tool, size = MonoTextSize::Overline, tone = MonoTextTone::Info)
            }
            Mono(content = result, size = MonoTextSize::Overline, tone = tone)
        }
    }
}

stylesheet! {
    pub DrawerHostBox<IdeaThemeRef> {
        base(t) {
            position: Position::Absolute,
            top: 0,
            left: 0,
            right: 0,
            bottom: 0,
        }
    }
}

stylesheet! {
    pub BackdropSheet<IdeaThemeRef> {
        base(t) {
            position: Position::Absolute,
            top: 0,
            left: 0,
            right: 0,
            bottom: 0,
            background: t.color.overlay(),
        }
    }
}

stylesheet! {
    pub PanelBox<IdeaThemeRef> {
        base(t) {
            position: Position::Absolute,
            top: 0,
            right: 0,
            bottom: 0,
            width: 420,
            max_width: runtime_core::Length::Percent(92.0),
            border_left_width: 1.0,
            border_color: t.color.border(),
            background: t.color.surface(),
            flex_direction: FlexDirection::Column,
        }
    }
}

stylesheet! {
    pub PanelScroll<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            min_height: 0,
        }
    }
}

stylesheet! {
    pub PanelCol<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.lg(),
            padding_top: t.spacing.lg(),
            padding_horizontal: t.spacing.lg(),
            padding_bottom: t.spacing.xxl(),
        }
    }
}

stylesheet! {
    pub HeadCol<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
            min_width: 0,
        }
    }
}

stylesheet! {
    pub CloseBox<IdeaThemeRef> {
        base(t) {
            width: 28,
            height: 28,
            border_radius: t.radius.sm(),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
        }
        state hovered(t) {
            background: t.color.surface_alt(),
        }
    }
}

stylesheet! {
    pub CloseGlyph<IdeaThemeRef> {
        base(t) {
            font_size: t.typography.body_lg_size(),
            color: t.color.text_muted(),
        }
    }
}

stylesheet! {
    pub StatBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: 2,
            padding: t.spacing.md(),
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.sm(),
            background: t.color.surface(),
        }
    }
}

stylesheet! {
    pub BlockBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
            padding: t.spacing.md(),
            border_width: 1.0,
            border_color: t.intent.danger.border(),
            border_radius: t.radius.md(),
            background: t.intent.danger.soft_bg(),
        }
    }
}

stylesheet! {
    pub BlockTitle<IdeaThemeRef> {
        base(t) {
            font_size: t.typography.body_sm_size(),
            font_weight: FontWeight::SemiBold,
            color: t.intent.danger.soft_text(),
        }
    }
}

stylesheet! {
    pub BlockBody<IdeaThemeRef> {
        base(t) {
            font_size: t.typography.caption_size(),
            color: t.intent.danger.soft_text(),
            opacity: 0.9,
        }
    }
}

stylesheet! {
    pub SectionCol<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
        }
    }
}

stylesheet! {
    pub TaskRowBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexStart,
            gap: t.spacing.sm(),
            padding_vertical: 9,
            border_bottom_width: 1.0,
            border_color: t.color.border(),
        }
    }
}

stylesheet! {
    pub TaskBoxSheet<IdeaThemeRef> {
        base(t) {
            width: 15,
            height: 15,
            flex_shrink: 0.0,
            margin_top: 1,
            border_width: 1.0,
            border_radius: t.radius.sm(),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
        }
        variant done {
            #[default]
            no(t) {
                border_color: t.color.border_strong(),
                background: t.color.surface(),
            }
            yes(t) {
                border_color: t.intent.success.solid_bg(),
                background: t.intent.success.solid_bg(),
            }
        }
    }
}

stylesheet! {
    pub TaskMark<IdeaThemeRef> {
        base(t) {
            font_size: 10,
            color: t.intent.success.solid_text(),
        }
    }
}

stylesheet! {
    pub TaskLabelCell<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            min_width: 0,
        }
    }
}

stylesheet! {
    pub TaskText<IdeaThemeRef> {
        base(t) {
            font_size: t.typography.body_sm_size(),
        }
        variant done {
            #[default]
            no(t) { color: t.color.text() }
            yes(t) {
                color: t.color.text_muted(),
                strikethrough: true,
            }
        }
    }
}

stylesheet! {
    pub HandoffGrid<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            gap: t.spacing.md(),
            align_items: AlignItems::Stretch,
        }
    }
}

stylesheet! {
    pub HandoffRail<IdeaThemeRef> {
        base(t) {
            width: 16,
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
        }
    }
}

stylesheet! {
    pub HandoffDot<IdeaThemeRef> {
        base(t) {
            width: 9,
            height: 9,
            border_radius: t.radius.pill(),
            margin_top: 4,
        }
        variant tone {
            #[default]
            agent(t) { background: t.intent.primary.fg() }
            gate(t) { background: t.intent.danger.fg() }
        }
    }
}

stylesheet! {
    pub HandoffLine<IdeaThemeRef> {
        base(t) {
            width: 1,
            flex_grow: 1.0,
            min_height: 8,
            margin_top: 4,
            background: t.color.border(),
        }
    }
}

stylesheet! {
    pub HandoffBody<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            min_width: 0,
            flex_direction: FlexDirection::Column,
            gap: 3,
            padding_bottom: t.spacing.md(),
        }
    }
}

stylesheet! {
    pub TraceBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.md(),
            overflow: runtime_core::Overflow::Hidden,
        }
    }
}

stylesheet! {
    pub TraceRowBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
            padding_vertical: t.spacing.sm(),
            padding_horizontal: 10,
            border_top_width: 1.0,
            border_color: t.color.border(),
            background: t.color.surface(),
        }
    }
}

stylesheet! {
    pub TraceToolCell<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            min_width: 0,
        }
    }
}

stylesheet! {
    pub WantTagRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            gap: t.spacing.xs(),
        }
    }
}

stylesheet! {
    pub WantLinkBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
            padding_vertical: t.spacing.sm(),
            padding_horizontal: t.spacing.md(),
            border_radius: t.radius.sm(),
            background: t.color.surface_alt(),
        }
    }
}

stylesheet! {
    pub WantLinkRationale<IdeaThemeRef> {
        base(t) {
            font_size: t.typography.caption_size(),
            color: t.color.text_muted(),
        }
    }
}
