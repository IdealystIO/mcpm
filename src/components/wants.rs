//! The want pool: the project-level inbox of loose ideas, and the
//! record of which ones were composed into which features.
//!
//! A filtered, paginated table — the pool grows without bound, so it
//! gets a toolbar (text, status, tags) and pages rather than an
//! ever-longer wall. Each row is a handle: the want in its author's
//! words plus what you filter by, with every other property one click
//! away in the want drawer.
//!
//! Capture is the one write here; composition and declines are MCP
//! calls (`promote_wants`, `update_want`).

use std::rc::Rc;

use idea_ui::{tone, typography_kind, variant, Button, Field, IdeaThemeRef, SegmentOption,
    SegmentedControl, Spacer, Table, TableCell, TableRow, Tag, Typography};
use runtime_core::{
    component, pressable, rx, stylesheet, switch, ui, AlignItems, Element, FlexDirection,
    FlexWrap, FontWeight, IdealystSchema, IntoElement, JustifyContent, StyleApplication,
};

use crate::components::bits::StatusDot;
use crate::components::composer::Composer;
use crate::model::{filter_wants, want_counts, wants, WantState};
use crate::state::Console;
use crate::styles::SectionLabel;

/// Rows per page. Rule 5 of UX_GUIDELINES: data tables page, they do
/// not render unbounded.
const PAGE_SIZE: usize = 20;

/// Props for [`WantsView`].
#[derive(Default, IdealystSchema)]
pub struct WantsViewProps {
    /// Console state handles.
    pub console: Console,
}

/// The pool: capture card, toolbar, table, pager.
///
/// The data-dependent parts sit inside `switch`es keyed on the poll
/// revision and the filters; the [`Composer`] deliberately does not, so
/// an agent's write landing mid-sentence cannot rebuild the text node
/// you are typing into.
#[component]
pub fn WantsView(props: &WantsViewProps) -> Element {
    let console = props.console;

    let head = switch(
        move || console.rev.get(),
        move |_rev: &u64| {
            let (loose, composed, declined) = want_counts();
            ui! {
                view(style = StatRow()) {
                    PoolStat(value = format!("{loose}"), label = "loose")
                    PoolStat(value = format!("{composed}"), label = "composed")
                    PoolStat(value = format!("{declined}"), label = "declined")
                }
            }
        },
    );

    let table = switch(
        move || {
            (
                console.rev.get(),
                console.pool_query.get(),
                console.pool_status.get(),
                console.pool_tags.get(),
                console.pool_page.get(),
            )
        },
        move |state: &(u64, String, String, Vec<String>, usize)| {
            let (_rev, query, status, tags, page) = state.clone();
            let matched = filter_wants(&query, &status, &tags);
            let total = matched.len();
            let pages = total.div_ceil(PAGE_SIZE).max(1);
            // A filter change resets the page, but a poll can shrink the
            // result set under a page that is already showing.
            let page = page.min(pages - 1);
            let start = page * PAGE_SIZE;
            let visible: Vec<usize> = matched.iter().skip(start).take(PAGE_SIZE).copied().collect();
            let shown = visible.len();
            let empty = shown == 0;
            let summary = if total == 0 {
                "No wants match".to_string()
            } else {
                format!("{}\u{2013}{} of {total}", start + 1, start + shown)
            };
            ui! {
                view(style = TableCol()) {
                    Table {
                        TableRow {
                            TableCell(header = true, text = Some("Want".to_string()))
                            TableCell(header = true, text = Some("Tags".to_string()))
                            TableCell(header = true, text = Some("State".to_string()))
                            TableCell(header = true, text = Some("Captured".to_string()))
                        }
                        if empty {
                            TableRow {
                                TableCell(text = Some("No wants match".to_string()))
                                TableCell(text = Some(String::new()))
                                TableCell(text = Some(String::new()))
                                TableCell(text = Some(String::new()))
                            }
                        }
                        for i in 0..shown {
                            WantRow(console = console, want = visible[i])
                        }
                    }
                    Pager(console = console, summary = summary, page = page, pages = pages)
                }
            }
        },
    );

    ui! {
        view(style = PoolBox()) {
            view(style = PoolHead()) {
                Typography(
                    content = "Want pool",
                    kind = typography_kind::H2,
                    weight = Some(FontWeight::SemiBold),
                )
                head
            }
            scroll_view(style = PoolScroll()) {
                view(style = PoolPad()) {
                    Composer(console = console)
                    Toolbar(console = console)
                    table
                }
            }
        }
    }
}

/// Props for [`Toolbar`].
#[derive(Default, IdealystSchema)]
pub struct ToolbarProps {
    /// Console state handles.
    pub console: Console,
}

