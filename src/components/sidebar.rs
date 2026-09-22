//! Left nav: the four project-wide destinations, then the features
//! still in play.
//!
//! The rail is for steering work in flight, so a completed feature
//! drops out of it — on a long-lived project the finished ones are the
//! majority, and a list you have to scroll past to reach the thing you
//! are working on has stopped being a rail. They are all still one
//! click away under Features, which says how many it is holding.
//!
//! The rail does not collapse itself. `AppShell` owns that axis: at
//! desktop widths the nav is pinned in flow, and below the pin
//! breakpoint the same panel slides in over a scrim. One mechanism,
//! one meaning — "is the nav showing" — rather than a hand-rolled
//! width toggle competing with the shell for the same edge.
//!
//! What that costs the rail is one duty: below the breakpoint the nav
//! is covering the work, so picking anything here has to put it away
//! again. Above it, dismissing would hide a pinned column the reader
//! never opened, so `sidebar_pinned` gates it.

use idea_ui::{typography_kind, Badge, IdeaThemeRef, Progress, ProgressCap, Spacer, Typography};
use idea_ui_nav::sidebar_pinned;
use runtime_core::{
    component, memo, rx, stylesheet, switch, ui, AlignItems, Breakpoint, Element, FlexDirection,
    FontWeight, IdealystSchema, IntoElement, StyleApplication,
};

use crate::components::bits::{Mono, StatusDot, tappable};
use crate::model::{in_play_of, rail_features_of, Status};
use crate::state::Console;
use crate::styles::{status_tone, MonoTextSize, SectionLabel};

/// The width at and above which the nav is pinned rather than a
/// drawer. Must match the `pin_at` `app()` hands `AppShell`.
pub const PIN_AT: Breakpoint = Breakpoint::Lg;

/// Put the nav away after a pick — but only when it is covering
/// something. A pinned column has nothing to get out of the way of, and
/// hiding it would leave the reader on a screen whose nav just
/// disappeared for no reason they can see.
fn dismiss_if_drawer(console: Console) {
    if !sidebar_pinned(PIN_AT) {
        console.nav_open.set(false);
    }
}

/// Props for [`Sidebar`].
#[derive(Default, IdealystSchema)]
pub struct SidebarProps {
    /// Console state handles.
    pub console: Console,
}

