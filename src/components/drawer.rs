//! The right-hand detail overlays. Two of them share one slot and one
//! shell: [`Drawer`] over a module (stats, prerequisites, task
//! checklist, handoff document, ledger history), and [`WantDrawer`] over one loose idea
//! (its tags, provenance, and every feature that absorbed it — with
//! the composing agent's rationale for each).
//!
//! Lists elsewhere in the console stay compact by pushing per-item
//! properties in here rather than spelling them out inline.

use idea_ui::{tone, typography_kind, variant, Badge, Grid, IdeaThemeRef, Spacer, Stack,
    StackAlign, StackAxis, StackGap, Tag, Typography};
use runtime_core::{
    component, presence, pressable, stylesheet, ui, AlignItems, Cursor, Easing, Element,
    FlexDirection, FlexWrap, FontWeight, IdealystSchema, IntoElement, JustifyContent, Position,
    PresenceAnim, PresenceState, StyleApplication,
};

use crate::components::bits::{Mono, StatusBadge, StatusDot};
use crate::components::document::DocumentView;
use crate::components::edits::{module_entries, want_entries, ActionMenu};
use crate::model::{features, module_detail, want_by_id};
use crate::state::{Console, Edit};
use crate::styles::{MonoTextSize, MonoTextTone, SectionLabel};

/// Props for [`Drawer`].
#[derive(Default, IdealystSchema)]
pub struct DrawerProps {
    /// Console state handles.
    pub console: Console,
    /// Feature index.
    pub feature: usize,
    /// Index of the open module in [`crate::model::Feature::modules`].
    pub module: usize,
}

/// The module drawer overlay: a backdrop that fades with the host, and
/// a panel that slides in over it.
#[component]
pub fn Drawer(props: &DrawerProps) -> Element {
    let console = props.console;
    let (fi, mi) = (props.feature, props.module);
    let backdrop = pressable(Vec::new(), move || console.close_drawer())
        .with_style(StyleApplication::new(backdrop_sheet_style()))
        .into_element();
    let panel = panel_motion(
        move || ui! { ModulePanel(console = console, feature = fi, module = mi) },
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
    /// Index of the open module in [`crate::model::Feature::modules`].
    pub module: usize,
}

/// The module panel's own contents. Split out from [`Drawer`] because
/// `presence` rebuilds its child through an `Fn` closure, which can
/// only capture `Copy` values — so the panel's markup has to live
/// behind props rather than in captured locals.
#[component]
pub fn ModulePanel(props: &ModulePanelProps) -> Element {
    let console = props.console;
    let (fi, mi) = (props.feature, props.module);
    let feats = features();
    let f = &feats[fi];
    let m = &f.modules[mi];
    let feature_id = f.id.clone();
    let module_id = m.id.clone();
    let entries = module_entries(&feature_id, &module_id);

    let path = format!("{}  \u{25b8}  Module", f.name);
    let name = m.name.clone();
    let status = m.status;
    let agent = m.agent.to_string();
    let description = m.description.trim().to_string();
    let has_description = !description.is_empty();
    let done = m.tasks.iter().filter(|t| t.done).count();
    let total = m.tasks.len();
    let added = m.tasks.iter().filter(|t| t.added).count();
    let column = format!("column {}", m.depth);
    let spawned = if m.spawned.is_empty() { "not spawned".to_string() } else { m.spawned.to_string() };
    let task_label = format!("{done} of {total} checked off");
    let block = m.block.clone();
    // Each prerequisite by its index, so its row opens the same drawer
    // onto it; `waiting` marks the ones still holding this module back.
    let prereqs: Vec<(usize, bool)> = m
        .depends_on
        .iter()
        .filter_map(|id| f.module_index(id))
        .map(|i| (i, m.waiting_on.iter().any(|w| *w == f.modules[i].id)))
        .collect();
    let has_prereqs = !prereqs.is_empty();
    let owns: Vec<String> = m.owns.clone();
    let has_owns = !owns.is_empty();
    let tasks: Vec<(String, String, bool, bool)> = m
        .tasks
        .iter()
        .map(|t| (t.id.clone(), t.label.to_string(), t.done, t.added))
        .collect();
    let summary = m.summary.clone().unwrap_or_default();
    let has_summary = !summary.is_empty();
    // The handoff and history are their own read, fetched when the
    // drawer opens; until it lands both sections say so rather than
    // claiming there is nothing.
    let detail = module_detail(&m.id);
    let detail_loaded = detail.is_some();
    let (handoff_present, handoff_body, handoff_meta) =
        match detail.as_ref().and_then(|d| d.handoff.as_ref()) {
            Some(d) => (true, d.body.clone(), d.meta()),
            None => (false, String::new(), String::new()),
        };
    let handoff_empty = if detail_loaded { "No handoff written." } else { "Loading\u{2026}" };
    let history: Vec<(String, String, String, String, bool)> = detail
        .as_ref()
        .map(|d| {
            d.history
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
                .collect()
        })
        .unwrap_or_default();
    let has_history = !history.is_empty();

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
                            ActionMenu(console = console, id = format!("module:{module_id}"), entries = entries.clone())
                            close
                        }

                        if has_description {
                            Typography(content = description, kind = typography_kind::BodySm, muted = true)
                        }

                        Grid(columns = 2u32, gap = StackGap::Xs) {
                            StatCell(label = "depth", value = column)
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
                            text(style = SectionLabel()) { "Prerequisites" }
                            if !has_prereqs {
                                Typography(content = "None.", kind = typography_kind::BodySm, muted = true)
                            }
                            for (index, waiting) in prereqs {
                                PrereqRow(
                                    console = console,
                                    feature = fi,
                                    module = index,
                                    waiting = waiting,
                                    dependent = module_id.clone(),
                                )
                            }
                        }

                        if has_owns {
                            view(style = SectionCol()) {
                                text(style = SectionLabel()) { "Owns" }
                                for path in owns {
                                    Mono(content = path, tone = MonoTextTone::Text)
                                }
                            }
                        }

                        view(style = SectionCol()) {
                            Stack(axis = StackAxis::Row, align = StackAlign::Center) {
                                text(style = SectionLabel()) { "Tasks" }
                                Spacer()
                                Typography(content = task_label, kind = typography_kind::Caption, muted = true)
                            }
                            for (task_id, label, task_done, task_added) in tasks {
                                TaskRow(
                                    console = console,
                                    feature = feature_id.clone(),
                                    task = task_id,
                                    label = label,
                                    done = task_done,
                                    added = task_added,
                                )
                            }
                        }

                        if has_summary {
                            view(style = SectionCol()) {
                                text(style = SectionLabel()) { "Summary" }
                                Typography(content = summary, kind = typography_kind::BodySm)
                            }
                        }

                        view(style = SectionCol()) {
                            text(style = SectionLabel()) { "Handoff" }
                            DocumentView(
                                console = console,
                                present = handoff_present,
                                body = handoff_body,
                                meta = handoff_meta,
                                empty = handoff_empty,
                            )
                        }

                        if has_history {
                            view(style = SectionCol()) {
                                text(style = SectionLabel()) { "History" }
                                for (title, body, at, from, gate) in history {
                                    HistoryRow(title = title, body = body, at = at, from = from, gate = gate)
                                }
                            }
                        }
                    }
                }
        }
    }
}

