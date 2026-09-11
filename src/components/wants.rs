//! The want pool and the capture screen.
//!
//! Two screens for two jobs. The **pool** is the project-level inbox
//! of loose ideas and the record of which ones were composed into
//! which features: a filtered, paginated table — the pool grows
//! without bound, so it gets a search field, a filter menu and pages
//! rather than an ever-longer wall. Each row is a handle: the want in
//! its author's words plus what you filter by, with every other
//! property one click away in the want drawer. The **capture** screen
//! is the composer, with the tag registry beside it for filing.
//!
//! They were one screen once, and the tag registry rendered twice on
//! it — once as filing labels for the composer, once as filter chips
//! for the table — a hundred chips each. The filter's tags now live in
//! the filter menu, searchable, and the composer's rail is the only
//! place the whole registry shows.

use std::rc::Rc;

use idea_ui::{size, tone, typography_kind, variant, Button, Field, IdeaThemeRef, Menu,
    MenuItem, MenuLabel, MenuSeparator, Spacer, Table, TableCell, TableRow, Tag, Typography};
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide};
use runtime_core::{
    component, pressable, stylesheet, switch, ui, AlignItems, Element, FlexDirection, FlexWrap,
    FontWeight, IdealystSchema, IntoElement, JustifyContent, PressableHandle, Ref,
    StyleApplication,
};

use crate::components::bits::{Pager, StatusDot};
use crate::components::composer::Composer;
use crate::model::{want_counts, want_total, wants, WantState};
use crate::state::Console;
use crate::styles::SectionLabel;

/// Rows per page. Rule 5 of UX_GUIDELINES: data tables page, they do
/// not render unbounded.
const PAGE_SIZE: usize = crate::app::POOL_PAGE;

/// The three states a want can be in, as the filter names them.
const STATES: [(&str, &str); 3] = [
    ("open", "Loose"),
    ("promoted", "Composed"),
    ("declined", "Declined"),
];

fn state_label(status: &str) -> &'static str {
    STATES.iter().find(|(id, _)| *id == status).map(|(_, l)| *l).unwrap_or("")
}

/// Props for [`WantsView`].
#[derive(Default, IdealystSchema)]
pub struct WantsViewProps {
    /// Console state handles.
    pub console: Console,
}

/// The pool: counts, toolbar, table, pager.
#[component]
pub fn WantsView(props: &WantsViewProps) -> Element {
    let console = props.console;
    let to_capture: Rc<dyn Fn()> = Rc::new(move || console.show_capture());

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
        move || (console.rev.get(), console.pool_page.get()),
        move |state: &(u64, usize)| {
            let (_rev, page) = *state;
            // The page on screen is whatever the server sent for the
            // filters — paged there, against the text index, because
            // the pool grows without bound (see `api::search_wants`).
            let total = want_total();
            let pages = total.div_ceil(PAGE_SIZE).max(1);
            // A filter change resets the page, but a tick can shrink
            // the result set under a page that is already showing.
            let page = page.min(pages - 1);
            let start = page * PAGE_SIZE;
            let shown = wants().len();
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
                        // The blank state goes INSIDE the table shell,
                        // never a lone sentence where the table was.
                        if empty {
                            TableRow {
                                TableCell(text = Some("No wants match".to_string()))
                                TableCell(text = Some(String::new()))
                                TableCell(text = Some(String::new()))
                                TableCell(text = Some(String::new()))
                            }
                        }
                        for i in 0..shown {
                            WantRow(console = console, want = i)
                        }
                    }
                    Pager(
                        on_page = Some(Rc::new(move |p| console.set_pool_page(p))
                            as Rc<dyn Fn(usize)>),
                        summary = summary,
                        page = page,
                        pages = pages,
                    )
                }
            }
        },
    );

    ui! {
        view(style = PoolBox()) {
            view(style = PoolHead()) {
                view(style = HeadLeft()) {
                    Typography(
                        content = "Want pool",
                        kind = typography_kind::H2,
                        weight = Some(FontWeight::SemiBold),
                    )
                }
                view(style = HeadRight()) {
                    head
                    Button(label = "Capture wants", on_click = to_capture, size = size::Sm)
                }
            }
            scroll_view(style = PoolScroll()) {
                view(style = PoolPad()) {
                    Toolbar(console = console)
                    table
                }
            }
        }
    }
}

/// Props for [`CaptureView`].
#[derive(Default, IdealystSchema)]
pub struct CaptureViewProps {
    /// Console state handles.
    pub console: Console,
}