/// The left nav.
#[component]
pub fn Sidebar(props: &SidebarProps) -> Element {
    let console = props.console;

    let data = console.data;
    // The destinations. Rebuilt on pane (for the selected arm) and on
    // the two counts it shows — never on `nav_open`, which is carried
    // by the reactive width below so the collapse can animate, and
    // never on a poll that changed neither.
    let nav = switch(
        move || {
            (
                console.pane.get(),
                data.features.get().len(),
                data.want_counts.get().0,
                // The badge counts items waiting on a person to say
                // they shipped — the one state here with a verb
                // attached, and the reason to look at the screen.
                data.roadmap.get().ready_count(),
            )
        },
        move |(pane, total, loose, ready): &(String, usize, usize, usize)| {
            let pane = pane.clone();
            let (total, loose, ready) = (*total, *loose, *ready);
            let on_feature = pane == "feature" || pane == "features";
            ui! {
                view(style = NavGroup()) {
                    NavItem(
                        console = console,
                        id = "overview",
                        glyph = "\u{25c7}",
                        label = "Overview",
                        selected = pane == "overview",
                    )
                    NavItem(
                        console = console,
                        id = "features",
                        glyph = "\u{25a4}",
                        label = "Features",
                        count = total,
                        selected = on_feature,
                    )
                    NavItem(
                        console = console,
                        id = "roadmap",
                        glyph = "\u{25b3}",
                        label = "Roadmap",
                        count = ready,
                        urgent = true,
                        selected = pane == "roadmap",
                    )
                    NavItem(
                        console = console,
                        id = "wants",
                        glyph = "\u{25cb}",
                        label = "Want pool",
                        count = loose,
                        urgent = true,
                        selected = pane == "wants",
                    )
                    NavItem(
                        console = console,
                        id = "capture",
                        glyph = "\u{270e}",
                        label = "Capture",
                        selected = pane == "capture",
                    )
                    NavItem(
                        console = console,
                        id = "knowledge",
                        glyph = "\u{25c8}",
                        label = "Knowledge",
                        selected = pane == "knowledge",
                    )
                }
            }
        },
    );

    // The rail's MEMBERSHIP is the key: which features it lists, in
    // what order, and which is selected. A poll that moves a count or
    // a spinner changes none of that, so the rows stay mounted and
    // read their own feature through the data signals. A feature
    // finishing (it leaves the rail) or being planned (it joins) is
    // what rebuilds the list.
    let rail = switch(
        move || {
            let sel = console.feature.get();
            let visible = rail_features_of(&data.features.get(), sel);
            (console.pane.get(), sel, visible)
        },
        move |(pane, sel, visible): &(String, usize, Vec<usize>)| {
            let (pane, sel, visible) = (pane.clone(), *sel, visible.clone());
            let count = visible.len();
            let on_feature = pane == "feature";
            let scope = rx!({
                let feats = data.features.get();
                format!("{} of {}", in_play_of(&feats).len(), feats.len())
            });
            ui! {
                view(style = RailBox()) {
                    view(style = RailHead()) {
                        text(style = SectionLabel()) { "In play" }
                        Spacer()
                        Mono(content = scope, size = MonoTextSize::Overline)
                    }
                    scroll_view(style = RailScroll()) {
                        view(style = RailList()) {
                            for i in 0..count {
                                RailFeature(
                                    console = console,
                                    index = visible[i],
                                    selected = on_feature && visible[i] == sel,
                                )
                            }
                            AllFeaturesLink(console = console)
                        }
                    }
                }
            }
        },
    );

    ui! {
        view(style = NavBox()) {
            nav
            rail
        }
    }
}

/// Props for [`NavItem`].
#[derive(Default, IdealystSchema)]
pub struct NavItemProps {
    /// Console state handles.
    pub console: Console,
    /// Pane id this entry selects.
    pub id: &'static str,
    /// Single-character mark in the glyph column.
    pub glyph: &'static str,
    /// What the entry is called.
    pub label: &'static str,
    /// Trailing count. Zero renders no badge.
    pub count: usize,
    /// Whether the count is something to drain (the loose-want pile)
    /// rather than a plain total.
    pub urgent: bool,
    /// Whether this destination is the one showing.
    pub selected: bool,
}

/// One destination in the nav.
#[component]
pub fn NavItem(props: &NavItemProps) -> Element {
    let console = props.console;
    let id = props.id;
    let glyph = props.glyph;
    let label = props.label;
    let count = props.count;
    let count_label = format!("{count}");
    let tone: idea_ui::ToneRef = if props.urgent {
        idea_ui::tone::Warning.into()
    } else {
        idea_ui::tone::Neutral.into()
    };
    let arm = if props.selected { "on" } else { "off" };

    let inner: Element = ui! {
        view(style = NavRow()) {
            text(style = NavGlyph()) { glyph }
            view(style = NavLabelSlot()) {
                Typography(
                    content = label,
                    kind = typography_kind::BodySm,
                    weight = Some(if props.selected {
                        FontWeight::SemiBold
                    } else {
                        FontWeight::Medium
                    }),
                )
            }
            if count > 0 {
                view(style = NavBadgeSlot()) {
                    Badge(label = count_label, tone = tone)
                }
            }
        }
    };

    tappable(vec![inner], move || {
        match id {
            "overview" => console.show_overview(),
            "features" => console.show_features(),
            "roadmap" => console.show_roadmap(),
            "wants" => console.show_wants(),
            "capture" => console.show_capture(),
            _ => console.show_knowledge(),
        }
        dismiss_if_drawer(console);
    })
    .with_style(StyleApplication::new(nav_entry_style()).with("selected", arm.to_string()))
    .into_element()
}

