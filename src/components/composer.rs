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
use runtime_core::primitives::key::{KeyEvent, KeyOutcome};
use runtime_core::primitives::overlay::BackdropMode;
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide};
use runtime_core::{
    anchored_overlay, component, primitives::text_area::TextAreaHandle, pressable, rx, signal,
    spawn_then, stylesheet, switch, text, ui, view, AlignItems, Cursor, Element, FlexDirection, FlexWrap,
    FontWeight, IdealystSchema, IntoElement, JustifyContent, Ref, Signal, StyleApplication,
    ViewHandle,
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
    // The editor's box, so the completion list can anchor under it.
    // The caret's own geometry is not exposed by any backend, so the
    // list hangs off the box rather than tracking the `#` glyph.
    let anchor: Ref<ViewHandle> = Ref::new();

    // The `#fragment` the caret is sitting in, or `None` for "no list".
    // `Some("")` is a real state — the caret is just past a bare `#`,
    // which matches every tag — so the emptiness of the string cannot
    // double as the closed sentinel.
    let fragment: Signal<Option<String>> = signal(None);
    // Where the keyboard cursor sits in the CURRENT match list. Reset
    // to the top whenever the fragment changes, because the list it
    // indexes into has just been replaced.
    let highlight: Signal<usize> = signal(0usize);
    // The caret the list was opened against, in UTF-16 code units. A
    // key-driven accept reads the caret off its own event; a CLICKED
    // row has no event to read, so it accepts against this.
    let caret: Signal<usize> = signal(0usize);

    // Pressing the button BUMPS this counter; the request itself is
    // spawned from the hole below.
    //
    // `spawn_then` anchors its callback to the scope that spawned it,
    // and the scope of a press handler is the button's own node — which
    // this very handler rebuilds by flipping `busy`. Spawned from here,
    // the callback was dropped as a dead scope's on the way out: the
    // server captured the wants, and the console kept spinning with the
    // buffer still full because the half that clears them never ran.
    let submits = signal(0u64);

    let submit = move || {
        if console.busy.get() {
            return;
        }
        if console.draft.get().trim().is_empty() {
            return;
        }
        console.busy.set(true);
        console.status.set(String::new());
        submits.update(|n| n + 1);
    };

    // The capture itself. Keyed on the counter, so the scope that owns
    // an in-flight request is torn down only by the NEXT submit — never
    // by the state the request's own callback writes. A `switch` build
    // closure runs UNTRACKED, which is what keeps the draft read here
    // from re-firing the capture on every keystroke.
    let capture = switch(
        move || submits.get(),
        move |&n: &u64| {
            // 0 is the mount, not a submit.
            if n > 0 {
                spawn_then(api::capture_wants(console.draft.get()), move |result| {
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
            }
            ui! { view(style = CaptureHole()) {} }
        },
    );

    // The editor neither wraps nor scrolls itself: its decorated layer
    // measures the WHOLE text and the editing layer is stretched over
    // that, so a long line paints straight out through the card's
    // border unless an ancestor both clamps the width and scrolls it.
    // `min_width: 0` on the host is the load-bearing half (rule 22):
    // without it nothing may size below the text and there is nothing
    // for the scroller to scroll against (rule 23).
    //
    // The row INSIDE the scroller is what the board does too, and for
    // the same reason: a flex item may not shrink below its own
    // content, so the editor's box grows to the longest line and the
    // editing layer — which is stretched over that box — grows with it.
    // Dropped straight into the scroller the box would stay the card's
    // width instead and the editor's own text node would scroll itself:
    // two horizontal scrollbars, and the highlighting drifting off the
    // glyphs it belongs to.
    //
    // Built with the `view(..)` builder rather than `ui!` for one
    // reason: the host node has to carry the anchor `Ref` the
    // completion list measures itself against, and `bind` is a builder
    // method.
    let editor_box: Element = ui! {
        scroll_view(horizontal = true, style = EditorBox()) {
            view(style = EditorRow()) {
                bulk_editor(console, editor, fragment, highlight, caret)
            }
        }
    };
    let body: Element = view(vec![editor_box])
        .with_style(EditorHost())
        .bind(anchor)
        .into_element();

    // The match list. Keyed on the FRAGMENT, not on a bool: `when`
    // would only rebuild as the list opened and closed, so typing
    // `#b` → `#bi` would leave yesterday's matches on screen. Keyed
    // here it also cannot rebuild the editor, which is a sibling
    // (rule 25).
    //
    // Every branch is wrapped in a hole for the same reason the
    // capture driver is: the card is a `gap`-ed column, so a node that
    // renders nothing still collects a gap and pushes the controls off
    // the editor. The panel itself is portalled out of the layout by
    // `anchored_overlay`, so the hole costs it nothing.
    let completions = switch(
        move || fragment.get(),
        move |frag: &Option<String>| {
            let panel: Element = match frag {
                Some(frag) => {
                    completion_list(console, editor, anchor, fragment, highlight, caret, frag)
                }
                None => ui! { view {} },
            };
            ui! { view(style = CaptureHole()) { panel } }
        },
    );

    // Live props, NOT a `switch` on the draft: every prop here is a
    // `Reactive`, so each one re-renders in place and the row keeps its
    // nodes. Keyed on the draft the whole strip — the button included —
    // was torn down and rebuilt on every keystroke, which is both waste
    // and the reason a press handler is not a safe place to spawn from.
    //
    // One text slot, not two: the last capture's result replaces the
    // pending count rather than appearing beside it, so the row never
    // gains or loses a node. Everything variable sits left of the
    // Spacer, so its width can't shove the button.
    let on_click: Rc<dyn Fn()> = Rc::new(submit);
    let controls: Element = ui! {
        view(style = ControlRow()) {
            Typography(content = rx!(note(console)), kind = typography_kind::Caption,
                       muted = true)
            Spacer()
            // Fixed width: the label counts up as you type, and a
            // control that resizes on every keystroke twitches the
            // whole row (UX_GUIDELINES rule 18).
            view(style = SubmitSlot()) {
                Button(
                    label = rx!(submit_label(console)),
                    on_click = on_click,
                    size = idea_ui::size::Sm,
                    disabled = rx!(!submit_ready(console)),
                    loading = rx!(console.busy.get()),
                    block = true,
                )
            }
        }
    };

    ui! {
        view(style = CardBox()) {
            view(style = HeadRow()) {
                text(style = SectionLabel()) { "Capture" }
            }
            body
            completions
            controls
            capture
            TagRail(console = console)
        }
    }
}

/// The bulk surface: one want per line, `#tag` highlighted, Tab
/// completes against the registry.
fn bulk_editor(
    console: Console,
    handle: Ref<TextAreaHandle>,
    fragment: Signal<Option<String>>,
    highlight: Signal<usize>,
    caret: Signal<usize>,
) -> Element {
    let t = idea_theme::tokens();
    code_editor(console.draft, move |text| console.draft.set(text))
        .decorate(decorate)
        // ONE line, and that is a constraint rather than a preference:
        // the editing layer is stretched over the DECORATED layer, which
        // measures the buffer — one line tall while the buffer is empty.
        // A two-line placeholder has nowhere to go and scrolls itself
        // half out of view.
        .placeholder("One want per line. Tag with #ux — Tab completes.")
        .font(crate::styles::MONO, 13.0)
        .line_height(22.0)
        .padding(12.0)
        .text_color(t.color.text().resolve().0)
        .caret_color(t.intent.primary.fg().resolve().0)
        .on_key_down(move |e| {
            let text = console.draft.get();
            // How many rows the list is currently showing, which is
            // what the keyboard cursor may move within. 0 means there
            // is no list, and every key below falls through to the
            // editor untouched.
            let open = fragment.get().map(|f| shown(&f).len()).unwrap_or(0);
            match e.key.as_str() {
                "ArrowDown" if open > 0 => {
                    highlight.set((highlight.get() + 1).min(open - 1));
                    KeyOutcome::PreventDefault
                }
                "ArrowUp" if open > 0 => {
                    highlight.set(highlight.get().saturating_sub(1));
                    KeyOutcome::PreventDefault
                }
                "Escape" if open > 0 => {
                    close(fragment, highlight);
                    KeyOutcome::PreventDefault
                }
                // Enter is only ours while the list is up. Left alone
                // it starts the next want, which is the whole shape of
                // this buffer — swallowing it unconditionally would
                // make the composer a one-line field.
                "Tab" | "Enter"
                    if open > 0 && !e.shift && !e.ctrl && !e.meta && !e.alt =>
                {
                    // The caret on the event is the real one, and
                    // neither key has changed the buffer yet.
                    if accept(console, handle, fragment, highlight, &text, e.selection_start) {
                        KeyOutcome::PreventDefault
                    } else {
                        KeyOutcome::Default
                    }
                }
                // Tab with no list up keeps its old job: complete as
                // far as the matches agree, the shell rule. This is
                // the path a caret moved by MOUSE takes — no keystroke
                // has run since, so nothing has opened a list — and it
                // reads the real caret, so it is always right.
                "Tab" if !e.shift && !e.ctrl && !e.meta && !e.alt => {
                    match complete_tag(&text, e.selection_start) {
                        Some(insert) => {
                            handle.with(|h| h.insert_text(&insert));
                            KeyOutcome::PreventDefault
                        }
                        // No `#fragment` under the caret: leave Tab
                        // alone rather than swallowing the key.
                        None => KeyOutcome::Default,
                    }
                }
                // Any other key: let it through, and re-derive the
                // list from the buffer it is ABOUT to produce.
                // `on_key_down` fires before the edit lands, so
                // reading the buffer here would show the list for the
                // previous keystroke — one character behind, forever.
                _ => {
                    match after_key(&text, e) {
                        Some((next, at)) => open_at(fragment, highlight, caret, &next, at),
                        // A key we cannot predict (a paste, a chord,
                        // an arrow): drop the list rather than leave a
                        // stale one pointing at the wrong caret.
                        None => close(fragment, highlight),
                    }
                    KeyOutcome::Default
                }
            }
        })
        .bind(handle)
        // No border or background here: this node is the scroller's
        // CONTENT and grows with the text, so a border on it would slide
        // sideways with the longest line. Those live on the scroller
        // (`EditorBox`), which stays put. What this node does carry is
        // `flex_grow`, so a short line still fills the card's width —
        // otherwise the box hugs the text and the space beside it looks
        // like editor you can click into but isn't.
        .with_style(EditorSurface())
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

// ---------------------------------------------------------------------
// The completion list
// ---------------------------------------------------------------------

/// How many matches the list shows at once. Past this the panel stops
/// being something you scan and starts being something you scroll, and
/// a scrolled panel needs the keyboard cursor scrolled into view —
/// which no backend exposes a handle for. Capping instead keeps every
/// row the cursor can reach on screen, and the overflow line says the
/// way out is to keep typing.
const SHOWN: usize = 8;

/// Indices into [`model::tags`] whose slug starts with `fragment`,
/// in the registry's own most-used-first order.
fn matching(fragment: &str) -> Vec<usize> {
    model::tags()
        .iter()
        .enumerate()
        .filter(|(_, tag)| tag.name.starts_with(fragment))
        .map(|(i, _)| i)
        .collect()
}

/// The matches the list actually renders — and therefore the ones the
/// keyboard cursor is allowed to land on. Every caller clamps against
/// THIS, never against `matching`, or ArrowDown walks off the visible
/// list onto a row nobody can see.
fn shown(fragment: &str) -> Vec<usize> {
    let mut hits = matching(fragment);
    hits.truncate(SHOWN);
    hits
}

/// Take the list down.
fn close(fragment: Signal<Option<String>>, highlight: Signal<usize>) {
    fragment.set(None);
    highlight.set(0);
}

/// Point the list at the `#fragment` under `caret` in `text`, or take
/// it down when there is no fragment there or nothing matches it.
///
/// `caret` is in UTF-16 code units, the unit every caret API reports.
fn open_at(
    fragment: Signal<Option<String>>,
    highlight: Signal<usize>,
    at: Signal<usize>,
    text: &str,
    caret: usize,
) {
    let next = api::tag_fragment_at(text, api::utf16_to_byte(text, caret))
        .map(|(_, frag)| frag)
        .filter(|frag| !matching(frag).is_empty());
    at.set(caret);
    // The cursor indexes into a list that has just been replaced, so
    // it goes back to the top — but only when the fragment actually
    // CHANGED. Resetting on every keystroke would fight the arrow keys,
    // which do not change the fragment.
    if next != fragment.get() {
        highlight.set(0);
        fragment.set(next);
    }
}

/// The buffer and caret as they will be once `e` has been applied.
///
/// `on_key_down` runs BEFORE the platform edits the text, so a list
/// derived from the buffer as it stands is always one keystroke stale:
/// type `#u` and it offers the matches for `#`. Predicting the edit is
/// what makes the list track what you actually typed.
///
/// Only the two edits worth predicting are handled — a printable
/// character and Backspace. Everything else returns `None`, and the
/// caller drops the list rather than guessing.
fn after_key(text: &str, e: &KeyEvent) -> Option<(String, usize)> {
    let start = api::utf16_to_byte(text, e.selection_start);
    let end = api::utf16_to_byte(text, e.selection_end.max(e.selection_start));
    // The backends normalise to the web `KeyboardEvent.key` vocabulary:
    // a printable key IS its character, and every other key has a
    // multi-character name ("Tab", "ArrowUp", "Backspace"). A modifier
    // held down means the key is a chord, not typing.
    let printable = e.key.chars().count() == 1 && !e.ctrl && !e.meta;
    if printable {
        let mut next = String::with_capacity(text.len() + e.key.len());
        next.push_str(&text[..start]);
        next.push_str(&e.key);
        next.push_str(&text[end..]);
        let width: usize = e.key.chars().map(char::len_utf16).sum();
        return Some((next, e.selection_start + width));
    }
    // A modifier pressed on its OWN is not an edit and must not be
    // read as one: Shift held to type `#UX` arrives as its own keydown
    // first, and treating that as unpredictable would drop the list
    // between the Shift and the letter.
    if matches!(e.key.as_str(), "Shift" | "Control" | "Alt" | "Meta") {
        return Some((text.to_string(), e.selection_start));
    }
    if e.key != "Backspace" {
        return None;
    }
    // A selected range goes wholesale; otherwise one character back.
    if end > start {
        let mut next = String::with_capacity(text.len());
        next.push_str(&text[..start]);
        next.push_str(&text[end..]);
        return Some((next, e.selection_start));
    }
    if start == 0 {
        return Some((text.to_string(), 0));
    }
    let prev = text[..start].chars().next_back()?;
    let mut next = String::with_capacity(text.len());
    next.push_str(&text[..start - prev.len_utf8()]);
    next.push_str(&text[start..]);
    Some((next, e.selection_start - prev.len_utf16()))
}

/// Put the highlighted tag into the buffer. Returns whether it did.
///
/// ## Why the caret is placed two different ways
///
/// `TextAreaHandle::insert_text` is documented to leave the caret
/// after the text it inserted. On web it does not: it reaches
/// `setRangeText` with the default `"preserve"` selection mode, and
/// the spec's preserve rule moves a caret only when it sits AFTER the
/// replaced range. Ours sits exactly at it, so it stays put and the
/// completion lands on the far side of the cursor — you finish a tag
/// and then have to arrow past your own text. There is no selection
/// setter on the handle to correct it with, so app code cannot fix
/// this insert (see FRAMEWORK_FEEDBACK.md #9).
///
/// Completing at the END of the buffer can dodge it, and that is where
/// tag completion nearly always happens. Writing the whole buffer back
/// through the controlling `Signal` reaches `textarea.value`, whose
/// setter is specified to "move the text entry cursor position to the
/// end of the text control" — and at the end of the buffer, the end of
/// the text IS the end of the tag. So that case takes the signal and
/// lands the caret correctly.
///
/// Mid-buffer keeps `insert_text`. It is wrong about the caret today
/// and right the moment the backend passes `SelectionMode::End`, which
/// is the property that matters: the workaround above is confined to
/// the case where it cannot rot, and nothing here depends on the bug
/// staying broken.
fn accept(
    console: Console,
    handle: Ref<TextAreaHandle>,
    fragment: Signal<Option<String>>,
    highlight: Signal<usize>,
    text: &str,
    caret: usize,
) -> bool {
    let caret = api::utf16_to_byte(text, caret);
    let Some((_, frag)) = api::tag_fragment_at(text, caret) else {
        return false;
    };
    let hits = shown(&frag);
    if hits.is_empty() {
        return false;
    }
    let tags = model::tags();
    let pick = &tags[hits[highlight.get().min(hits.len() - 1)]].name;
    // The tail of the chosen tag, plus the space that ends it. The
    // fragment was lowercased on the way out of `tag_fragment_at` and
    // slugs are lowercase, so the byte lengths line up.
    let insert = format!("{} ", &pick[frag.len()..]);
    if caret == text.len() {
        console.draft.set(format!("{text}{insert}"));
    } else {
        handle.with(|h| h.insert_text(&insert));
    }
    close(fragment, highlight);
    true
}

/// The anchored panel of matches for `fragment`.
///
/// `preserves_focus` on the surface is what lets a row be CLICKED: a
/// press inside it must not blur the editor, or the caret the accept
/// path reads is gone before the press resolves.
fn completion_list(
    console: Console,
    handle: Ref<TextAreaHandle>,
    anchor: Ref<ViewHandle>,
    fragment: Signal<Option<String>>,
    highlight: Signal<usize>,
    caret: Signal<usize>,
    frag: &str,
) -> Element {
    let all = matching(frag);
    if all.is_empty() {
        return ui! { view {} };
    }
    let hits = shown(frag);
    let mut rows: Vec<Element> = hits
        .iter()
        .enumerate()
        .map(|(pos, &tag)| completion_row(console, handle, fragment, highlight, caret, pos, tag))
        .collect();
    if all.len() > hits.len() {
        // A count, not an instruction about how the panel works: it
        // says how much is out of sight, which is the fact that makes
        // typing another letter worth doing (rule 19).
        // Built with the `text(..)` builder like every row beside it,
        // rather than a `ui!` block pushed into the vec: this list is
        // assembled in Rust, and mixing the two spellings is what
        // `prefer-keyed-list` reads as a hand-built child list.
        rows.push(
            text(format!("+{} more", all.len() - hits.len()))
                .with_style(MoreLine())
                .into_element(),
        );
    }
    let panel = view(rows)
        .with_style(PanelBox())
        .preserves_focus(true)
        .into_element();
    anchored_overlay(AnchorTarget::from(anchor), vec![panel])
        .side(ElementSide::Below)
        .align(ElementAlign::Start)
        .offset(4.0)
        // No scrim and no focus trap: the editor behind stays live and
        // keeps the caret, which is the whole point of the surface.
        .backdrop(BackdropMode::None)
        .trap_focus(false)
        // Clicking anywhere off the panel — including back into the
        // editor to move the caret — takes it down. That is also what
        // keeps a list from lingering after a MOUSE caret move, which
        // fires no key event for us to notice.
        .on_dismiss(move || close(fragment, highlight))
        .into_element()
}

/// One row: the tag as it will be written, and how many wants already
/// carry it — the fact that separates two tags whose names do not.
fn completion_row(
    console: Console,
    handle: Ref<TextAreaHandle>,
    fragment: Signal<Option<String>>,
    highlight: Signal<usize>,
    caret: Signal<usize>,
    position: usize,
    tag: usize,
) -> Element {
    let tags = model::tags();
    let tag = &tags[tag];
    let name = format!("#{}", tag.name);
    let uses = if tag.uses > 0 {
        tag.uses.to_string()
    } else {
        String::new()
    };
    let inner: Element = ui! {
        view(style = RowLine()) {
            text(style = RowName()) { name }
            text(style = RowUses()) { uses }
        }
    };
    pressable(vec![inner], move || {
        // A press carries no caret, so it accepts against the one that
        // opened this list. That is still the live caret: anything
        // that could have moved it since — a keystroke, a click in the
        // editor — takes the panel down first.
        highlight.set(position);
        accept(console, handle, fragment, highlight, &console.draft.get(), caret.get());
    })
    .preserves_focus(true)
    .with_style(move || {
        StyleApplication::new(completion_row_style()).with(
            "cursor",
            if highlight.get() == position { "on" } else { "off" }.to_string(),
        )
    })
    .into_element()
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

/// The line under the composer: what the last capture did, or — with
/// nothing to report yet — what pressing the button will do.
///
/// Called from `rx!`, so the signals it reads are what make the slot
/// live; it must not be handed a snapshot instead.
fn note(console: Console) -> String {
    let status = console.status.get();
    if !status.is_empty() {
        return status;
    }
    summarize(&api::parse_buffer(&console.draft.get()))
}

/// The button's own label, counting what is in the buffer (rule 16:
/// what the action does is the label, not a line above it).
fn submit_label(console: Console) -> String {
    match api::parse_buffer(&console.draft.get()).len() {
        0 => "Add want".to_string(),
        1 => "Add 1 want".to_string(),
        n => format!("Add {n} wants"),
    }
}

/// Whether there is something to send and nothing already in flight.
fn submit_ready(console: Console) -> bool {
    !console.busy.get() && !api::parse_buffer(&console.draft.get()).is_empty()
}

/// The pending half of [`note`]: what pressing the button will do.
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
            // The editor is as wide as its longest line. `min_width: 0`
            // is what lets this column size below that content so the
            // scroller inside gets a definite width to scroll against;
            // `Hidden` is the backstop for a line with no break
            // opportunity in it at all (rule 22).
            min_width: 0,
            overflow: runtime_core::Overflow::Hidden,
        }
    }
}

// The scroller's content row. No `min_width: 0` anywhere from here
// inward — that floor is exactly what makes the editor's box take the
// width of its longest line instead of the card's.
stylesheet! {
    pub EditorRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
        }
    }
}

