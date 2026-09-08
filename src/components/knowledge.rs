//! The knowledge base: what this crew knows, in one searchable place.
//!
//! Entries are prose, so they are shown in full on cards rather than
//! truncated into table cells — reading is the point of the screen, and
//! a paragraph clipped to a column is a paragraph nobody reads. That
//! also means there is nothing left for a detail drawer to reveal: the
//! card already carries the entry, its kind, where it is true, and who
//! wrote it.
//!
//! Unlike every other view here, this one does NOT read
//! [`crate::model`]. The base grows without bound and the ranking that
//! makes a loose query useful runs against Postgres' text index, so the
//! screen asks the server per query and renders what comes back.

use std::rc::Rc;

use idea_ui::{size, tone, typography_kind, variant, Button, Field, IdeaThemeRef, SegmentOption,
    SegmentedControl, Spacer, Tag, Typography};
use runtime_core::{
    component, pressable, rx, signal, spawn_then, stylesheet, switch, ui, AlignItems, Element,
    FlexDirection, FlexWrap, FontWeight, IdealystSchema, IntoElement, JustifyContent, Signal,
    StyleApplication,
};

use crate::components::bits::{Mono, Pager};
use crate::state::Console;
use crate::styles::{MonoTextTone, SectionLabel};

/// Entries per page. Matches the server's own page size — the two must
/// agree or the pager counts pages that do not exist.
const PAGE_SIZE: usize = 20;

/// The kinds, in the order the filter rail shows them: the ones a
/// reader most often wants to isolate first.
const KINDS: [(&str, &str); 6] = [
    ("convention", "Conventions"),
    ("decision", "Decisions"),
    ("gotcha", "Gotchas"),
    ("outcome", "Outcomes"),
    ("reference", "References"),
    ("note", "Notes"),
];

/// Props for [`KnowledgeView`].
#[derive(Default, IdealystSchema)]
pub struct KnowledgeViewProps {
    /// Console state handles.
    pub console: Console,
}

