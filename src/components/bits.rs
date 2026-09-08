//! Small shared console pieces: status dot, status badge, monospace
//! metadata text, the per-task tick strip, and the `?` hint.

use std::rc::Rc;

use idea_ui::{typography_kind, Badge, Spacer, Tooltip, Typography};
use runtime_core::{component, pressable, ui, Element, IdealystSchema, IntoElement,
    StyleApplication};

use crate::model::Status;
use crate::styles::{
    status_dot, status_tone, Dot, MonoText, MonoTextSize, MonoTextTone, Tick, TickState,
};

/// Props for [`StatusDot`].
#[derive(Default, IdealystSchema)]
pub struct StatusDotProps {
    /// Status whose semantic color the dot takes.
    pub status: Status,
}

/// A small round indicator colored by status.
#[component]
pub fn StatusDot(props: &StatusDotProps) -> Element {
    let style = Dot().tone(status_dot(props.status));
    ui! {
        view(style = style) {}
    }
}

/// Props for [`StatusBadge`].
#[derive(Default, IdealystSchema)]
pub struct StatusBadgeProps {
    /// Status rendered as a soft idea-ui badge.
    pub status: Status,
}

/// The status pill: idea-ui `Badge` with the console's tone mapping.
#[component]
pub fn StatusBadge(props: &StatusBadgeProps) -> Element {
    let label = props.status.label();
    let tone = status_tone(props.status);
    ui! {
        Badge(label = label, tone = tone)
    }
}

/// Props for [`Mono`].
#[derive(IdealystSchema)]
pub struct MonoProps {
    /// The text to render in the console's monospace stack.
    pub content: String,
    /// Type size arm of the [`MonoText`] sheet.
    pub size: MonoTextSize,
    /// Color arm of the [`MonoText`] sheet.
    pub tone: MonoTextTone,
}

impl Default for MonoProps {
    fn default() -> Self {
        Self {
            content: String::new(),
            size: MonoTextSize::Caption,
            tone: MonoTextTone::Muted,
        }
    }
}

/// Monospace metadata text (agent ids, tool names, timestamps).
#[component]
pub fn Mono(props: &MonoProps) -> Element {
    let content = props.content.clone();
    let style = MonoText().size(props.size).tone(props.tone);
    ui! {
        text(style = style) { content }
    }
}

/// Props for [`Ticks`].
#[derive(Default, IdealystSchema)]
pub struct TicksProps {
    /// One entry per task; `true` = done.
    pub ticks: Vec<bool>,
}

/// The per-task progress strip on a module card.
#[component]
pub fn Ticks(props: &TicksProps) -> Element {
    let ticks = props.ticks.clone();
    ui! {
        view(style = TickRow()) {
            for done in ticks {
                view(style = Tick().state(if done { TickState::On } else { TickState::Off })) {}
            }
        }
    }
}

runtime_core::stylesheet! {
    pub TickRow<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: runtime_core::FlexDirection::Row,
            gap: t.spacing.xs(),
        }
    }
}

/// Props for [`Hint`].
#[derive(Default, IdealystSchema)]
pub struct HintProps {
    /// What the bubble says. Keep it to the fact the reader cannot
    /// infer from what is already on screen.
    pub text: String,
}

/// The `?` affordance: the only sanctioned carrier for explanatory
/// copy (UX_GUIDELINES rule 7). Sits beside the thing it explains and
/// stays out of the way until asked.
#[component]
pub fn Hint(props: &HintProps) -> Element {
    let text = props.text.clone();
    ui! {
        Tooltip(text = text) {
            view(style = HintBox()) {
                text(style = HintGlyph()) { "?" }
            }
        }
    }
}