stylesheet! {
    pub EditorSurface<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
        }
    }
}

// The bordered surface AND the horizontal scroller: one node, because a
// border around a scroller keeps the scrollbar inside the box, and the
// content the bar belongs to is the editor. No `min_height` — the
// editor's own height is the buffer's (one line while it is empty), and
// a floor here only adds box that no click can reach: the editing layer
// is stretched over the decorated one, not over this.
stylesheet! {
    pub EditorBox<IdeaThemeRef> {
        base(t) {
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.sm(),
            background: t.color.background(),
            min_width: 0,
        }
    }
}

// A hole is a DRIVER or a portal host, not layout: taken out of flow
// so its zero-size node cannot collect one of the card's column gaps.
// Used by the capture request and by the completion panel, which both
// sit in the card's column while occupying none of it.
stylesheet! {
    pub CaptureHole<IdeaThemeRef> {
        base(t) {
            position: runtime_core::Position::Absolute,
            width: 0,
            height: 0,
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
    use super::{after_key, complete_tag, matching, shown, KeyEvent, SHOWN};
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

    /// The list is derived from the buffer the keystroke is ABOUT to
    /// produce, because `on_key_down` runs before the edit lands.
    #[test]
    fn a_predicted_keystroke_is_what_the_list_matches_on() {
        let key = |k: &str, caret: usize| KeyEvent {
            key: k.to_string(),
            shift: false,
            ctrl: false,
            alt: false,
            meta: false,
            selection_start: caret,
            selection_end: caret,
        };

        // Typing `x` at the end: the list must see `#ux`, not `#u`.
        assert_eq!(
            after_key("tagged #u", &key("x", 9)),
            Some(("tagged #ux".to_string(), 10))
        );

        // Backspace shortens it by exactly one character.
        assert_eq!(
            after_key("tagged #ux", &key("Backspace", 10)),
            Some(("tagged #u".to_string(), 9))
        );

        // A selected range is replaced wholesale by the typed key.
        let mut sel = key("z", 7);
        sel.selection_end = 10;
        assert_eq!(after_key("tagged #ux", &sel), Some(("tagged z".to_string(), 8)));

        // Offsets are UTF-16, so an emoji ahead of the caret must not
        // shift the splice. The emoji is 2 units and 4 bytes.
        assert_eq!(
            after_key("🙂 #u", &key("x", 5)),
            Some(("🙂 #ux".to_string(), 6))
        );

        // A modifier on its own leaves the buffer — and the list —
        // exactly as they were.
        assert_eq!(
            after_key("tagged #u", &key("Shift", 9)),
            Some(("tagged #u".to_string(), 9))
        );

        // Anything else is unpredictable, and the caller drops the list.
        assert_eq!(after_key("tagged #u", &key("ArrowLeft", 9)), None);
    }

    /// What the panel renders is what the arrow keys may land on: the
    /// cursor is clamped against the SHOWN list, never the full one.
    #[test]
    fn the_list_shows_most_used_first_and_caps_itself() {
        with_tags(&["billing", "billing-export", "ux", "ops"]);

        // Registry order is preserved — `model::tags()` is already
        // most-used first, so the panel does not re-sort it.
        assert_eq!(matching("bil"), vec![0, 1]);
        assert_eq!(matching("u"), vec![2]);
        assert!(matching("zzz").is_empty());

        // A bare `#` matches everything, which is a real state: the
        // caret is just past the hash and the whole registry is on
        // offer.
        assert_eq!(matching("").len(), 4);

        // Past the cap the list stops growing, so every row the cursor
        // can reach is a row on screen.
        let many: Vec<String> = (0..12).map(|i| format!("tag-{i}")).collect();
        let names: Vec<&str> = many.iter().map(String::as_str).collect();
        with_tags(&names);
        assert_eq!(matching("tag").len(), 12);
        assert_eq!(shown("tag").len(), SHOWN);
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

// The completion panel. Tight by rule 2 — a popover is a compact
// surface, not a card — so the padding is the smallest token and the
// rows carry their own. `min_width` keeps it from shrinking to the
// width of the shortest slug as you type, which reads as the panel
// twitching under the caret (rule 18).
stylesheet! {
    pub PanelBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            padding: t.spacing.xs(),
            min_width: 180,
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.sm(),
            background: t.color.surface(),
        }
    }
}

// The slug and its use count. Rule 22's two slots: the name may shrink
// and wrap, the count may not — a number clipped to one digit is worse
// than a slug on two lines.
stylesheet! {
    pub RowLine<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
            justify_content: JustifyContent::SpaceBetween,
            overflow: runtime_core::Overflow::Hidden,
        }
    }
}