/// The knowledge screen: header counts, toolbar, cards, pager.
#[component]
pub fn KnowledgeView(props: &KnowledgeViewProps) -> Element {
    let console = props.console;
    // The last page the server returned, and whether a fetch is in
    // flight. Held here rather than on `Console` because nothing
    // outside this screen reads them.
    let page: Signal<api::KnowledgePage> = signal(api::KnowledgePage::default());
    let loaded = signal(false);

    // One fetch per distinct query. Keyed on `rev` too, so an agent
    // committing a memory refreshes the screen the same way it
    // refreshes every other view.
    let loader = switch(
        move || {
            (
                console.rev.get(),
                console.know_query.get(),
                console.know_kinds.get(),
                console.know_tags.get(),
                console.know_level.get(),
                console.know_page.get(),
                console.know_history.get(),
            )
        },
        move |state: &(u64, String, Vec<String>, Vec<String>, String, usize, bool)| {
            let (_rev, query, kinds, tags, level, at, history) = state.clone();
            spawn_then(
                api::search_knowledge(query, kinds, tags, level, at as i64, history),
                move |result| {
                    // A failed query leaves the previous page on screen
                    // rather than blanking it: the poll's own error
                    // path already reports a dead host, and a blank
                    // list would read as "nothing matches".
                    loaded.set(true);
                    if let Ok(next) = result {
                        page.set(next);
                    } else {
                        runtime_core::log_warn!("knowledge search failed");
                    }
                },
            );
            ui! { view {} }
        },
    );

    let head = switch(
        move || page.get(),
        move |p: &api::KnowledgePage| {
            let total = p.total;
            let counts = p.by_kind.clone();
            // Only the kinds that exist get a stat. A row of zeroes
            // teaches the reader nothing and pushes the content down.
            let shown: Vec<(String, i64)> = counts
                .into_iter()
                .filter(|c| c.count > 0)
                .map(|c| (label_for(&c.kind).to_string(), c.count))
                .collect();
            let n = shown.len();
            ui! {
                view(style = StatRow()) {
                    HeadStat(value = format!("{total}"), label = "entries")
                    for i in 0..n {
                        HeadStat(value = format!("{}", shown[i].1), label = shown[i].0.clone())
                    }
                }
            }
        },
    );

    let body = switch(
        move || (page.get(), loaded.get(), console.know_page.get()),
        move |state: &(api::KnowledgePage, bool, usize)| {
            let (p, has_loaded, at) = state.clone();
            let count = p.entries.len();
            let total = p.total.max(0) as usize;
            let pages = total.div_ceil(PAGE_SIZE).max(1);
            let at = at.min(pages - 1);
            let start = at * PAGE_SIZE;
            let summary = if total == 0 {
                String::new()
            } else {
                format!("{}\u{2013}{} of {total}", start + 1, start + count)
            };
            // "Nothing here" and "not asked yet" are different states,
            // and only one of them means the reader should change their
            // query.
            let empty_line = if !has_loaded {
                "Searching\u{2026}"
            } else {
                "No entries match"
            };
            ui! {
                view(style = ListCol()) {
                    if count == 0 {
                        view(style = EmptyCard()) {
                            Typography(
                                content = empty_line,
                                kind = typography_kind::BodySm,
                                muted = true,
                            )
                        }
                    }
                    for i in 0..count {
                        EntryCard(console = console, entry = p.entries[i].clone())
                    }
                    if total > 0 {
                        Pager(
                            on_page = Some(Rc::new(move |to| console.set_know_page(to))
                                as Rc<dyn Fn(usize)>),
                            summary = summary.clone(),
                            page = at,
                            pages = pages,
                        )
                    }
                }
            }
        },
    );

    ui! {
        view(style = ScreenBox()) {
            view(style = ScreenHead()) {
                Typography(
                    content = "Knowledge",
                    kind = typography_kind::H2,
                    weight = Some(FontWeight::SemiBold),
                )
                head
            }
            scroll_view(style = ScreenScroll()) {
                view(style = ScreenPad()) {
                    Toolbar(console = console)
                    body
                }
            }
            loader
        }
    }
}

/// Props for [`Toolbar`].
#[derive(Default, IdealystSchema)]
pub struct ToolbarProps {
    /// Console state handles.
    pub console: Console,
}