/// Props for [`RailFeature`].
#[derive(Default, IdealystSchema)]
pub struct RailFeatureProps {
    /// Console state handles.
    pub console: Console,
    /// Index into [`features`].
    pub index: usize,
    /// Whether this is the selected feature.
    pub selected: bool,
}

/// One feature in the in-play rail: what it is, and how far along.
///
/// Two lines, not the three a card took — the rail's job is to be
/// scanned, and at three lines each four features filled it and the
/// rest became a scroll.
///
/// Built once per membership of the rail and never for a poll: every
/// value on it is read live off the feature's signal, so the name,
/// the percentage, the bar and the ring move in place — and a ring
/// that is turning keeps turning.
#[component]
pub fn RailFeature(props: &RailFeatureProps) -> Element {
    let console = props.console;
    let data = console.data;
    let index = props.index;
    let f = data.feature(index);
    let status = memo(move || f().map(|f| f.status).unwrap_or_default());
    let live = memo(move || {
        let now = data.clock.get();
        f().is_some_and(|f| f.live_at(now))
    });
    let name = rx!(f().map(|f| f.name.clone()).unwrap_or_default());
    let pct = rx!(f().map(|f| f.pct_label()).unwrap_or_default());
    let fraction = rx!(f().map(|f| f.fraction()).unwrap_or(0.0));
    let tone = rx!(status_tone(status.get()));
    let arm = if props.selected { "on" } else { "off" };

    let inner: Element = ui! {
        view(style = RailInner()) {
            view(style = RailTitleRow()) {
                StatusDot(status = status, live = live)
                view(style = RailNameSlot()) {
                    Typography(
                        content = name,
                        kind = typography_kind::Caption,
                        weight = Some(if props.selected {
                            FontWeight::SemiBold
                        } else {
                            FontWeight::Medium
                        }),
                    )
                }
                view(style = NavBadgeSlot()) {
                    Mono(content = pct, size = MonoTextSize::Overline)
                }
            }
            view(style = RailBarSlot()) {
                Progress(
                    value = fraction,
                    tone = tone,
                    cap = ProgressCap::Rounded,
                )
            }
        }
    };

    tappable(vec![inner], move || {
        console.select_feature(index);
        dismiss_if_drawer(console);
    })
    .with_style(StyleApplication::new(nav_entry_style()).with("selected", arm.to_string()))
    .into_element()
}

/// Props for [`AllFeaturesLink`].
#[derive(Default, IdealystSchema)]
pub struct AllFeaturesLinkProps {
    /// Console state handles.
    pub console: Console,
}

/// The way to the features the rail is not showing.
///
/// Its label is a count, not an explanation of the rail's behaviour:
/// "All 14 features" is a fact the reader acts on, where "completed
/// features are hidden here" would be the app narrating its own logic
/// (rule 19).
#[component]
pub fn AllFeaturesLink(props: &AllFeaturesLinkProps) -> Element {
    let console = props.console;
    let data = console.data;
    let label = move || {
        let feats = data.features.get();
        let total = feats.len();
        let done = feats.iter().filter(|f| f.status == Status::Done).count();
        if done == 0 {
            format!("All {total} features \u{2192}")
        } else {
            format!("All {total} features, {done} complete \u{2192}")
        }
    };

    let inner: Element = ui! {
        text(style = QuietLink()) { label }
    };
    tappable(vec![inner], move || {
        console.show_features();
        dismiss_if_drawer(console);
    })
    .with_style(StyleApplication::new(quiet_link_box_style()))
    .into_element()
}