runtime_core::stylesheet! {
    pub HintBox<idea_ui::IdeaThemeRef> {
        base(t) {
            width: 14,
            height: 14,
            border_radius: 7,
            border_width: 1.0,
            border_color: t.color.border_strong(),
            align_items: runtime_core::AlignItems::Center,
            justify_content: runtime_core::JustifyContent::Center,
            flex_shrink: 0.0,
            cursor: runtime_core::Cursor::Pointer,
        }
        transitions {
            background: 160ms EaseOut,
            border_color: 160ms EaseOut,
            opacity: 160ms EaseOut,
        }
        state hovered(t) {
            border_color: t.color.text_muted(),
            background: t.color.surface_alt(),
        }
    }
}

runtime_core::stylesheet! {
    pub HintGlyph<idea_ui::IdeaThemeRef> {
        base(t) {
            font_size: t.typography.overline_size(),
            font_weight: runtime_core::FontWeight::SemiBold,
            color: t.color.text_muted(),
        }
    }
}

/// Props for [`Pager`].
#[derive(Default, IdealystSchema)]
pub struct PagerProps {
    /// Where to go. `None` renders the footer inert — the shape is the
    /// same, which is the point: a table with one page must not be a
    /// different height from a table with four.
    pub on_page: Option<Rc<dyn Fn(usize)>>,
    /// "1-20 of 47", already formatted.
    pub summary: String,
    /// Zero-based current page.
    pub page: usize,
    /// Total pages, at least 1.
    pub pages: usize,
}

/// The table's footer: what you are looking at, and the way to the rest
/// of it. The chevrons stay put on a single page rather than vanishing,
/// so the footer never changes width under you.
///
/// Shared by every paged table in the console. A second copy of this
/// would be a second place for the ghost-chevron rule to be forgotten.
#[component]
pub fn Pager(props: &PagerProps) -> Element {
    let step = props.on_page.clone();
    let summary = props.summary.clone();
    let page = props.page;
    let pages = props.pages;
    let of_label = format!("Page {} of {}", page + 1, pages);
    let back_on = page > 0;
    let next_on = page + 1 < pages;

    // Dimmed rather than removed: a chevron that vanishes on the last
    // page resizes the footer under the reader (rule 18).
    let back_arm = Chevron().live(if back_on { ChevronLive::Yes } else { ChevronLive::No });
    let next_arm = Chevron().live(if next_on { ChevronLive::Yes } else { ChevronLive::No });
    let back_step = step.clone();
    let back = pressable(
        vec![ui! { text(style = back_arm) { "\u{2039}" } }],
        move || {
            if let (true, Some(step)) = (page > 0, back_step.as_ref()) {
                step(page - 1);
            }
        },
    )
    .with_style(StyleApplication::new(pager_button_style()))
    .into_element();
    let next_step = step;
    let next = pressable(
        vec![ui! { text(style = next_arm) { "\u{203a}" } }],
        move || {
            if let (true, Some(step)) = (page + 1 < pages, next_step.as_ref()) {
                step(page + 1);
            }
        },
    )
    .with_style(StyleApplication::new(pager_button_style()))
    .into_element();

    ui! {
        view(style = PagerBox()) {
            Typography(content = summary, kind = typography_kind::Caption, muted = true)
            Spacer()
            Typography(content = of_label, kind = typography_kind::Caption, muted = true)
            back
            next
        }
    }
}


runtime_core::stylesheet! {
    pub PagerBox<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: runtime_core::FlexDirection::Row,
            align_items: runtime_core::AlignItems::Center,
            gap: t.spacing.sm(),
        }
    }
}


runtime_core::stylesheet! {
    pub PagerButton<idea_ui::IdeaThemeRef> {
        base(t) {
            width: 24,
            height: 24,
            border_radius: t.radius.sm(),
            align_items: runtime_core::AlignItems::Center,
            justify_content: runtime_core::JustifyContent::Center,
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


runtime_core::stylesheet! {
    pub Chevron<idea_ui::IdeaThemeRef> {
        base(t) {
            font_size: t.typography.body_size(),
        }
        variant live {
            #[default]
            no(t) { color: t.color.border() }
            yes(t) { color: t.color.text() }
        }
    }
}