/// Search, scope, kind chips and tag chips — the same grammar the pool
/// and the features screen use.
#[component]
pub fn Toolbar(props: &ToolbarProps) -> Element {
    let console = props.console;
    let on_query: Rc<dyn Fn(String)> = Rc::new(move |t| console.set_know_query(t));
    let on_level: Rc<dyn Fn(String)> = Rc::new(move |id| {
        // The control needs a non-empty id per option; "all" is the
        // screen's word for "no level filter" and the server's is an
        // empty string.
        console.set_know_level(if id == "all" { String::new() } else { id });
    });

    let kind_row = switch(
        move || console.know_kinds.get(),
        move |active: &Vec<String>| {
            let active = active.clone();
            ui! {
                view(style = FilterRow()) {
                    text(style = RowLabel()) { "Kind" }
                    view(style = ChipRow()) {
                    for i in 0..KINDS.len() {
                        KindChip(
                            console = console,
                            kind = KINDS[i].0,
                            label = KINDS[i].1,
                            on = active.iter().any(|k| k == KINDS[i].0),
                        )
                    }
                    }
                }
            }
        },
    );

    let tag_row = switch(
        move || (console.rev.get(), console.know_tags.get()),
        move |state: &(u64, Vec<String>)| {
            let (_rev, active) = state.clone();
            let count = crate::model::tags().len();
            if count == 0 {
                return ui! { view {} };
            }
            ui! {
                view(style = FilterRow()) {
                    text(style = RowLabel()) { "Tags" }
                    view(style = ChipRow()) {
                        for i in 0..count {
                            KnowTag(console = console, index = i, active = active.clone())
                        }
                    }
                }
            }
        },
    );

    // Nothing is ever deleted, so the history is always one toggle
    // away rather than something a reader has to know a tool call for.
    let history = switch(
        move || console.know_history.get(),
        move |on: &bool| {
            let on = *on;
            let toggle: Rc<dyn Fn()> = Rc::new(move || console.set_know_history(!on));
            let label = if on { "Hide superseded" } else { "Show superseded" };
            if on {
                ui! {
                    Button(label = label, on_click = toggle, size = size::Sm,
                           variant = variant::Soft)
                }
            } else {
                ui! {
                    Button(label = label, on_click = toggle, size = size::Sm,
                           variant = variant::Ghost)
                }
            }
        },
    );

    let clear = switch(
        move || {
            (
                console.know_query.get(),
                console.know_kinds.get(),
                console.know_tags.get(),
                console.know_level.get(),
            )
        },
        move |state: &(String, Vec<String>, Vec<String>, String)| {
            let (query, kinds, tags, level) = state.clone();
            if query.is_empty() && kinds.is_empty() && tags.is_empty() && level.is_empty() {
                return ui! { view {} };
            }
            let on_click: Rc<dyn Fn()> = Rc::new(move || console.clear_know_filters());
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
        view(style = ToolbarBox()) {
            view(style = ToolbarRow()) {
                view(style = SearchSlot()) {
                    Field(
                        value = console.know_query,
                        on_change = on_query,
                        placeholder = Some("Ask in your own words".to_string()),
                    )
                }
                SegmentedControl(
                    value = rx!(match console.know_level.get().as_str() {
                        "" => "all".to_string(),
                        other => other.to_string(),
                    }),
                    on_change = on_level,
                    options = vec![
                        SegmentOption::new("all", "Everywhere"),
                        SegmentOption::new("project", "Project"),
                        SegmentOption::new("feature", "Features"),
                        SegmentOption::new("module", "Modules"),
                    ],
                )
                Spacer()
                history
                clear
            }
            kind_row
            tag_row
        }
    }
}

/// Props for [`KindChip`].
#[derive(Default, IdealystSchema)]
pub struct KindChipProps {
    /// Console state handles.
    pub console: Console,
    /// The kind's wire value.
    pub kind: &'static str,
    /// How it reads on screen.
    pub label: &'static str,
    /// Whether it is currently narrowing the list.
    pub on: bool,
}

/// One kind in the filter rail. Solid means it is narrowing.
#[component]
pub fn KindChip(props: &KindChipProps) -> Element {
    let console = props.console;
    let kind = props.kind;
    let label = props.label;
    let inner: Element = if props.on {
        ui! { Tag(label = label, tone = kind_tone(kind), variant = variant::Solid) }
    } else {
        ui! { Tag(label = label, tone = tone::Neutral, variant = variant::Soft) }
    };
    pressable(vec![inner], move || console.toggle_know_kind(kind))
        .with_style(StyleApplication::new(chip_press_style()))
        .into_element()
}

/// Props for [`KnowTag`].
#[derive(Default, IdealystSchema)]
pub struct KnowTagProps {
    /// Console state handles.
    pub console: Console,
    /// Index into [`crate::model::tags`].
    pub index: usize,
    /// The tags currently filtered on.
    pub active: Vec<String>,
}

/// One tag in the filter rail, from the shared registry — the pool and
/// the knowledge base file under the same vocabulary.
#[component]
pub fn KnowTag(props: &KnowTagProps) -> Element {
    let console = props.console;
    let tags = crate::model::tags();
    let tag = &tags[props.index];
    let name = tag.name.clone();
    let label = tag.label.clone();
    let on = props.active.iter().any(|t| *t == name);
    let inner: Element = if on {
        ui! { Tag(label = label, tone = tone::Primary, variant = variant::Solid) }
    } else {
        ui! { Tag(label = label, tone = tone::Neutral, variant = variant::Soft) }
    };
    let pick = name.clone();
    pressable(vec![inner], move || console.toggle_know_tag(&pick))
        .with_style(StyleApplication::new(chip_press_style()))
        .into_element()
}

/// Props for [`EntryCard`].
#[derive(Default, IdealystSchema)]
pub struct EntryCardProps {
    /// Console state handles.
    pub console: Console,
    /// The entry to render.
    pub entry: api::MemoryDto,
}

/// One entry, whole. The content is the item; the kind and the scope
/// are what you filter by; the author and the date are the two facts
/// that make it possible to weigh — everything a reader needs is here,
/// which is why this card has nothing behind it.
#[component]
pub fn EntryCard(props: &EntryCardProps) -> Element {
    let console = props.console;
    let e = &props.entry;
    let open_id = e.id.clone();
    let content = e.content.clone();
    let kind = e.kind.clone();
    let kind_label = label_for(&kind).to_string();
    let tone = kind_tone(&kind);
    let author = e.author.clone();
    let written = e.written.clone();
    let tag_count = e.tags.len();
    let tags = e.tags.clone();
    // Project scope reads as the project's own name, which is more
    // useful than the word "project" repeated down the column.
    let where_ = if e.level == "project" {
        e.subject_name.clone()
    } else {
        format!("{} \u{b7} {}", e.level, e.subject_name)
    };

    // A superseded or disputed entry must not read like current
    // knowledge. It is dimmed and says which it is — it is only on
    // screen because the reader asked to see history.
    let state = e.state.clone();
    let withdrawn = state != "current";
    let arm = match state.as_str() {
        "superseded" => "superseded",
        "disputed" => "disputed",
        _ => "current",
    };

    // Standing, decomposed. Never a single score: "2 confirms" and
    // "1 dispute" are facts a reader can weigh, where "0.31" is a
    // number they can only take on faith.
    let mut marks: Vec<String> = Vec::new();
    if e.confirms > 0 {
        marks.push(format!("{} confirmed", e.confirms));
    }
    if e.disputes > 0 {
        marks.push(format!("{} disputed", e.disputes));
    }
    if e.touches > 0 {
        marks.push(format!("{} used", e.touches));
    }
    let standing = marks.join(" \u{b7} ");
    let has_standing = !standing.is_empty();

    let inner: Element = ui! {
        view(style = Card().state(match arm {
            "superseded" => CardState::Superseded,
            "disputed" => CardState::Disputed,
            _ => CardState::Current,
        })) {
            view(style = CardHead()) {
                Tag(label = kind_label, tone = tone, variant = variant::Soft)
                if withdrawn {
                    Tag(
                        label = state.clone(),
                        tone = state_tone(arm),
                        variant = variant::Outlined,
                    )
                }
                view(style = WhereSlot()) {
                    Mono(content = where_)
                }
                Spacer()
            }
            Typography(content = content, kind = typography_kind::Body)
            view(style = CardFoot()) {
                if tag_count > 0 {
                    view(style = ChipRow()) {
                        for i in 0..tag_count {
                            Tag(label = tags[i].clone(), tone = tone::Neutral, variant = variant::Soft)
                        }
                    }
                }
                Spacer()
                view(style = ByLine()) {
                    if has_standing {
                        Mono(content = standing.clone(), tone = MonoTextTone::Text)
                    }
                    Mono(content = author)
                    Mono(content = written)
                }
            }
        }
    };
    // The card carries the entry; the drawer carries its place in the
    // graph. Clicking is how you get from one to the other (rule 15:
    // one action belongs on the row, not behind a menu).
    pressable(vec![inner], move || console.open_knowledge(Some(open_id.clone())))
        .with_style(StyleApplication::new(card_press_style()))
        .into_element()
}

runtime_core::stylesheet! {
    pub CardPress<IdeaThemeRef> {
        base(t) {
            border_radius: t.radius.md(),
            cursor: runtime_core::Cursor::Pointer,
            min_width: 0,
        }
    }
}

/// Props for [`HeadStat`].
#[derive(Default, IdealystSchema)]
pub struct HeadStatProps {
    /// Stat value.
    pub value: String,
    /// Uppercase label.
    pub label: String,
}

/// One count in the screen header.
#[component]
pub fn HeadStat(props: &HeadStatProps) -> Element {
    let value = props.value.clone();
    let label = props.label.clone();
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

/// A kind's plural, for chips and stats.
fn label_for(kind: &str) -> &'static str {
    KINDS
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, label)| *label)
        .unwrap_or("Entries")
}

