//! Shared console styles + status→tone mapping.
//!
//! Every sheet declares the `IdeaThemeRef` vocabulary so colors,
//! spacing, and radii resolve from the installed idea-ui theme — a
//! light/dark swap re-flows all of it with no per-node wiring.

use idea_ui::{tone, IdeaThemeRef, ToneRef};
use runtime_core::{stylesheet, FontFamily, FontWeight, TextTransform};

use crate::model::Status;

/// Monospace stack used for agent ids, tool names, and codes.
pub const MONO: &str = "ui-monospace, Menlo, Consolas, monospace";

/// idea-ui semantic tone for a status (drives Badge / Progress fills).
pub fn status_tone(status: Status) -> ToneRef {
    match status {
        Status::Done => tone::Success.into(),
        Status::Running => tone::Info.into(),
        Status::Blocked | Status::Queued => tone::Neutral.into(),
        Status::Violation => tone::Danger.into(),
        Status::Planning => tone::Warning.into(),
    }
}

/// [`Dot`] variant arm for a status.
pub fn status_dot(status: Status) -> DotTone {
    match status {
        Status::Done => DotTone::Success,
        Status::Running => DotTone::Info,
        Status::Blocked | Status::Queued => DotTone::Neutral,
        Status::Violation => DotTone::Danger,
        Status::Planning => DotTone::Warning,
    }
}

// The status dot and the task tick are the two shapes that repeat on
// every screen, so they are where a change of state is most often
// SEEN. Both carry a color transition: wherever the node survives the
// change — a hover, a selection, a tick going green under a poll that
// only touched one card — the color moves instead of snapping, and the
// eye is drawn to the thing that actually changed rather than to the
// whole list redrawing. Where the subtree IS rebuilt the transition is
// simply inert, which is why it costs nothing to declare here.
stylesheet! {
    pub Dot<IdeaThemeRef> {
        base(t) {
            width: 8,
            height: 8,
            // A row squeezes an 8px child sideways before it wraps
            // anything else (rule 18): pin both axes and refuse to
            // shrink, or the dot becomes an oval and the ring a "C".
            min_width: 8,
            min_height: 8,
            flex_shrink: 0.0,
            border_radius: t.radius.pill(),
        }
        transitions {
            background: 320ms EaseOut,
        }
        variant tone {
            #[default]
            neutral(t) { background: t.color.border_strong() }
            success(t) { background: t.intent.success.fg() }
            info(t) { background: t.intent.info.fg() }
            danger(t) { background: t.intent.danger.fg() }
            warning(t) { background: t.intent.warning.fg() }
            primary(t) { background: t.intent.primary.fg() }
        }
    }
}

// The live ring: the status dot's shape, drawn as an arc and spun.
// One side transparent is what makes rotation visible at 8px; the
// widths are spelled per side because the shorthand collides with
// per-side colours (see ModuleBox). Colour is the same info ink as
// the running dot, so a card that starts moving changes MOTION, not
// palette — the eye reads "this one is alive", not "this one changed
// state".
stylesheet! {
    pub LiveRing<IdeaThemeRef> {
        base(t) {
            width: 8,
            height: 8,
            min_width: 8,
            min_height: 8,
            flex_shrink: 0.0,
            border_radius: t.radius.pill(),
            border_top_width: 2.0,
            border_right_width: 2.0,
            border_bottom_width: 2.0,
            border_left_width: 2.0,
            border_top_color: runtime_core::Color("#00000000".into()),
            border_right_color: t.intent.info.fg(),
            border_bottom_color: t.intent.info.fg(),
            border_left_color: t.intent.info.fg(),
        }
    }
}

stylesheet! {
    pub MonoText<IdeaThemeRef> {
        base(t) {
            font_family: FontFamily::System(String::from(MONO)),
            font_size: t.typography.caption_size(),
            color: t.color.text_muted(),
        }
        variant size {
            #[default]
            caption(t) { font_size: t.typography.caption_size() }
            overline(t) { font_size: t.typography.overline_size() }
        }
        variant tone {
            #[default]
            muted(t) { color: t.color.text_muted() }
            text(t) { color: t.color.text() }
            info(t) { color: t.intent.info.fg() }
            danger(t) { color: t.intent.danger.fg() }
            success(t) { color: t.intent.success.fg() }
            warning(t) { color: t.intent.warning.soft_text() }
            primary(t) { color: t.intent.primary.fg() }
        }
    }
}

stylesheet! {
    pub SectionLabel<IdeaThemeRef> {
        base(t) {
            font_size: t.typography.overline_size(),
            font_weight: FontWeight::SemiBold,
            text_transform: TextTransform::Uppercase,
            letter_spacing: 1.0,
            color: t.color.text_muted(),
        }
    }
}

stylesheet! {
    pub Tick<IdeaThemeRef> {
        base(t) {
            height: 4,
            flex_grow: 1.0,
            border_radius: t.radius.sm(),
        }
        transitions {
            background: 320ms EaseOut,
        }
        variant state {
            #[default]
            off(t) { background: t.color.border() }
            on(t) { background: t.intent.success.fg() }
        }
    }
}
