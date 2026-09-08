//! Module cards — the full board card and the compact graph row. Both
//! are pressables that open the module drawer.

use idea_ui::{typography_kind, IdeaThemeRef, Spacer, Stack, StackAlign, StackAxis, StackGap,
    Typography};
use runtime_core::{
    component, pressable, stylesheet, ui, AlignItems, Element, FlexDirection, FontWeight,
    IdealystSchema, IntoElement, StyleApplication,
};

use crate::components::bits::{Mono, StatusBadge, StatusDot, Ticks};
use crate::model::{features, Status};
use crate::state::Console;
use crate::styles::{MonoTextSize, MonoTextTone};

/// Props for [`ModuleCard`] and [`ModuleRow`].
#[derive(Default, IdealystSchema)]
pub struct ModuleCardProps {
    /// Console state handles.
    pub console: Console,
    /// Feature index.
    pub feature: usize,
    /// Stage index within the feature.
    pub stage: usize,
    /// Module index within the stage.
    pub module: usize,
}

/// The card's pressable shell, shared by the board card and the graph
/// row.
///
/// The style is a CLOSURE and not a value: selecting a module writes
/// `Console::selected`, and reading it here means the highlight lands
/// on this one node instead of rebuilding the board that contains it.
/// That is what lets the border transition play at all — a rebuilt node
/// has no previous colour to move from — and it is why the pane's
/// `switch` no longer keys on the selection.
fn module_pressable(
    console: Console,
    si: usize,
    mi: usize,
    status: Status,
    inner: Element,
) -> Element {
    let selection = console.selected;
    let dim = matches!(status, Status::Blocked | Status::Queued);
    pressable(vec![inner], move || console.open_module(si, mi))
        .with_style(move || {
            let arm = accent_arm(status, selection.get() == Some((si, mi)));
            StyleApplication::new(module_box_style())
                .with("accent", arm.to_string())
                .with("dim", if dim { "yes" } else { "no" }.to_string())
        })
        .into_element()
}

/// Accent arm for a module surface, from status + selection.
fn accent_arm(status: Status, selected: bool) -> &'static str {
    if selected {
        "selected"
    } else {
        match status {
            Status::Running => "running",
            Status::Violation => "violation",
            _ => "plain",
        }
    }
}

/// The full board card for one module.
#[component]
pub fn ModuleCard(props: &ModuleCardProps) -> Element {
    let console = props.console;
    let (fi, si, mi) = (props.feature, props.stage, props.module);
    let feats = features();
    let m = &feats[fi].stages[si].modules[mi];

    let name = m.name.clone();
    let status = m.status;
    let agent = m.agent.to_string();
    let task_label = m.task_label();
    let ticks: Vec<bool> = m.tasks.iter().map(|t| t.done).collect();
    let now = m.now.clone();

    let inner: Element = ui! {
        view(style = ModuleInner()) {
            Stack(axis = StackAxis::Row, gap = StackGap::Sm, align = StackAlign::Center) {
                StatusDot(status = status)
                Typography(
                    content = name,
                    kind = typography_kind::BodySm,
                    weight = Some(FontWeight::SemiBold),
                )
                Spacer()
                StatusBadge(status = status)
            }
            Stack(axis = StackAxis::Row, align = StackAlign::Center) {
                Mono(content = agent, size = MonoTextSize::Overline)
                Spacer()
                Mono(content = task_label, size = MonoTextSize::Overline)
            }
            Ticks(ticks = ticks)
            if let Some((tool, doing)) = now {
                view(style = NowRow()) {
                    Mono(content = tool.to_string(), tone = MonoTextTone::Info)
                    Typography(content = doing, kind = typography_kind::Caption, muted = true)
                }
            }
        }
    };

    module_pressable(console, si, mi, status, inner)
}

/// Props for [`ModuleRow`] — same shape as [`ModuleCardProps`], its own
/// type because each `#[component]` owns its props' dispatch impl.
#[derive(Default, IdealystSchema)]
pub struct ModuleRowProps {
    /// Console state handles.
    pub console: Console,
    /// Feature index.
    pub feature: usize,
    /// Stage index within the feature.
    pub stage: usize,
    /// Module index within the stage.
    pub module: usize,
}

/// The compact dependency-graph row for one module.
#[component]
pub fn ModuleRow(props: &ModuleRowProps) -> Element {
    let console = props.console;
    let (fi, si, mi) = (props.feature, props.stage, props.module);
    let feats = features();
    let m = &feats[fi].stages[si].modules[mi];

    let name = m.name.clone();
    let status = m.status;
    let task_label = m.task_label();

    let inner: Element = ui! {
        view(style = RowInner()) {
            StatusDot(status = status)
            Typography(content = name, kind = typography_kind::Caption)
            Spacer()
            Mono(content = task_label, size = MonoTextSize::Overline)
        }
    };

    module_pressable(console, si, mi, status, inner)
}

stylesheet! {
    pub ModuleBox<IdeaThemeRef> {
        base(t) {
            border_width: 1.0,
            border_radius: t.radius.md(),
            background: t.color.surface(),
        }
        variant accent {
            #[default]
            plain(t) { border_color: t.color.border() }
            running(t) { border_color: t.intent.info.border() }
            violation(t) { border_color: t.intent.danger.border() }
            selected(t) { border_color: t.intent.primary.fg() }
        }
        variant dim {
            #[default]
            no(t) { opacity: 1.0 }
            yes(t) { opacity: 0.72 }
        }
        transitions {
            border_color: 260ms EaseOut,
            opacity: 260ms EaseOut,
        }
        state hovered(t) {
            border_color: t.color.border_strong(),
        }
    }
}

stylesheet! {
    pub ModuleInner<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
            padding: t.spacing.md(),
        }
    }
}

stylesheet! {
    pub RowInner<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
            padding_vertical: t.spacing.sm(),
            padding_horizontal: t.spacing.md(),
        }
    }
}

stylesheet! {
    pub NowRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.xs(),
        }
    }
}