/// A withdrawn entry's tone: a dispute is a live problem, a
/// supersession is settled history.
fn state_tone(state: &str) -> idea_ui::ToneRef {
    match state {
        "disputed" => tone::Danger.into(),
        _ => tone::Neutral.into(),
    }
}

/// The tone a kind carries. A gotcha is a warning, an outcome is
/// settled, a convention binds — the colors say which without a
/// sentence saying it (rule 16).
fn kind_tone(kind: &str) -> idea_ui::ToneRef {
    match kind {
        "convention" => tone::Primary.into(),
        "decision" => tone::Info.into(),
        "gotcha" => tone::Warning.into(),
        "outcome" => tone::Success.into(),
        "reference" => tone::Secondary.into(),
        _ => tone::Neutral.into(),
    }
}

stylesheet! {
    pub ScreenBox<IdeaThemeRef> {
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
    pub ScreenHead<IdeaThemeRef> {
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
    pub ScreenScroll<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            min_height: 0,
        }
    }
}

// Padding on the scroll view's CONTENT, so the scrollbar hugs the
// container edge (rule 3).
stylesheet! {
    pub ScreenPad<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xl(),
            padding: t.spacing.xl(),
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
            width: 280,
            flex_shrink: 0.0,
        }
    }
}

// A labelled filter axis. The label is fixed-width so the two rows'
// chips line up with each other rather than stepping in and out as the
// words change length (rule 18).
stylesheet! {
    pub FilterRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
            min_width: 0,
        }
    }
}