/// Text, status and tag filters over the pool. Text and status hold
/// their own signals, so only the tag rail is rebuilt on a poll.
#[component]
pub fn Toolbar(props: &ToolbarProps) -> Element {
    let console = props.console;
    let on_query: Rc<dyn Fn(String)> = Rc::new(move |t| console.set_pool_query(t));
    let on_status: Rc<dyn Fn(String)> = Rc::new(move |id| console.set_pool_status(id));

    let tag_row = switch(
        move || (console.rev.get(), console.pool_tags.get()),
        move |state: &(u64, Vec<String>)| {
            let (_rev, active) = state.clone();
            let count = crate::model::tags().len();
            if count == 0 {
                return ui! { view {} };
            }
            ui! {
                view(style = FilterTags()) {
                    for i in 0..count {
                        FilterTag(console = console, index = i, active = active.clone())
                    }
                }
            }
        },
    );

    let clear = switch(
        move || {
            (
                console.pool_query.get(),
                console.pool_status.get(),
                console.pool_tags.get(),
            )
        },
        move |state: &(String, String, Vec<String>)| {
            let (query, status, tags) = state.clone();
            let filtered = !query.is_empty() || status != "all" || !tags.is_empty();
            if !filtered {
                return ui! { view {} };
            }
            let on_click: Rc<dyn Fn()> = Rc::new(move || console.clear_pool_filters());
            ui! {
                Button(
                    label = "Clear filters",
                    on_click = on_click,
                    size = idea_ui::size::Sm,
                    variant = variant::Ghost,
                )
            }
        },
    );

    ui! {
        view(style = ToolbarBox()) {
            view(style = ToolbarRow()) {
                view(style = SearchSlot()) {
                    Field(
                        value = console.pool_query,
                        on_change = on_query,
                        placeholder = Some("Search wants".to_string()),
                    )
                }
                SegmentedControl(
                    value = rx!(console.pool_status.get()),
                    on_change = on_status,
                    options = vec![
                        SegmentOption::new("all", "All"),
                        SegmentOption::new("open", "Loose"),
                        SegmentOption::new("promoted", "Composed"),
                        SegmentOption::new("declined", "Declined"),
                    ],
                )
                Spacer()
                clear
            }
            tag_row
        }
    }
}

/// Props for [`FilterTag`].
#[derive(Default, IdealystSchema)]
pub struct FilterTagProps {
    /// Console state handles.
    pub console: Console,
    /// Index into [`crate::model::tags`].
    pub index: usize,
    /// The tags currently filtered on.
    pub active: Vec<String>,
}

/// One tag in the filter rail. Solid means it is narrowing the table.
#[component]
pub fn FilterTag(props: &FilterTagProps) -> Element {
    let console = props.console;
    let tags = crate::model::tags();
    let tag = &tags[props.index];
    let name = tag.name.clone();
    let on = props.active.iter().any(|t| *t == name);
    let label = tag.label.clone();
    let inner: Element = if on {
        ui! { Tag(label = label, tone = tone::Primary, variant = variant::Solid) }
    } else {
        ui! { Tag(label = label, tone = tone::Neutral, variant = variant::Soft) }
    };
    let pick = name.clone();
    pressable(vec![inner], move || console.toggle_pool_tag(&pick))
        .with_style(StyleApplication::new(chip_press_style()))
        .into_element()
}

/// Props for [`WantRow`].
#[derive(Default, IdealystSchema)]
pub struct WantRowProps {
    /// Console state handles.
    pub console: Console,
    /// Index into [`crate::model::wants`].
    pub want: usize,
}

/// One row of the pool. Clicking it opens the want drawer, which is
/// where the id, author, decline reason and per-feature rationale live
/// — a row carries the idea and what you filter by, nothing else.
#[component]
pub fn WantRow(props: &WantRowProps) -> Element {
    let console = props.console;
    let index = props.want;
    let pool = wants();
    let w = &pool[index];
    let id = w.id.clone();
    let body = w.body.clone();
    let captured = w.captured.clone();
    let state = w.state;
    let status = state.status();
    let state_word = match state {
        WantState::Open => "Loose",
        WantState::Promoted => "Composed",
        WantState::Declined => "Declined",
    };
    let tag_count = w.tags.len();
    let on_row_click: Rc<dyn Fn()> = Rc::new(move || console.open_want(&id));

    ui! {
        TableRow(on_row_click = Some(on_row_click)) {
            TableCell(text = Some(body))
            TableCell {
                view(style = TagRow()) {
                    for i in 0..tag_count {
                        WantTag(want = index, index = i)
                    }
                }
            }
            TableCell {
                view(style = StateCell()) {
                    StatusDot(status = status)
                    Typography(content = state_word, kind = typography_kind::BodySm)
                }
            }
            TableCell(text = Some(captured))
        }
    }
}