/// The capture screen: the composer, and the way back to the pool.
///
/// The [`Composer`] deliberately sits in no `switch` keyed on the poll
/// revision, so an agent's write landing mid-sentence cannot rebuild
/// the text node you are typing into.
#[component]
pub fn CaptureView(props: &CaptureViewProps) -> Element {
    let console = props.console;
    let to_pool: Rc<dyn Fn()> = Rc::new(move || console.show_wants());
    ui! {
        view(style = PoolBox()) {
            view(style = PoolHead()) {
                view(style = HeadLeft()) {
                    Typography(
                        content = "Capture",
                        kind = typography_kind::H2,
                        weight = Some(FontWeight::SemiBold),
                    )
                }
                view(style = HeadRight()) {
                    Button(label = "Open the pool", on_click = to_pool, size = size::Sm, variant = variant::Soft)
                }
            }
            scroll_view(style = PoolScroll()) {
                view(style = PoolPad()) {
                    Composer(console = console)
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

/// Search, then the filter menu, then the active filters as chips —
/// the one toolbar grammar (UX_GUIDELINES rule 6). The search field
/// holds its own signal, so only the chips rebuild on a change.
#[component]
pub fn Toolbar(props: &ToolbarProps) -> Element {
    let console = props.console;
    let on_query: Rc<dyn Fn(String)> = Rc::new(move |t| console.set_pool_query(t));

    let chips = switch(
        move || (console.pool_status.get(), console.pool_tags.get()),
        move |state: &(Vec<String>, Vec<String>)| {
            let (statuses, tags) = state.clone();
            let (ns, nt) = (statuses.len(), tags.len());
            ui! {
                view(style = ChipRow()) {
                    for i in 0..ns {
                        ActiveChip(console = console, id = statuses[i].clone(), label = state_label(&statuses[i]).to_string(), tag = false)
                    }
                    for i in 0..nt {
                        ActiveChip(console = console, id = tags[i].clone(), label = format!("#{}", tags[i]), tag = true)
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
        move |state: &(String, Vec<String>, Vec<String>)| {
            let (query, statuses, tags) = state.clone();
            let default = statuses == vec!["open".to_string()];
            if query.is_empty() && default && tags.is_empty() {
                return ui! { view {} };
            }
            let on_click: Rc<dyn Fn()> = Rc::new(move || console.clear_pool_filters());
            ui! {
                Button(
                    label = "Clear filters",
                    on_click = on_click,
                    size = size::Sm,
                    variant = variant::Ghost,
                )
            }
        },
    );

    ui! {
        view(style = ToolbarRow()) {
            view(style = SearchSlot()) {
                Field(
                    value = console.pool_query,
                    on_change = on_query,
                    placeholder = Some("Search wants".to_string()),
                )
            }
            FilterMenu(console = console)
            chips
            Spacer()
            clear
        }
    }
}

/// Props for [`FilterMenu`].
#[derive(Default, IdealystSchema)]
pub struct FilterMenuProps {
    /// Console state handles.
    pub console: Console,
}

/// The Filter button and its menu: the three states as toggles, then
/// the tag registry, searchable. Toggling keeps the menu open — a
/// reader narrowing to two tags should not have to reopen it between
/// them.
#[component]
pub fn FilterMenu(props: &FilterMenuProps) -> Element {
    let console = props.console;
    let trigger: Ref<PressableHandle> = Ref::new();
    let on_open: Rc<dyn Fn()> = Rc::new(move || {
        console.pool_tag_query.set(String::new());
        console.pool_filter_open.update(|open| !open);
    });
    let button: Element = ui! {
        Button(
            label = "Filter",
            on_click = on_open,
            size = size::Sm,
            variant = variant::Soft,
            bind_to = Some(trigger),
        )
    };

    // The state toggles rebuild as the set changes; the tag rows sit
    // in their own switch on the search, so typing into the search
    // field rebuilds the rows and never the field.
    let menu = switch(
        move || (console.pool_filter_open.get(), console.pool_status.get()),
        move |state: &(bool, Vec<String>)| {
            let (open, statuses) = state.clone();
            if !open {
                return ui! { view {} };
            }
            let dismiss: Rc<dyn Fn()> = Rc::new(move || console.pool_filter_open.set(false));
            let on_tag_query: Rc<dyn Fn(String)> = Rc::new(move |t| console.pool_tag_query.set(t));
            let tag_rows = switch(
                move || (console.pool_tag_query.get(), console.pool_tags.get(), console.rev.get()),
                move |state: &(String, Vec<String>, u64)| {
                    let (query, active, _rev) = state.clone();
                    let needle = query.trim().to_lowercase();
                    let tags = crate::model::tags();
                    let matched: Vec<usize> = tags
                        .iter()
                        .enumerate()
                        .filter(|(_, t)| needle.is_empty() || t.name.contains(&needle))
                        .map(|(i, _)| i)
                        // A menu, not a wall: the search is how the
                        // rest are reached.
                        .take(12)
                        .collect();
                    let n = matched.len();
                    ui! {
                        view(style = MenuRows()) {
                            if n == 0 {
                                view(style = MenuBlank()) {
                                    Typography(content = "No tags match.", kind = typography_kind::Caption, muted = true)
                                }
                            }
                            for i in 0..n {
                                TagToggle(console = console, index = matched[i], active = active.clone())
                            }
                        }
                    }
                },
            );
            ui! {
                Menu(
                    target = Some(AnchorTarget::from(trigger)),
                    on_dismiss = Some(dismiss.clone()),
                    side = ElementSide::Below,
                    align = ElementAlign::Start,
                ) {
                    MenuLabel(text = "State")
                    for i in 0..STATES.len() {
                        StateToggle(
                            console = console,
                            id = STATES[i].0,
                            label = STATES[i].1,
                            active = statuses.iter().any(|s| s == STATES[i].0),
                        )
                    }
                    MenuSeparator()
                    MenuLabel(text = "Tags")
                    view(style = MenuSearch()) {
                        Field(
                            value = console.pool_tag_query,
                            on_change = on_tag_query,
                            placeholder = Some("Find a tag".to_string()),
                            size = idea_ui::FieldSize::Sm,
                        )
                    }
                    tag_rows
                }
            }
        },
    );

    ui! {
        view(style = FilterAnchor()) {
            button
            menu
        }
    }
}

/// Props for [`StateToggle`].
#[derive(Default, IdealystSchema)]
pub struct StateToggleProps {
    /// Console state handles.
    pub console: Console,
    /// The status id.
    pub id: &'static str,
    /// Its label.
    pub label: &'static str,
    /// Whether it is in the filter.
    pub active: bool,
}

/// One state row in the filter menu.
#[component]
pub fn StateToggle(props: &StateToggleProps) -> Element {
    let console = props.console;
    let id = props.id;
    let active = props.active;
    let on_select: Rc<dyn Fn()> = Rc::new(move || console.toggle_pool_status(id));
    let leading: Option<Element> = Some(check_mark(active));
    ui! {
        MenuItem(label = props.label.to_string(), on_select = on_select, leading = leading, active = active)
    }
}

/// Props for [`TagToggle`].
#[derive(Default, IdealystSchema)]
pub struct TagToggleProps {
    /// Console state handles.
    pub console: Console,
    /// Index into [`crate::model::tags`].
    pub index: usize,
    /// The tags currently filtered on.
    pub active: Vec<String>,
}

/// One tag row in the filter menu.
#[component]
pub fn TagToggle(props: &TagToggleProps) -> Element {
    let console = props.console;
    let tags = crate::model::tags();
    let Some(tag) = tags.get(props.index) else {
        return ui! { view {} };
    };
    let name = tag.name.clone();
    let on = props.active.iter().any(|t| *t == name);
    let label = format!("{} \u{b7} {}", tag.label, tag.uses);
    let pick = name.clone();
    let on_select: Rc<dyn Fn()> = Rc::new(move || console.toggle_pool_tag(&pick));
    let leading: Option<Element> = Some(check_mark(on));
    ui! {
        MenuItem(label = label, on_select = on_select, leading = leading, active = on)
    }
}

/// The toggle's mark: a check when on, the same width of nothing when
/// off, so rows do not shift as they are toggled.
fn check_mark(on: bool) -> Element {
    let glyph = if on { "\u{2713}" } else { "" };
    ui! { text(style = CheckMark()) { glyph } }
}

/// Props for [`ActiveChip`].
#[derive(Default, IdealystSchema)]
pub struct ActiveChipProps {
    /// Console state handles.
    pub console: Console,
    /// The status id or tag slug.
    pub id: String,
    /// What the chip says.
    pub label: String,
    /// A tag (true) or a state (false).
    pub tag: bool,
}

/// One active filter. Pressing it removes it.
#[component]
pub fn ActiveChip(props: &ActiveChipProps) -> Element {
    let console = props.console;
    let id = props.id.clone();
    let label = format!("{} \u{d7}", props.label);
    let is_tag = props.tag;
    let inner: Element = ui! { Tag(label = label, tone = tone::Primary, variant = variant::Soft) };
    pressable(vec![inner], move || {
        if is_tag {
            console.toggle_pool_tag(&id);
        } else {
            console.toggle_pool_status(&id);
        }
    })
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
    let Some(w) = pool.get(index) else {
        return ui! { view {} };
    };
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
    let tag = pool
        .get(props.want)
        .and_then(|w| w.tags.get(props.index))
        .cloned()
        .unwrap_or_default();
    ui! {
        Tag(label = tag, tone = tone::Neutral, variant = variant::Soft)
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
    pub HeadRight<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexEnd,
            flex_wrap: FlexWrap::Wrap,
            gap: t.spacing.xl(),
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
        base(_t) {
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
        base(_t) {
            width: 260,
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    pub FilterAnchor<IdeaThemeRef> {
        base(_t) {
            position: runtime_core::Position::Relative,
            flex_direction: FlexDirection::Row,
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    pub ChipRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            align_items: AlignItems::Center,
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

// Popovers stay tight (rule 2): the search sits in the menu's own
// gutter, and the rows keep the menu's row rhythm.
stylesheet! {
    pub MenuSearch<IdeaThemeRef> {
        base(t) {
            padding_horizontal: t.spacing.sm(),
            padding_vertical: t.spacing.xs(),
            width: 240,
        }
    }
}

stylesheet! {
    pub MenuRows<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
        }
    }
}

stylesheet! {
    pub MenuBlank<IdeaThemeRef> {
        base(t) {
            padding_horizontal: t.spacing.sm(),
            padding_vertical: t.spacing.xs(),
        }
    }
}

stylesheet! {
    pub CheckMark<IdeaThemeRef> {
        base(t) {
            width: 14,
            font_size: t.typography.body_sm_size(),
            color: t.intent.primary.fg(),
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

#[cfg(test)]
mod tests {
    use super::{CaptureView, WantsView};
    use crate::state::use_console;

    /// Regression: both want screens must MOUNT.
    ///
    /// They are the views that render scene-registry payloads — the
    /// composer's `code_editor` on the capture screen and
    /// `idea_ui::Table` on the pool — and a payload whose handler was
    /// never registered panics at realize with "no handler registered
    /// for item payload". Nothing catches that at compile time: it
    /// type-checks, links, and then aborts on screen.
    ///
    /// So this mounts the real views through the real `realize` against
    /// a mock host, with the app's own `register_scene_extensions` as
    /// the boot seam — the same registry the CLI wrapper installs. Drop
    /// `codeblock::register` and this fails with the browser's panic.
    ///
    /// It does NOT cover the table: see the test below for why.
    #[test]
    fn the_want_screens_mount_with_the_apps_registrations() {
        let harness = host_mock::Harness::with_registry(crate::register_scene_extensions);
        let tree = harness.world.enter(|| {
            // Inside the world: installing the theme injects an ambient,
            // and `use_console` creates signals.
            idea_ui::install_idea_theme(idea_ui::light_theme());
            let console = use_console();
            runtime_core::ui! {
                view {
                    WantsView(console = console)
                    CaptureView(console = console)
                }
            }
        });
        harness.mount(tree);
        harness.flush();
    }

    /// The `table` SDK emits its payloads ONLY on wasm — off-web it
    /// lowers a table to a CSS grid of plain views, which needs no
    /// handler. So the mount test above passes on this host whether or
    /// not `table::register` is in the seam, while the browser panics:
    /// the one payload a host test structurally cannot see is the one
    /// that broke.
    ///
    /// This is the reachable check — that the seam registers exactly the
    /// SDKs the console renders, counted on a registry rather than
    /// through a mount. `table::register` is not cfg'd, so its three
    /// handlers (table, row, cell) are countable here even though a
    /// mounted table never asks for them.
    #[test]
    fn the_seam_registers_every_sdk_the_console_renders() {
        let mut seam = runtime_scene::Registry::<host_mock::HostMock>::new();
        crate::register_scene_extensions(&mut seam);

        let mut expected = runtime_scene::Registry::<host_mock::HostMock>::new();
        codeblock::register(&mut expected);
        table::register(&mut expected);
        markdown::register(&mut expected);

        assert_eq!(
            seam.handler_count(),
            expected.handler_count(),
            "register_scene_extensions must install every SDK payload handler the \
             console renders — a missing line here is a runtime panic on web, not \
             a build error"
        );
    }
}
