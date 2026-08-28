//! The capture card: type wants straight into the console.
//!
//! One surface — a `code_editor` where every line is its own want, the
//! shape a brain-dump actually has — with `#tag` highlighting and Tab
//! completion. It handles a single want as well as forty, so there is
//! no second "quick" mode to choose between.
//!
//! The syntax is defined once, in [`api::capture`], and used twice here:
//! [`api::scan_tags`] drives the highlighting, and the server function
//! parses the same buffer with the same code when it writes. What the
//! editor colors is therefore exactly what lands in the pool — there is
//! no second parser to drift.
//!
//! The card is built OUTSIDE the pool's data-keyed `switch` on purpose:
//! a rebuild would recreate the text node and steal focus mid-sentence,
//! and the poll that lands an agent's want must not interrupt typing.

use std::rc::Rc;

use codeblock::{code_editor, Decoration, DecorationStyle, Underline};
use idea_ui::{tone, typography_kind, variant, Button, IdeaThemeRef, Spacer, Tag, Typography};
use runtime_core::{
    component, primitives::key::KeyOutcome, primitives::text_area::TextAreaHandle, pressable,
    spawn_then, stylesheet, switch, ui, AlignItems, Cursor, Element, FlexDirection, FlexWrap,
    FontWeight, IdealystSchema, IntoElement, Ref,
};

use crate::model;
use crate::state::Console;
use crate::styles::SectionLabel;

/// Props for [`Composer`].
#[derive(Default, IdealystSchema)]
pub struct ComposerProps {
    /// Console state handles.
    pub console: Console,
}

/// The capture card at the top of the want pool.
#[component]
pub fn Composer(props: &ComposerProps) -> Element {
    let console = props.console;
    // The editing layer's imperative handle, so Tab can insert the
    // completion at the real caret instead of rewriting the buffer.
    let editor: Ref<TextAreaHandle> = Ref::new();

    let submit = move || {
        if console.busy.get() {
            return;
        }
        let text = console.draft.get();
        if text.trim().is_empty() {
            return;
        }
        console.busy.set(true);
        console.status.set(String::new());
        spawn_then(api::capture_wants(text), move |result| {
            console.busy.set(false);
            match result {
                Ok(capture) => {
                    console.draft.set(String::new());
                    console.status.set(describe(&capture));
                    // Don't wait out the poll window for your own write.
                    console.refresh.update(|r| r + 1);
                }
                Err(err) => console.status.set(format!("Nothing captured — {err}")),
            }
        });
    };

    let body: Element = ui! { view(style = EditorHost()) { bulk_editor(console, editor) } };

    // The live count and the submit button read signals, so this strip
    // rebuilds while the editor above it does not.
    let controls = switch(
        move || (console.busy.get(), console.draft.get(), console.status.get()),
        move |state: &(bool, String, String)| {
            let (busy, text, status) = state.clone();
            let drafts = api::parse_buffer(&text);
            let ready = !drafts.is_empty() && !busy;
            let label = match drafts.len() {
                0 => "Add want".to_string(),
                1 => "Add 1 want".to_string(),
                n => format!("Add {n} wants"),
            };
            // One text slot, not two: the last capture's result replaces
            // the pending count rather than appearing beside it, so the
            // row never gains or loses a node. Everything variable sits
            // left of the Spacer, so its width can't shove the button.
            let note = if status.is_empty() { summarize(&drafts) } else { status };
            let on_click: Rc<dyn Fn()> = Rc::new(submit);
            ui! {
                view(style = ControlRow()) {
                    Typography(content = note, kind = typography_kind::Caption, muted = true)
                    Spacer()
                    // Fixed width: the label counts up as you type, and a
                    // control that resizes on every keystroke twitches the
                    // whole row (UX_GUIDELINES rule 18).
                    view(style = SubmitSlot()) {
                        Button(
                            label = label,
                            on_click = on_click,
                            size = idea_ui::size::Sm,
                            disabled = !ready,
                            loading = busy,
                            block = true,
                        )
                    }
                }
            }
        },
    );

    ui! {
        view(style = CardBox()) {
            view(style = HeadRow()) {
                text(style = SectionLabel()) { "Capture" }
            }
            body
            controls
            TagRail(console = console)
        }
    }
}

/// The bulk surface: one want per line, `#tag` highlighted, Tab
/// completes against the registry.
fn bulk_editor(console: Console, handle: Ref<TextAreaHandle>) -> Element {
    let t = idea_theme::tokens();
    code_editor(console.draft, move |text| console.draft.set(text))
        .decorate(decorate)
        .placeholder("One want per line.\nTag with #ux — press Tab to complete.")
        .font(crate::styles::MONO, 13.0)
        .line_height(22.0)
        .padding(12.0)
        .text_color(t.color.text().resolve().0)
        .caret_color(t.intent.primary.fg().resolve().0)
        .on_key_down(move |e| {
            if e.key != "Tab" || e.shift || e.ctrl || e.meta || e.alt {
                return KeyOutcome::Default;
            }
            match complete_tag(&console.draft.get(), e.selection_start) {
                Some(insert) => {
                    handle.with(|h| h.insert_text(&insert));
                    KeyOutcome::PreventDefault
                }
                // No `#fragment` under the caret: leave Tab alone rather
                // than swallowing the key.
                None => KeyOutcome::Default,
            }
        })
        .bind(handle)
        .with_style(EditorBox())
        .into_element()
}

