//! The module card: the one pressable per module on the graph, opening
//! the module drawer.

use idea_ui::{typography_kind, IdeaThemeRef, Spacer, Typography};
use runtime_core::{
    component, pressable, stylesheet, ui, AlignItems, Element, FlexDirection, FontWeight,
    IdealystSchema, IntoElement, StyleApplication,
};

use crate::components::bits::{Mono, StatusBadge, StatusDot, Ticks};
use crate::model::{features, Readiness, Status};
use crate::state::Console;
use crate::styles::{MonoTextSize, MonoTextTone};

/// Card width in px. The graph positions cards and draws edges from
/// this geometry, so it is a constant and not a style: a value the
/// layout maths cannot read is a value the edges would miss by.
pub const CARD_W: f32 = 232.0;
/// Card height in px — fixed for the same reason. The card clips
/// rather than grows; the drawer has the room.
pub const CARD_H: f32 = 124.0;

/// The "waits on" line is one line: there is no no-wrap text style,
/// so a long list is cut here and the drawer carries the whole of it.
const WAITS_CHARS: usize = 38;

/// Props for [`ModuleCard`].
#[derive(Default, IdealystSchema)]
pub struct ModuleCardProps {
    /// Console state handles.
    pub console: Console,
    /// Feature index.
    pub feature: usize,
    /// Module index in [`crate::model::Feature::modules`].
    pub module: usize,
}

/// One module on the graph.
///
/// Two axes are visible at once: the status pill says what the module
/// is doing, and the left edge says where it stands against its
/// prerequisites — green when every one is done, amber while it waits
/// (with the names it waits on underneath), muted once it is done
/// itself.
#[component]
pub fn ModuleCard(props: &ModuleCardProps) -> Element {
    let console = props.console;
    let (fi, mi) = (props.feature, props.module);
    let feats = features();
    let f = &feats[fi];
    let m = &f.modules[mi];

    let name = m.name.clone();
    let status = m.status;
    let agent = m.agent.clone();
    let agent_tone = match agent.as_str() {
        "ready" => MonoTextTone::Success,
        "waiting" => MonoTextTone::Warning,
        _ => MonoTextTone::Muted,
    };
    let task_label = m.task_label();
    let ticks: Vec<bool> = m.tasks.iter().map(|t| t.done).collect();
    let readiness = m.readiness();
    let waits = (readiness == Readiness::Waiting)
        .then(|| clip(&format!("waits on {}", f.module_names(&m.waiting_on)), WAITS_CHARS));
    let ready_arm = match readiness {
        Readiness::Open => "open",
        Readiness::Waiting => "waiting",
        Readiness::Done => "done",
    };
    let dim = matches!(status, Status::Blocked | Status::Queued);
    let selection = console.selected;
    let id = m.id.clone();
    let press_id = id.clone();

    let inner: Element = ui! {
        view(style = ModuleInner()) {
            view(style = TitleRow()) {
                view(style = DotSlot()) {
                    StatusDot(status = status)
                }
                view(style = TitleSlot()) {
                    Typography(
                        content = name,
                        kind = typography_kind::BodySm,
                        weight = Some(FontWeight::SemiBold),
                    )
                }
                view(style = FixedSlot()) {
                    StatusBadge(status = status)
                }
            }
            view(style = MetaRow()) {
                Mono(content = agent, size = MonoTextSize::Overline, tone = agent_tone)
                Spacer()
                Mono(content = task_label, size = MonoTextSize::Overline)
            }
            Ticks(ticks = ticks)
            if let Some(line) = waits {
                text(style = WaitsLine()) { line }
            }
        }
    };

    // The style is a CLOSURE and not a value: selecting a module writes
    // `Console::selected`, and reading it here means the highlight
    // lands on this one node instead of rebuilding the graph around it
    // — which is what lets the border transition play at all.
    pressable(vec![inner], move || console.open_module(&press_id))
        .with_style(move || {
            let accent = if selection.get().as_deref() == Some(id.as_str()) {
                "selected"
            } else {
                match status {
                    Status::Running => "running",
                    Status::Violation => "violation",
                    _ => "plain",
                }
            };
            StyleApplication::new(module_box_style())
                .with("accent", accent.to_string())
                .with("ready", ready_arm.to_string())
                .with("dim", if dim { "yes" } else { "no" }.to_string())
        })
        .into_element()
}

/// Cut `s` to at most `max` characters, with an ellipsis when it was.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

stylesheet! {
    pub ModuleBox<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            min_height: 0,
            // Spelled per side: the `border_width` shorthand expands to
            // all four and collides with the left edge.
            border_top_width: 1.0,
            border_right_width: 1.0,
            border_bottom_width: 1.0,
            border_left_width: 3.0,
            border_radius: t.radius.md(),
            background: t.color.surface(),
            overflow: runtime_core::Overflow::Hidden,
            cursor: runtime_core::Cursor::Pointer,
        }
        variant accent {
            #[default]
            plain(t) { border_color: t.color.border() }
            running(t) { border_color: t.intent.info.border() }
            violation(t) { border_color: t.intent.danger.border() }
            selected(t) { border_color: t.intent.primary.fg() }
        }
        // Declared AFTER `accent` so the left edge wins over the
        // all-sides colour.
        variant ready {
            #[default]
            open(t) { border_left_color: t.intent.success.fg() }
            waiting(t) { border_left_color: t.intent.warning.fg() }
            done(t) { border_left_color: t.color.border_strong() }
        }
        variant dim {
            #[default]
            no(t) { opacity: 1.0 }
            yes(t) { opacity: 0.8 }
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
            min_width: 0,
        }
    }
}

stylesheet! {
    pub TitleRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexStart,
            gap: t.spacing.sm(),
            min_width: 0,
        }
    }
}

// The flexible / fixed pair (rule 22). The title gets two lines and
// then clips: a name longer than that belongs to the drawer's H3.
stylesheet! {
    pub TitleSlot<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            min_width: 0,
            flex_grow: 1.0,
            flex_shrink: 1.0,
            max_height: 40,
            overflow: runtime_core::Overflow::Hidden,
        }
    }
}

// The dot stays on the first line of a two-line title.
stylesheet! {
    pub DotSlot<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            flex_shrink: 0.0,
            height: 20,
        }
    }
}

stylesheet! {
    pub FixedSlot<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    pub MetaRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
            min_width: 0,
        }
    }
}

stylesheet! {
    pub WaitsLine<IdeaThemeRef> {
        base(t) {
            font_size: t.typography.overline_size(),
            color: t.intent.warning.soft_text(),
            overflow: runtime_core::Overflow::Hidden,
        }
    }
}
