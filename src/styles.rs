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

stylesheet! {
    pub Dot<IdeaThemeRef> {
        base(t) {
            width: 8,
            height: 8,
            border_radius: t.radius.pill(),
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
        variant state {
            #[default]
            off(t) { background: t.color.border() }
            on(t) { background: t.intent.success.fg() }
        }
    }
}