stylesheet! {
    pub RowLabel<IdeaThemeRef> {
        base(t) {
            width: 38,
            flex_shrink: 0.0,
            font_size: t.typography.overline_size(),
            font_weight: FontWeight::SemiBold,
            text_transform: runtime_core::TextTransform::Uppercase,
            letter_spacing: 1.0,
            color: t.color.text_muted(),
        }
    }
}

stylesheet! {
    pub ChipRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            gap: t.spacing.xs(),
            min_width: 0,
            flex_shrink: 1.0,
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
    pub ListCol<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
        }
    }
}

stylesheet! {
    pub Card<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
            padding: t.spacing.lg(),
            border_width: 1.0,
            border_radius: t.radius.md(),
            min_width: 0,
        }
        variant state {
            #[default]
            current(t) {
                border_color: t.color.border(),
                background: t.color.surface(),
            }
            // Recessed rather than merely tagged: a withdrawn entry
            // sitting at full contrast among current ones still reads
            // as current at a glance, which is the whole risk of
            // showing history inline.
            superseded(t) {
                border_color: t.color.border(),
                background: t.color.background(),
                opacity: 0.72,
            }
            disputed(t) {
                border_color: t.intent.danger.border(),
                background: t.color.background(),
                opacity: 0.72,
            }
        }
    }
}

stylesheet! {
    pub EmptyCard<IdeaThemeRef> {
        base(t) {
            padding_vertical: t.spacing.xl(),
            padding_horizontal: t.spacing.lg(),
            align_items: AlignItems::Center,
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.md(),
            background: t.color.surface(),
        }
    }
}

stylesheet! {
    pub CardHead<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
            min_width: 0,
        }
    }
}

// The scope line shrinks; the kind badge beside it does not (rule 22).
stylesheet! {
    pub WhereSlot<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            min_width: 0,
            flex_shrink: 1.0,
        }
    }
}

stylesheet! {
    pub CardFoot<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            flex_wrap: FlexWrap::Wrap,
            gap: t.spacing.sm(),
            min_width: 0,
        }
    }
}

stylesheet! {
    pub ByLine<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.md(),
            flex_shrink: 0.0,
        }
    }
}