/// Props for [`PrereqRow`].
#[derive(Default, IdealystSchema)]
pub struct PrereqRowProps {
    /// Console state handles.
    pub console: Console,
    /// Feature index.
    pub feature: usize,
    /// The prerequisite's index in [`crate::model::Feature::modules`].
    pub module: usize,
    /// Whether this prerequisite is still holding the open module back.
    pub waiting: bool,
    /// The open module — the one that waits — by id, for the remove.
    pub dependent: String,
}

/// One prerequisite of the open module. Pressing it moves the drawer
/// onto that module — one detail surface per entity, reached from
/// wherever it appears (rule 20).
#[component]
pub fn PrereqRow(props: &PrereqRowProps) -> Element {
    let console = props.console;
    let index = props.module;
    let feats = features();
    let Some(m) = feats.get(props.feature).and_then(|f| f.modules.get(index)) else {
        return ui! { view {} };
    };
    let name = m.name.clone();
    let status = m.status;
    let waiting = props.waiting;
    let id = m.id.clone();
    let edge = Edit::RemoveDependency {
        feature: feats[props.feature].id.clone(),
        module: props.dependent.clone(),
        depends_on: id.clone(),
    };
    let remove = pressable(
        vec![ui! { text(style = RemoveGlyph()) { "\u{d7}" } }],
        move || crate::components::edits::choose(console, edge.clone()),
    )
    .with_style(StyleApplication::new(remove_box_style()))
    .into_element();

    let inner: Element = ui! {
        view(style = PrereqInner()) {
            StatusDot(status = status)
            view(style = TaskLabelCell()) {
                Typography(content = name, kind = typography_kind::BodySm)
            }
            if waiting {
                Mono(content = "waiting", size = MonoTextSize::Overline, tone = MonoTextTone::Warning)
            }
            text(style = PrereqChevron()) { "\u{203a}" }
        }
    };
    let row = pressable(vec![inner], move || console.open_module(&id))
        .with_style(StyleApplication::new(prereq_box_style()))
        .into_element();
    ui! {
        view(style = PrereqLine()) {
            row
            remove
        }
    }
}

/// Props for [`WantDrawer`].
#[derive(Default, IdealystSchema)]
pub struct WantDrawerProps {
    /// Console state handles.
    pub console: Console,
    /// The want's id. Resolved through [`crate::model::want_by_id`] on
    /// every rebuild — the pool is paged under the reader, so nothing
    /// here holds a position in it.
    pub want: String,
}