/// Props for [`TagRail`].
#[derive(Default, IdealystSchema)]
pub struct TagRailProps {
    /// Console state handles.
    pub console: Console,
}

/// The known tags, most-used first. Clicking one appends `#name` to
/// whichever buffer is open — the "preset tags" half of filing, next to
/// the `#`-anything half.
#[component]
pub fn TagRail(props: &TagRailProps) -> Element {
    let console = props.console;
    switch(
        move || console.rev.get(),
        move |&_rev: &u64| {
            let count = model::tags().len();
            if count == 0 {
                return ui! { view {} };
            }
            ui! {
                view(style = RailBox()) {
                    text(style = SectionLabel()) { "Tags" }
                    view(style = RailWrap()) {
                        for i in 0..count {
                            TagChip(console = console, index = i)
                        }
                    }
                }
            }
        },
    )
}

/// Props for [`TagChip`].
#[derive(Default, IdealystSchema)]
pub struct TagChipProps {
    /// Console state handles.
    pub console: Console,
    /// Index into [`crate::model::tags`].
    pub index: usize,
}

/// One clickable tag in the rail.
#[component]
pub fn TagChip(props: &TagChipProps) -> Element {
    let console = props.console;
    let tags = model::tags();
    let tag = &tags[props.index];
    let name = tag.name.clone();
    let label = if tag.uses > 0 {
        format!("{} {}", tag.label, tag.uses)
    } else {
        tag.label.clone()
    };
    let insert = name.clone();
    let inner: Element = ui! {
        Tag(label = label, tone = tone::Neutral, variant = variant::Soft)
    };
    pressable(vec![inner], move || append_tag(console, &insert))
        .with_style(ChipPress())
        .into_element()
}

/// Append `#name` to the draft, with the spacing already right.
fn append_tag(console: Console, name: &str) {
    console.draft.update(|current| {
        let mut next = current.clone();
        if !next.is_empty() && !next.ends_with(char::is_whitespace) {
            next.push(' ');
        }
        next.push('#');
        next.push_str(name);
        next.push(' ');
        next
    });
}

// ---------------------------------------------------------------------
// Highlighting + completion
// ---------------------------------------------------------------------

/// Color every `#tag` in the buffer: known tags in the info tone, tags
/// that do not exist yet in the success tone with a dotted underline —
/// so "this will create a new tag" is visible BEFORE you press the
/// button, which is the one thing about a free-text tag syntax that is
/// otherwise invisible until it's too late.
///
/// Runs on every keystroke, so it stays a single linear scan. Colors
/// resolve through the theme's token registry, which means the swap to
/// dark mode re-tints them with no work here.
fn decorate(text: &str) -> Vec<Decoration> {
    let known = model::tag_names();
    let t = idea_theme::tokens();
    let known_color = t.intent.info.fg().resolve().0;
    let fresh_color = t.intent.success.fg().resolve().0;

    api::scan_tags(text)
        .into_iter()
        .map(|span| {
            let is_known = known.iter().any(|k| *k == span.name);
            let mut style = DecorationStyle::default()
                .with_weight(FontWeight::SemiBold)
                .with_color(if is_known { known_color.clone() } else { fresh_color.clone() });
            if !is_known {
                style = style.with_underline(Underline::dotted());
            }
            Decoration::new(span.start..span.end, style)
        })
        .collect()
}

/// What Tab should insert at the caret, if anything.
///
/// `caret` arrives in UTF-16 code units (every platform's native caret
/// API reports those); the buffer is indexed in bytes, so it converts
/// first. Completes to the longest common prefix of the matches — the
/// shell rule — and finishes an unambiguous tag with a trailing space.
fn complete_tag(text: &str, caret: usize) -> Option<String> {
    let caret = api::utf16_to_byte(text, caret);
    let (_, fragment) = api::tag_fragment_at(text, caret)?;
    let matches: Vec<String> = model::tag_names()
        .into_iter()
        .filter(|name| name.starts_with(&fragment))
        .collect();
    let (first, rest) = matches.split_first()?;
    let common = rest.iter().fold(first.clone(), |acc, name| {
        let keep = acc
            .char_indices()
            .zip(name.chars())
            .take_while(|((_, a), b)| a == b)
            .count();
        acc.chars().take(keep).collect()
    });
    if common.len() <= fragment.len() {
        return None;
    }
    let mut insert = common[fragment.len()..].to_string();
    if rest.is_empty() {
        insert.push(' ');
    }
    Some(insert)
}