/// Props for [`KnowledgeDrawer`].
#[derive(Default, IdealystSchema)]
pub struct KnowledgeDrawerProps {
    /// Console state handles.
    pub console: Console,
    /// The entry to describe.
    pub memory_id: String,
}

/// One entry's place in the graph: what it replaced, what replaced it,
/// what it stands beside, and what the co-use record thinks it might
/// belong with.
///
/// This is where a drawer finally earns its place (rule 9). The card
/// carries the entry itself; everything here is about its RELATIONS,
/// which are properties of the links rather than of the entry — most of
/// all the rationale, which is usually the only record of why a rule
/// changed and lives on no memory at all.
#[component]
pub fn KnowledgeDrawer(props: &KnowledgeDrawerProps) -> Element {
    let console = props.console;
    let id = props.memory_id.clone();
    let detail: Signal<api::MemoryDetail> = signal(api::MemoryDetail::default());
    let loaded = signal(false);

    let loader = switch(
        move || (id.clone(), console.rev.get()),
        move |state: &(String, u64)| {
            let (id, _rev) = state.clone();
            spawn_then(api::knowledge_detail(id), move |result| {
                loaded.set(true);
                if let Ok(next) = result {
                    detail.set(next);
                } else {
                    runtime_core::log_warn!("knowledge detail failed");
                }
            });
            ui! { view {} }
        },
    );

    let body = switch(
        move || (detail.get(), loaded.get()),
        move |state: &(api::MemoryDetail, bool)| {
            let (d, has_loaded) = state.clone();
            if !has_loaded {
                return ui! {
                    view(style = Section()) {
                        Typography(content = "Loading\u{2026}", kind = typography_kind::BodySm,
                                   muted = true)
                    }
                };
            }
            let e = d.entry.clone();
            // Nothing linked in either direction is a real answer, not
            // an empty state to apologise for.
            let isolated = d.supersedes.is_empty()
                && d.superseded_by.is_empty()
                && d.relations.is_empty()
                && d.suggestions.is_empty();
            // The `ui!` body is an `Fn`, so everything it reads has to
            // be owned up front rather than moved out of `d`.
            let (supersedes, superseded_by) = (d.supersedes.clone(), d.superseded_by.clone());
            let relations = d.relations.clone();
            let suggestions = d.suggestions.clone();
            let has_suggestions = !suggestions.is_empty();
            let n_suggestions = suggestions.len();
            ui! {
                view(style = DrawerCol()) {
                    view(style = Section()) {
                        text(style = SectionLabel()) { "Entry" }
                        Typography(content = e.content.clone(), kind = typography_kind::Body)
                        view(style = MetaRow()) {
                            Mono(content = e.id.clone())
                            Spacer()
                            Mono(content = e.author.clone())
                            Mono(content = e.written.clone())
                        }
                    }
                    LinkGroup(
                        label = "Replaced by",
                        // Newest first: what is true NOW is the thing a
                        // reader opening a superseded entry came for.
                        links = superseded_by.clone(),
                        console = console,
                    )
                    LinkGroup(label = "Replaces", links = supersedes.clone(), console = console)
                    LinkGroup(label = "Related", links = relations.clone(), console = console)
                    if has_suggestions {
                        view(style = Section()) {
                            text(style = SectionLabel()) { "Often used together" }
                            // Named as a suggestion, because that is
                            // what it is: nobody has said these are
                            // connected, agents have merely reached for
                            // them in the same breath.
                            for i in 0..n_suggestions {
                                SuggestionRow(
                                    console = console,
                                    entry = suggestions[i].clone(),
                                )
                            }
                        }
                    }
                    if isolated {
                        view(style = Section()) {
                            Typography(
                                content = "Nothing links to this yet.",
                                kind = typography_kind::BodySm,
                                muted = true,
                            )
                        }
                    }
                }
            }
        },
    );

    let close: Rc<dyn Fn()> = Rc::new(move || console.open_knowledge(None));
    let backdrop = pressable(vec![ui! { view(style = Backdrop()) {} }], {
        let console = console;
        move || console.open_knowledge(None)
    })
    .with_style(StyleApplication::new(backdrop_style()))
    .into_element();

    ui! {
        view(style = DrawerHost()) {
            backdrop
            view(style = Panel()) {
                view(style = PanelHead()) {
                    Typography(
                        content = "In the graph",
                        kind = typography_kind::H3,
                        weight = Some(FontWeight::SemiBold),
                    )
                    Spacer()
                    Button(label = "Close", on_click = close, size = size::Sm,
                           variant = variant::Ghost)
                }
                scroll_view(style = PanelScroll()) { body }
            }
            loader
        }
    }
}