/// The want drawer overlay: backdrop plus the sliding panel.
#[component]
pub fn WantDrawer(props: &WantDrawerProps) -> Element {
    let console = props.console;
    let id = props.want.clone();
    let backdrop = pressable(Vec::new(), move || console.close_drawer())
        .with_style(StyleApplication::new(backdrop_sheet_style()))
        .into_element();
    let panel = panel_motion(
        move || ui! { WantPanel(console = console, want = id.clone()) },
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
    /// The want's id.
    pub want: String,
}

/// Everything about one loose idea, so the lists that point at it (the
/// pool, a feature's "Composed from") can stay a single line each.
#[component]
pub fn WantPanel(props: &WantPanelProps) -> Element {
    let console = props.console;
    let Some(w) = want_by_id(&props.want) else {
        return ui! { view {} };
    };
    let id = w.id.clone();
    // The rows below re-read the want by id inside `Fn` closures —
    // one clone per for-each, since each closure takes its own.
    let tag_id = id.clone();
    let link_id = id.clone();
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
    let entries = want_entries(&w);
    let menu_id = format!("want:{id}");

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
                            ActionMenu(console = console, id = menu_id.clone(), entries = entries.clone())
                            close
                        }

                        if tag_count > 0 {
                            view(style = WantTagRow()) {
                                for i in 0..tag_count {
                                    WantDrawerTag(want = tag_id.clone(), index = i)
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
                                    WantLinkRow(want = link_id.clone(), index = i)
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
    /// The want's id.
    pub want: String,
    /// Index into that want's tag list.
    pub index: usize,
}

/// One filing label, re-read from the model because the `ui!` for-each
/// body is an `Fn` closure and cannot consume a captured `String`.
#[component]
pub fn WantDrawerTag(props: &WantDrawerTagProps) -> Element {
    let tag = want_by_id(&props.want)
        .and_then(|w| w.tags.get(props.index).cloned())
        .unwrap_or_default();
    ui! {
        Tag(label = tag, tone = tone::Neutral, variant = variant::Soft)
    }
}

/// Props for [`WantLinkRow`].
#[derive(Default, IdealystSchema)]
pub struct WantLinkRowProps {
    /// The want's id.
    pub want: String,
    /// Index into that want's feature links.
    pub index: usize,
}

/// One want\u{2192}feature composition: the feature that absorbed the idea,
/// and the rationale the composing agent recorded for reading it in
/// that way. This is the only place that rationale is spelled out.
#[component]
pub fn WantLinkRow(props: &WantLinkRowProps) -> Element {
    let (name, rationale) = want_by_id(&props.want)
        .and_then(|w| w.features.get(props.index).cloned())
        .unwrap_or_default();
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
            PresenceState::rest().translate_x(PANEL_SLIDE_PX).opacity(0.0),
            crate::app::BACKDROP_IN_MS,
            Easing::EaseOut,
        ))
        .exit(PresenceAnim::new(
            PresenceState::rest().translate_x(PANEL_SLIDE_PX).opacity(0.0),
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
    /// Console state handles.
    pub console: Console,
    /// The feature the task's module is in.
    pub feature: String,
    /// The task's id.
    pub task: String,
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
    let console = props.console;
    let label = props.label.clone();
    let done = props.done;
    let added = props.added;
    // A resolved task is history and cannot be removed; the control
    // is absent rather than present and refused.
    let edit = Edit::RemoveTask { feature: props.feature.clone(), task: props.task.clone() };
    let remove: Option<Element> = (!done).then(|| {
        pressable(
            vec![ui! { text(style = RemoveGlyph()) { "\u{d7}" } }],
            move || crate::components::edits::choose(console, edit.clone()),
        )
        .with_style(StyleApplication::new(remove_box_style()))
        .into_element()
    });
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
            if let Some(control) = remove {
                control
            }
        }
    }
}

stylesheet! {
    pub PrereqLine<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.xs(),
        }
    }
}

stylesheet! {
    pub RemoveBox<IdeaThemeRef> {
        base(t) {
            width: 24,
            height: 24,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            border_radius: t.radius.sm(),
            cursor: Cursor::Pointer,
            flex_shrink: 0.0,
        }
        transitions {
            background: 120ms EaseOut,
        }
        state hovered(t) {
            background: t.color.surface_alt(),
        }
    }
}

stylesheet! {
    pub RemoveGlyph<IdeaThemeRef> {
        base(t) {
            font_size: 15,
            color: t.color.text_muted(),
        }
    }
}

/// Props for [`HistoryRow`].
#[derive(Default, IdealystSchema)]
pub struct HistoryRowProps {
    /// Event title.
    pub title: String,
    /// Event body.
    pub body: String,
    /// Timestamp.
    pub at: String,
    /// Originating agent.
    pub from: String,
    /// Whether the origin is the server gate (danger dot).
    pub gate: bool,
}

/// One ledger entry that touched the open module.
#[component]
pub fn HistoryRow(props: &HistoryRowProps) -> Element {
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
    pub PrereqBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            border_bottom_width: 1.0,
            border_color: t.color.border(),
            cursor: runtime_core::Cursor::Pointer,
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
    pub PrereqInner<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
            padding_vertical: 9,
            min_width: 0,
        }
    }
}

stylesheet! {
    pub PrereqChevron<IdeaThemeRef> {
        base(t) {
            font_size: t.typography.body_size(),
            color: t.color.text_muted(),
            flex_shrink: 0.0,
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
