//! Every feature the project has, including the completed ones the
//! sidebar rail leaves out.
//!
//! The rail is for steering work in flight, so it drops what is
//! finished; this is where the finished work still lives. Same shape as
//! the want pool — search, status, a paged table — because they answer
//! the same kind of question and one toolbar grammar is the rule
//! (UX_GUIDELINES rule 6).

use std::rc::Rc;

use idea_ui::{size, typography_kind, variant, Button, Field, IdeaThemeRef, Progress,
    SegmentOption, SegmentedControl, Spacer, Table, TableCell, TableRow, Typography};
use runtime_core::{
    component, rx, stylesheet, switch, ui, AlignItems, Element, FlexDirection, FlexWrap,
    FontWeight, IdealystSchema, JustifyContent,
};

use crate::components::bits::{Mono, Pager, StatusBadge};
use crate::model::{completed_count, features, filter_features, Status};
use crate::state::Console;
use crate::styles::{status_tone, SectionLabel};

/// Rows per page (rule 5: data tables page, they do not render
/// unbounded).
const PAGE_SIZE: usize = 20;

/// Props for [`FeaturesView`].
#[derive(Default, IdealystSchema)]
pub struct FeaturesViewProps {
    /// Console state handles.
    pub console: Console,
}

/// The all-features screen: header counts, toolbar, table, pager.
#[component]
pub fn FeaturesView(props: &FeaturesViewProps) -> Element {
    let console = props.console;

    let head = switch(
        move || console.rev.get(),
        move |_rev: &u64| {
            let all = features();
            let total = all.len();
            let done = completed_count();
            let running = all.iter().filter(|f| f.status == Status::Running).count();
            ui! {
                view(style = StatRow()) {
                    HeadStat(value = format!("{total}"), label = "features")
                    HeadStat(value = format!("{running}"), label = "running")
                    HeadStat(value = format!("{done}"), label = "complete")
                }
            }
        },
    );

    let table = switch(
        move || {
            (
                console.rev.get(),
                console.feature_query.get(),
                console.feature_status.get(),
                console.feature_page.get(),
            )
        },
        move |state: &(u64, String, String, usize)| {
            let (_rev, query, status, page) = state.clone();
            let matched = filter_features(&query, &status);
            let total = matched.len();
            let pages = total.div_ceil(PAGE_SIZE).max(1);
            // A filter change resets the page, but a poll can shrink the
            // result set under a page that is already showing.
            let page = page.min(pages - 1);
            let start = page * PAGE_SIZE;
            let visible: Vec<usize> =
                matched.iter().skip(start).take(PAGE_SIZE).copied().collect();
            let shown = visible.len();
            let empty = shown == 0;
            let summary = if total == 0 {
                "No features match".to_string()
            } else {
                format!("{}\u{2013}{} of {total}", start + 1, start + shown)
            };
            ui! {
                view(style = TableCol()) {
                    Table {
                        TableRow {
                            TableCell(header = true, text = Some("Feature".to_string()))
                            TableCell(header = true, text = Some("Status".to_string()))
                            TableCell(header = true, text = Some("Progress".to_string()))
                            TableCell(header = true, text = Some("Planned by".to_string()))
                        }
                        // The blank state goes INSIDE the table shell,
                        // never a lone sentence where the table was.
                        if empty {
                            TableRow {
                                TableCell(text = Some("No features match".to_string()))
                                TableCell(text = Some(String::new()))
                                TableCell(text = Some(String::new()))
                                TableCell(text = Some(String::new()))
                            }
                        }
                        for i in 0..shown {
                            FeatureRow(console = console, feature = visible[i])
                        }
                    }
                    Pager(
                        on_page = Some(Rc::new(move |p| console.set_feature_page(p))
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
        view(style = ScreenBox()) {
            view(style = ScreenHead()) {
                Typography(
                    content = "Features",
                    kind = typography_kind::H2,
                    weight = Some(FontWeight::SemiBold),
                )
                head
            }
            scroll_view(style = ScreenScroll()) {
                view(style = ScreenPad()) {
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

/// Search and status over the feature list.
#[component]
pub fn Toolbar(props: &ToolbarProps) -> Element {
    let console = props.console;
    let on_query: Rc<dyn Fn(String)> = Rc::new(move |t| console.set_feature_query(t));
    let on_status: Rc<dyn Fn(String)> = Rc::new(move |id| console.set_feature_status(id));

    let clear = switch(
        move || (console.feature_query.get(), console.feature_status.get()),
        move |state: &(String, String)| {
            let (query, status) = state.clone();
            if query.is_empty() && status == "all" {
                return ui! { view {} };
            }
            let on_click: Rc<dyn Fn()> = Rc::new(move || console.clear_feature_filters());
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
                    value = console.feature_query,
                    on_change = on_query,
                    placeholder = Some("Search features".to_string()),
                )
            }
            // Open is "not complete" rather than an enumeration of the
            // other states, so a feature is always reachable under
            // exactly one of these however the domain grows.
            SegmentedControl(
                value = rx!(console.feature_status.get()),
                on_change = on_status,
                options = vec![
                    SegmentOption::new("all", "All"),
                    SegmentOption::new("open", "Open"),
                    SegmentOption::new("done", "Complete"),
                ],
            )
            Spacer()
            clear
        }
    }
}

/// Props for [`FeatureRow`].
#[derive(Default, IdealystSchema)]
pub struct FeatureRowProps {
    /// Console state handles.
    pub console: Console,
    /// Index into [`features`].
    pub feature: usize,
}

/// One feature in the list. Clicking it selects the feature and returns
/// to its board — the row is a handle on the feature, and everything
/// else about it lives on the board it opens (rule 20).
#[component]
pub fn FeatureRow(props: &FeatureRowProps) -> Element {
    let console = props.console;
    let index = props.feature;
    let feats = features();
    let f = &feats[index];
    let name = f.name.clone();
    let status = f.status;
    let fraction = f.fraction();
    let pct = f.pct_label();
    let agent = f.agent.clone();
    let (mods_done, mods_total) = f.module_count();
    let modules = format!("{mods_done}/{mods_total} modules");
    let on_row_click: Rc<dyn Fn()> = Rc::new(move || console.select_feature(index));

    ui! {
        TableRow(on_row_click = Some(on_row_click)) {
            TableCell(text = Some(name))
            TableCell {
                StatusBadge(status = status)
            }
            TableCell {
                view(style = ProgressCell()) {
                    Progress(value = fraction, tone = status_tone(status))
                    view(style = ProgressLabels()) {
                        Mono(content = modules)
                        Spacer()
                        Mono(content = pct)
                    }
                }
            }
            TableCell(text = Some(agent))
        }
    }
}

/// Props for [`HeadStat`].
#[derive(Default, IdealystSchema)]
pub struct HeadStatProps {
    /// Stat value.
    pub value: String,
    /// Uppercase label.
    pub label: &'static str,
}

/// One count in the screen header.
#[component]
pub fn HeadStat(props: &HeadStatProps) -> Element {
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

// Padding sits on the scroll view's CONTENT, not on a wrapper around it,
// so the scrollbar hugs the container edge (rule 3).
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
    pub TableCol<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
        }
    }
}

stylesheet! {
    pub ProgressCell<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
            min_width: 120,
        }
    }
}

stylesheet! {
    pub ProgressLabels<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FeaturesView;
    use crate::state::use_console;

    /// Regression: the features screen must MOUNT.
    ///
    /// It renders `idea_ui::Table`, which is not a plain component over
    /// builtins — it emits the `table` SDK's payloads, and a payload
    /// whose handler was never registered panics at realize rather than
    /// failing to compile. See the want pool's own mount test for the
    /// full argument, and for why the seam's handler count is asserted
    /// separately: off-web the table lowers to a grid of views, so this
    /// mount cannot see a missing `table::register` on its own.
    #[test]
    fn the_features_screen_mounts_with_the_apps_registrations() {
        let harness = host_mock::Harness::with_registry(crate::register_scene_extensions);
        let tree = harness.world.enter(|| {
            idea_ui::install_idea_theme(idea_ui::light_theme());
            let console = use_console();
            runtime_core::ui! { FeaturesView(console = console) }
        });
        harness.mount(tree);
        harness.flush();
    }

    /// The empty case is the one a fresh deployment sees first, and the
    /// blank row lives INSIDE the table shell — which means it goes
    /// through the same payload path as a populated one.
    #[test]
    fn the_features_screen_mounts_with_no_features() {
        let harness = host_mock::Harness::with_registry(crate::register_scene_extensions);
        let tree = harness.world.enter(|| {
            idea_ui::install_idea_theme(idea_ui::light_theme());
            let console = use_console();
            console.feature_status.set("done".to_string());
            runtime_core::ui! { FeaturesView(console = console) }
        });
        harness.mount(tree);
        harness.flush();
    }
}