stylesheet! {
    pub RowName<IdeaThemeRef> {
        base(t) {
            min_width: 0,
            flex_shrink: 1.0,
            font_family: crate::styles::MONO,
            font_size: 13,
            color: t.color.text(),
        }
    }
}

stylesheet! {
    pub RowUses<IdeaThemeRef> {
        base(t) {
            flex_shrink: 0.0,
            font_size: 12,
            color: t.color.text_muted(),
        }
    }
}

stylesheet! {
    pub MoreLine<IdeaThemeRef> {
        base(t) {
            padding_horizontal: t.spacing.xs(),
            padding_vertical: t.spacing.xs(),
            font_size: 12,
            color: t.color.text_muted(),
        }
    }
}

// The keyboard cursor and the hover are the SAME look on purpose: the
// row under the pointer and the row under the arrow keys are the same
// affordance, and painting them differently would put two candidate
// rows on screen at once.
stylesheet! {
    pub CompletionRow<IdeaThemeRef> {
        base(t) {
            padding_horizontal: t.spacing.xs(),
            padding_vertical: t.spacing.xs(),
            border_radius: t.radius.sm(),
            cursor: Cursor::Pointer,
        }
        variant cursor {
            #[default]
            off(t) {
                background: t.color.surface(),
            }
            on(t) {
                background: t.intent.primary.soft_bg(),
            }
        }
        transitions {
            background: 160ms EaseOut,
            border_color: 160ms EaseOut,
            opacity: 160ms EaseOut,
        }
        state hovered(t) {
            background: t.intent.primary.soft_bg(),
        }
    }
}

stylesheet! {
    pub ChipPress<IdeaThemeRef> {
        base(t) {
            border_radius: t.radius.pill(),
            cursor: Cursor::Pointer,
            opacity: 1.0,
        }
        transitions {
            background: 160ms EaseOut,
            border_color: 160ms EaseOut,
            opacity: 160ms EaseOut,
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