/// Props for [`LinkGroup`].
#[derive(Default, IdealystSchema)]
pub struct LinkGroupProps {
    /// Console state handles.
    pub console: Console,
    /// Heading.
    pub label: &'static str,
    /// The links to show. An empty group renders nothing at all.
    pub links: Vec<api::LinkDto>,
}

/// One labelled group of links. Absent rather than empty when there are
/// none: a drawer of "no supersessions / no relations / no suggestions"
/// is three lines of nothing pushing the content down.
#[component]
pub fn LinkGroup(props: &LinkGroupProps) -> Element {
    let console = props.console;
    let links = props.links.clone();
    let label = props.label;
    if links.is_empty() {
        return ui! { view {} };
    }
    ui! {
        view(style = Section()) {
            text(style = SectionLabel()) { label }
            for i in 0..links.len() {
                LinkRow(console = console, link = links[i].clone())
            }
        }
    }
}

/// Props for [`LinkRow`].
#[derive(Default, IdealystSchema)]
pub struct LinkRowProps {
    /// Console state handles.
    pub console: Console,
    /// The link.
    pub link: api::LinkDto,
}

/// One link: how it relates, to what, and why. Clicking walks the graph
/// — the drawer re-targets rather than stacking, so a reader can follow
/// a chain without accumulating panels.
#[component]
pub fn LinkRow(props: &LinkRowProps) -> Element {
    let console = props.console;
    let link = props.link.clone();
    let target = link.other.id.clone();
    let kind = link.kind.clone();
    // `refines` outward and inward mean opposite things, so the label
    // says which way it is being read.
    let verb = if link.outgoing { kind.replace('_', " ") } else { format!("{} \u{2190}", kind.replace('_', " ")) };
    let rationale = link.rationale.clone();
    let has_why = !rationale.trim().is_empty();
    let content = link.other.content.clone();
    let state = link.other.state.clone();
    let withdrawn = state != "current";

    let inner: Element = ui! {
        view(style = LinkBox()) {
            view(style = LinkHead()) {
                Tag(label = verb, tone = tone::Neutral, variant = variant::Soft)
                if withdrawn {
                    Tag(label = state.clone(), tone = state_tone(&state), variant = variant::Outlined)
                }
                Spacer()
            }
            Typography(content = content, kind = typography_kind::BodySm)
            if has_why {
                Mono(content = rationale, tone = MonoTextTone::Muted)
            }
        }
    };
    pressable(vec![inner], move || console.open_knowledge(Some(target.clone())))
        .with_style(StyleApplication::new(link_press_style()))
        .into_element()
}

/// Props for [`SuggestionRow`].
#[derive(Default, IdealystSchema)]
pub struct SuggestionRowProps {
    /// Console state handles.
    pub console: Console,
    /// The suggested entry.
    pub entry: api::MemoryDto,
}

/// A co-use suggestion. Deliberately plainer than a declared link: it is
/// a question the graph is asking, not something it knows.
#[component]
pub fn SuggestionRow(props: &SuggestionRowProps) -> Element {
    let console = props.console;
    let entry = props.entry.clone();
    let target = entry.id.clone();
    let content = entry.content.clone();
    let inner: Element = ui! {
        view(style = LinkBox()) {
            Typography(content = content, kind = typography_kind::BodySm, muted = true)
        }
    };
    pressable(vec![inner], move || console.open_knowledge(Some(target.clone())))
        .with_style(StyleApplication::new(link_press_style()))
        .into_element()
}

