//! A rendered document — the feature's whitepaper, a module's handoff —
//! with its provenance line under it, or the one-sentence empty state.
//!
//! One component for both so the two places a document appears cannot
//! disagree about how markdown looks or how "nothing written" reads.

use idea_ui::{typography_kind, IdeaThemeRef, Typography};
use markdown::{Markdown, MdTheme};
use runtime_core::{
    component, rx, stylesheet, ui, Color, Element, FlexDirection, IdealystSchema, Tokenized,
};

use crate::state::Console;
use crate::styles::MONO;

/// Props for [`DocumentView`].
#[derive(Default, IdealystSchema)]
pub struct DocumentViewProps {
    /// Console state handles — the markdown theme follows the dark flag.
    pub console: Console,
    /// Whether there is a document at all. When false only `empty`
    /// renders.
    pub present: bool,
    /// The markdown source.
    pub body: String,
    /// "rev N · author · when", from [`crate::model::Document::meta`].
    pub meta: String,
    /// The one sentence shown when nothing has been written.
    pub empty: &'static str,
}

/// The document, rendered, with revision and author under it.
#[component]
pub fn DocumentView(props: &DocumentViewProps) -> Element {
    let console = props.console;
    let present = props.present;
    let body = props.body.clone();
    let meta = props.meta.clone();
    let empty = props.empty;
    if !present {
        return ui! {
            Typography(content = empty, kind = typography_kind::BodySm, muted = true)
        };
    }
    // Reactive on the dark flag, not rebuilt from it: the markdown node
    // re-resolves its theme in place when the toggle flips.
    let theme = rx!(md_theme(console.dark.get()));
    ui! {
        view(style = DocBox()) {
            Markdown(source = body, theme = theme)
            Typography(content = meta, kind = typography_kind::Caption, muted = true)
        }
    }
}

/// The markdown SDK's theme, taken from the SAME palette idea-ui is
/// installed with — so a document never carries its own idea of what
/// "text colour" is, in either mode.
pub fn md_theme(dark: bool) -> MdTheme {
    let t = if dark { idea_ui::dark_theme() } else { idea_ui::light_theme() };
    let c = |x: &Tokenized<Color>| x.value().0.clone();
    MdTheme {
        text: c(&t.colors.text),
        muted: c(&t.colors.text_muted),
        heading: c(&t.colors.text),
        link: c(&t.intents.primary.fg),
        code_fg: c(&t.colors.text),
        code_bg: c(&t.colors.surface_alt),
        quote_fg: c(&t.colors.text_muted),
        base_size: t.typography.body_size,
        // Console headings, not web-page headings: a whitepaper's h1 is
        // a section title inside a pane that already has a title.
        heading_scale: [1.5, 1.3, 1.15, 1.0, 1.0, 0.9],
        mono_family: Some(MONO.to_string()),
    }
}

stylesheet! {
    pub DocBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.md(),
            min_width: 0,
        }
    }
}