// Fills the shell's panel rather than sizing itself: `AppShell` owns
// the width (it bakes it into the panel and content-offset sheets), and
// a second declaration here would be the one that drifts.
stylesheet! {
    pub NavBox<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            min_height: 0,
            flex_direction: FlexDirection::Column,
            border_right_width: 1.0,
            border_color: t.color.border(),
            overflow: runtime_core::Overflow::Hidden,
        }
    }
}

stylesheet! {
    pub NavGroup<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: 1,
            padding_top: t.spacing.sm(),
            padding_horizontal: t.spacing.sm(),
        }
    }
}

stylesheet! {
    pub NavEntry<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            border_radius: t.radius.sm(),
            cursor: runtime_core::Cursor::Pointer,
            overflow: runtime_core::Overflow::Hidden,
        }
        variant selected {
            #[default]
            off(_t) { background: runtime_core::Color("#00000000".into()) }
            on(t) { background: t.intent.primary.soft_bg() }
        }
        transitions {
            background: 160ms EaseOut,
        }
        state hovered(t) {
            background: t.color.surface_alt(),
        }
    }
}

stylesheet! {
    pub NavRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
            padding_vertical: t.spacing.sm(),
            padding_horizontal: t.spacing.sm(),
            min_width: 0,
        }
    }
}

// The glyph column is what stays visible when the nav is collapsed, so
// it holds a fixed width and centres its mark — the marks have to line
// up as a column of their own at 56px.
stylesheet! {
    pub NavGlyph<IdeaThemeRef> {
        base(t) {
            width: 22,
            flex_shrink: 0.0,
            text_align: runtime_core::TextAlign::Center,
            font_family: runtime_core::FontFamily::System(String::from(crate::styles::MONO)),
            font_size: t.typography.caption_size(),
            color: t.color.text_muted(),
        }
    }
}

stylesheet! {
    pub NavLabelSlot<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            min_width: 0,
            flex_grow: 1.0,
            flex_shrink: 1.0,
        }
    }
}

stylesheet! {
    pub NavBadgeSlot<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    pub RailBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            min_height: 0,
            border_top_width: 1.0,
            border_color: t.color.border(),
            margin_top: t.spacing.sm(),
        }
    }
}

stylesheet! {
    pub RailHead<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
            padding_top: t.spacing.md(),
            padding_horizontal: t.spacing.md(),
            padding_bottom: t.spacing.xs(),
        }
    }
}

stylesheet! {
    pub RailScroll<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            min_height: 0,
        }
    }
}

stylesheet! {
    pub RailList<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: 1,
            padding_horizontal: t.spacing.sm(),
            padding_bottom: t.spacing.md(),
        }
    }
}

stylesheet! {
    pub RailInner<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
            padding_vertical: t.spacing.sm(),
            padding_horizontal: t.spacing.sm(),
            min_width: 0,
        }
    }
}

stylesheet! {
    pub RailTitleRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
            min_width: 0,
        }
    }
}

stylesheet! {
    pub RailNameSlot<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            min_width: 0,
            flex_grow: 1.0,
            flex_shrink: 1.0,
        }
    }
}

// The bar is inset to the label column so it reads as belonging to the
// name above it rather than to the rail's edge.
stylesheet! {
    pub RailBarSlot<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            margin_left: 16,
            min_width: 0,
        }
    }
}

stylesheet! {
    pub QuietLinkBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            border_radius: t.radius.sm(),
            padding_vertical: t.spacing.sm(),
            padding_horizontal: t.spacing.sm(),
            margin_top: t.spacing.xs(),
            cursor: runtime_core::Cursor::Pointer,
            overflow: runtime_core::Overflow::Hidden,
        }
        transitions {
            background: 160ms EaseOut,
        }
        state hovered(t) {
            background: t.color.surface_alt(),
        }
    }
}

stylesheet! {
    pub QuietLink<IdeaThemeRef> {
        base(t) {
            font_size: t.typography.caption_size(),
            color: t.intent.primary.fg(),
        }
    }
}