/// Props for [`WantTag`].
#[derive(Default, IdealystSchema)]
pub struct WantTagProps {
    /// Index into [`crate::model::wants`].
    pub want: usize,
    /// Index into that want's tag list.
    pub index: usize,
}

/// One filing label on a want, re-read from the model because the `ui!`
/// for-each body is an `Fn` closure and cannot consume a `String`.
#[component]
pub fn WantTag(props: &WantTagProps) -> Element {
    let pool = wants();
    let tag = pool[props.want].tags[props.index].clone();
    ui! {
        Tag(label = tag, tone = tone::Neutral, variant = variant::Soft)
    }
}

/// Props for [`Pager`].
#[derive(Default, IdealystSchema)]
pub struct PagerProps {
    /// Console state handles.
    pub console: Console,
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
#[component]
pub fn Pager(props: &PagerProps) -> Element {
    let console = props.console;
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
    let back = pressable(
        vec![ui! { text(style = back_arm) { "\u{2039}" } }],
        move || {
            if page > 0 {
                console.set_pool_page(page - 1);
            }
        },
    )
    .with_style(StyleApplication::new(pager_button_style()))
    .into_element();
    let next = pressable(
        vec![ui! { text(style = next_arm) { "\u{203a}" } }],
        move || {
            if page + 1 < pages {
                console.set_pool_page(page + 1);
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

/// Props for [`PoolStat`].
#[derive(Default, IdealystSchema)]
pub struct PoolStatProps {
    /// Stat value.
    pub value: String,
    /// Uppercase label.
    pub label: &'static str,
}

/// One count in the pool header.
#[component]
pub fn PoolStat(props: &PoolStatProps) -> Element {
    let value = props.value.clone();
    let label = props.label;
    ui! {
        view(style = StatCol()) {
            Typography(
                content = value,
                kind = typography_kind::BodyLg,
                weight = Some(FontWeight::SemiBold),
            )
            text(style = SectionLabel()) { label }
        }
    }
}

stylesheet! {
    pub PoolBox<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            min_width: 0,
            min_height: 0,
            flex_direction: FlexDirection::Column,
            background: t.color.background(),
        }
    }
}

stylesheet! {
    pub PoolHead<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexEnd,
            justify_content: JustifyContent::SpaceBetween,
            flex_wrap: FlexWrap::Wrap,
            gap: t.spacing.lg(),
            padding_vertical: t.spacing.lg(),
            padding_horizontal: t.spacing.xl(),
            border_bottom_width: 1.0,
            border_color: t.color.border(),
            background: t.color.surface(),
        }
    }
}

stylesheet! {
    pub HeadLeft<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
            min_width: 0,
            max_width: 620,
        }
    }
}

stylesheet! {
    pub StatRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            justify_content: JustifyContent::FlexEnd,
            gap: t.spacing.xl(),
        }
    }
}

stylesheet! {
    pub StatCol<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
            align_items: AlignItems::FlexEnd,
        }
    }
}

stylesheet! {
    pub PoolScroll<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            min_height: 0,
        }
    }
}

stylesheet! {
    pub PoolPad<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xl(),
            padding: t.spacing.xl(),
        }
    }
}






stylesheet! {
    pub TagRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            gap: t.spacing.xs(),
        }
    }
}

stylesheet! {
    pub ToolbarBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
        }
    }
}

stylesheet! {
    pub ToolbarRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            flex_wrap: FlexWrap::Wrap,
            gap: t.spacing.sm(),
        }
    }
}

stylesheet! {
    pub SearchSlot<IdeaThemeRef> {
        base(t) {
            width: 260,
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    pub FilterTags<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            gap: t.spacing.xs(),
        }
    }
}

stylesheet! {
    pub ChipPress<IdeaThemeRef> {
        base(t) {
            border_radius: t.radius.sm(),
            cursor: runtime_core::Cursor::Pointer,
        }
    }
}

stylesheet! {
    pub TableCol<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
        }
    }
}

stylesheet! {
    pub StateCell<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.xs(),
        }
    }
}

stylesheet! {
    pub PagerBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
        }
    }
}

stylesheet! {
    pub PagerButton<IdeaThemeRef> {
        base(t) {
            width: 24,
            height: 24,
            border_radius: t.radius.sm(),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            cursor: runtime_core::Cursor::Pointer,
        }
        state hovered(t) {
            background: t.color.surface_alt(),
        }
    }
}

stylesheet! {
    pub Chevron<IdeaThemeRef> {
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
