//! The module card: the one pressable per module on the graph, opening
//! the module drawer.

use idea_ui::{typography_kind, IdeaThemeRef, Spacer, Typography};
use runtime_core::{
    component, memo, rx, stylesheet, ui, AlignItems, Element, FlexDirection, FontWeight,
    IdealystSchema, IntoElement, StyleApplication,
};

use crate::components::bits::{Mono, StatusBadge, StatusDot, Ticks, tappable};
use crate::model::{AgentTone, Readiness, Status};
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
///
/// Built once per graph layout and never for a poll: every value on
/// it is read live off the module's signal, so a task ticked on a
/// box moves one tick here, and the ring keeps turning through it.
#[component]
pub fn ModuleCard(props: &ModuleCardProps) -> Element {
    let console = props.console;
    let data = console.data;
    let (fi, mi) = (props.feature, props.module);
    let m = data.module(fi, mi);
    let status = memo(move || m().map(|m| m.status).unwrap_or_default());
    let live = memo(move || {
        let now = data.clock.get();
        m().is_some_and(|m| m.live_at(now))
    });
    let name = rx!(m().map(|m| m.name.clone()).unwrap_or_default());
    // The agent slot says who holds the card — and, once the holder
    // has gone quiet, for how long: that is the one fact that tells a
    // parked box from a working one, and the spinner's absence alone
    // would only whisper it.
    let agent_line = move || {
        let now = data.clock.get();
        m().map(|m| m.agent_line_at(now)).unwrap_or_default()
    };
    let agent = rx!(agent_line().0);
    let agent_tone = rx!(match agent_line().1 {
        AgentTone::Quiet | AgentTone::Waiting => MonoTextTone::Warning,
        AgentTone::Ready => MonoTextTone::Success,
        AgentTone::Plain => MonoTextTone::Muted,
    });
    let task_label = rx!(m().map(|m| m.task_label()).unwrap_or_default());
    let ticks = rx!(m().map(|m| m.tasks.iter().map(|t| t.done).collect::<Vec<bool>>()).unwrap_or_default());
    // The one line under the ticks is the "waits on" list while the
    // gate is shut and the worker's last word once it is open: a card
    // that is waiting has nobody speaking on it, and a card that is
    // moving has nothing to wait for.
    let waits = move || {
        let feats = data.features.get();
        let f = feats.get(fi)?;
        let m = f.modules.get(mi)?;
        (m.readiness() == Readiness::Waiting)
            .then(|| clip(&format!("waits on {}", f.module_names(&m.waiting_on)), WAITS_CHARS))
    };
    let word = move || {
        let m = m()?;
        match (waits(), &m.last_word) {
            (None, Some(w)) if m.status != Status::Done => {
                Some(clip(&format!("\u{201c}{}\u{201d}", w.text), WAITS_CHARS))
            }
            _ => None,
        }
    };
    let selection = console.selected;
    let id = move || m().map(|m| m.id.clone()).unwrap_or_default();

    let inner: Element = ui! {
        view(style = ModuleInner()) {
            view(style = TitleRow()) {
                view(style = DotSlot()) {
                    StatusDot(status = status, live = live)
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
            match waits() {
                Some(line) => {
                    text(style = WaitsLine()) { line.clone() }
                }
                None => {}
            }
            match word() {
                Some(line) => {
                    text(style = WordLine()) { line.clone() }
                }
                None => {}
            }
        }
    };

    // The style is a CLOSURE and not a value: selecting a module writes
    // `Console::selected`, and reading it here means the highlight
    // lands on this one node instead of rebuilding the graph around it
    // — which is what lets the border transition play at all. The
    // status and readiness arms are read the same way.
    tappable(vec![inner], move || console.open_module(&id()))
        .with_style(move || {
            let m = m();
            let status = m.as_ref().map(|m| m.status).unwrap_or_default();
            let readiness = m.as_ref().map(|m| m.readiness()).unwrap_or(Readiness::Open);
            let accent = if m.as_ref().is_some_and(|m| selection.get().as_deref() == Some(m.id.as_str())) {
                "selected"
            } else {
                match status {
                    Status::Running => "running",
                    Status::Violation => "violation",
                    _ => "plain",
                }
            };
            let ready_arm = match readiness {
                Readiness::Open => "open",
                Readiness::Waiting => "waiting",
                Readiness::Done => "done",
            };
            let dim = matches!(status, Status::Blocked | Status::Queued);
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
    pub WordLine<IdeaThemeRef> {
        base(t) {
            font_size: t.typography.overline_size(),
            color: t.color.text_muted(),
            overflow: runtime_core::Overflow::Hidden,
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