/// The line under the composer: what pressing the button will do.
fn summarize(drafts: &[api::WantDraftDto]) -> String {
    if drafts.is_empty() {
        // The editor's own placeholder carries the instruction; saying it
        // again here is the same fact twice (rule 16).
        return String::new();
    }
    let known = model::tag_names();
    let mut tags: Vec<&String> = Vec::new();
    for draft in drafts {
        for tag in &draft.tags {
            if !tags.contains(&tag) {
                tags.push(tag);
            }
        }
    }
    let fresh = tags.iter().filter(|t| !known.contains(t)).count();
    let mut parts = vec![match drafts.len() {
        1 => "1 want".to_string(),
        n => format!("{n} wants"),
    }];
    if !tags.is_empty() {
        parts.push(format!("{} tag(s)", tags.len()));
    }
    if fresh > 0 {
        parts.push(format!("{fresh} new"));
    }
    parts.join(" · ")
}

/// What the server reported, in one line.
fn describe(capture: &api::CaptureResult) -> String {
    let mut out = match capture.added.len() {
        1 => "Captured 1 want".to_string(),
        n => format!("Captured {n} wants"),
    };
    if !capture.new_tags.is_empty() {
        out.push_str(&format!(
            " · created #{}",
            capture.new_tags.join(" #")
        ));
    }
    if capture.skipped > 0 {
        out.push_str(&format!(" · {} line(s) had no idea in them", capture.skipped));
    }
    out
}

stylesheet! {
    pub CardBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.md(),
            padding: t.spacing.lg(),
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.md(),
            background: t.color.surface(),
        }
    }
}

stylesheet! {
    pub HeadRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.md(),
        }
    }
}

stylesheet! {
    pub EditorHost<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
        }
    }
}



stylesheet! {
    pub EditorBox<IdeaThemeRef> {
        base(t) {
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.sm(),
            background: t.color.background(),
            min_height: 96,
        }
    }
}

stylesheet! {
    pub ControlRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            flex_wrap: FlexWrap::Wrap,
            gap: t.spacing.md(),
        }
    }
}




stylesheet! {
    pub RailBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
            padding_top: t.spacing.xs(),
            border_top_width: 1.0,
            border_color: t.color.border(),
        }
    }
}

stylesheet! {
    pub RailWrap<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            gap: t.spacing.xs(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::complete_tag;
    use crate::model;

    /// Seed the model's tag registry the way a snapshot would.
    fn with_tags(names: &[&str]) {
        let mut snap = api::Snapshot::default();
        snap.tags = names
            .iter()
            .map(|n| api::TagDto {
                name: n.to_string(),
                label: n.to_string(),
                uses: 1,
            })
            .collect();
        model::apply_snapshot(snap);
    }

    #[test]
    fn tab_completion_follows_the_shell_rules() {
        with_tags(&["billing", "billing-export", "ux", "ops"]);

        // Unambiguous: finish it and add the separating space.
        let text = "exports are painful #u";
        assert_eq!(complete_tag(text, text.len() as usize), Some("x ".to_string()));

        // Ambiguous: complete only as far as the shared prefix, no space
        // — the caret is left where you keep typing the difference.
        let text = "#bil";
        assert_eq!(complete_tag(text, text.len() as usize), Some("ling".to_string()));

        // Already at the shared prefix and still ambiguous: nothing to add.
        let text = "#billing";
        assert_eq!(complete_tag(text, text.len() as usize), None);

        // No match at all, and no `#` under the caret: Tab stays Tab.
        let text = "#zzz";
        assert_eq!(complete_tag(text, text.len() as usize), None);
        let text = "just prose";
        assert_eq!(complete_tag(text, text.len() as usize), None);
    }

    #[test]
    fn completes_mid_buffer_and_past_wide_characters() {
        with_tags(&["ux"]);
        // A caret in the middle of a multi-line buffer completes the
        // fragment it is actually sitting in, not the last one typed.
        let text = "first #u\nsecond line #ops";
        let caret = "first #u".len();
        assert_eq!(complete_tag(text, caret), Some("x ".to_string()));

        // The caret arrives in UTF-16 units: an emoji earlier in the
        // line must not shift the fragment we look at.
        let text = "🙂 #u";
        let caret_utf16 = 2 + 1 + 2; // emoji + space + "#u"
        assert_eq!(complete_tag(text, caret_utf16), Some("x ".to_string()));
    }
}

stylesheet! {
    pub ChipPress<IdeaThemeRef> {
        base(t) {
            border_radius: t.radius.pill(),
            cursor: Cursor::Pointer,
            opacity: 1.0,
        }
        state hovered(t) {
            opacity: 0.72,
        }
    }
}

// Sized for the longest label the button can produce ("Add 99 wants"),
// so short counts sit in the slack instead of resizing it.
stylesheet! {
    pub SubmitSlot<IdeaThemeRef> {
        base(t) {
            width: 132,
            flex_shrink: 0.0,
        }
    }
}