stylesheet! {
    pub DrawerHost<IdeaThemeRef> {
        base(t) {
            position: runtime_core::Position::Absolute,
            top: 0, left: 0, right: 0, bottom: 0,
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::FlexEnd,
        }
    }
}

stylesheet! {
    pub Backdrop<IdeaThemeRef> {
        base(t) {
            position: runtime_core::Position::Absolute,
            top: 0, left: 0, right: 0, bottom: 0,
            background: t.color.overlay(),
        }
    }
}

stylesheet! {
    pub Panel<IdeaThemeRef> {
        base(t) {
            width: 420,
            max_width: runtime_core::Length::Percent(100.0),
            flex_direction: FlexDirection::Column,
            background: t.color.surface(),
            border_left_width: 1.0,
            border_color: t.color.border(),
            min_height: 0,
        }
    }
}

stylesheet! {
    pub PanelHead<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
            padding_vertical: t.spacing.md(),
            padding_horizontal: t.spacing.lg(),
            border_bottom_width: 1.0,
            border_color: t.color.border(),
        }
    }
}

stylesheet! {
    pub PanelScroll<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            min_height: 0,
        }
    }
}

stylesheet! {
    pub DrawerCol<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xl(),
            padding: t.spacing.lg(),
        }
    }
}

stylesheet! {
    pub Section<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
            min_width: 0,
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
    pub LinkBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
            padding: t.spacing.md(),
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.sm(),
            background: t.color.background(),
            min_width: 0,
        }
    }
}

stylesheet! {
    pub LinkHead<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.xs(),
            min_width: 0,
        }
    }
}

stylesheet! {
    pub LinkPress<IdeaThemeRef> {
        base(t) {
            border_radius: t.radius.sm(),
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

#[cfg(test)]
mod tests {
    use super::{KnowledgeDrawer, KnowledgeView};
    use crate::state::use_console;

    /// The screen must MOUNT before any data arrives — its first frame
    /// is always the empty one, because the fetch is in flight.
    ///
    /// It renders no SDK payload of its own (cards are plain views, and
    /// the entries are prose rather than a `Table`), but the `Field` and
    /// `SegmentedControl` in its toolbar go through the same realize
    /// path, and a props/vocabulary mistake there is a panic on screen
    /// rather than a build error.
    #[test]
    fn the_knowledge_screen_mounts_before_its_first_result() {
        let harness = host_mock::Harness::with_registry(crate::register_scene_extensions);
        let tree = harness.world.enter(|| {
            idea_ui::install_idea_theme(idea_ui::light_theme());
            let console = use_console();
            runtime_core::ui! { KnowledgeView(console = console) }
        });
        harness.mount(tree);
        harness.flush();
    }

    /// The drawer must mount before its fetch lands — its first frame
    /// is always the empty one.
    #[test]
    fn the_knowledge_drawer_mounts_before_its_first_result() {
        let harness = host_mock::Harness::with_registry(crate::register_scene_extensions);
        let tree = harness.world.enter(|| {
            idea_ui::install_idea_theme(idea_ui::light_theme());
            let console = use_console();
            runtime_core::ui! {
                KnowledgeDrawer(console = console, memory_id = "mem_abc".to_string())
            }
        });
        harness.mount(tree);
        harness.flush();
    }

    /// Every filter arm has to survive a mount too: the chips and the
    /// scope control render differently when they are narrowing, and
    /// that state is reachable from a link the reader clicks once.
    #[test]
    fn the_knowledge_screen_mounts_with_every_filter_engaged() {
        let harness = host_mock::Harness::with_registry(crate::register_scene_extensions);
        let tree = harness.world.enter(|| {
            idea_ui::install_idea_theme(idea_ui::light_theme());
            let console = use_console();
            console.set_know_query("migrations".to_string());
            console.toggle_know_kind("convention");
            console.toggle_know_tag("database");
            console.set_know_level("project".to_string());
            runtime_core::ui! { KnowledgeView(console = console) }
        });
        harness.mount(tree);
        harness.flush();
    }
}
