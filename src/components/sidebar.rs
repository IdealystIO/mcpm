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
    component, pressable, stylesheet, switch, ui, AlignItems, Breakpoint, Element, FlexDirection,
    FontWeight, IdealystSchema, IntoElement, StyleApplication,
};

use crate::components::bits::{Mono, StatusDot};
use crate::model::{completed_count, features, in_play, rail_features, want_counts};
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

    // The destinations. Rebuilt on pane (for the selected arm) and on
    // rev (for the counts) — never on `nav_open`, which is carried by
    // the reactive width below so the collapse can animate.
    let nav = switch(
        move || (console.pane.get(), console.rev.get()),
        move |(pane, _rev): &(String, u64)| {
            let pane = pane.clone();
            let total = features().len();
            let (loose, _, _) = want_counts();
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

    let rail = switch(
        move || (console.pane.get(), console.feature.get(), console.rev.get()),
        move |(pane, sel, _rev): &(String, usize, u64)| {
            let (pane, sel) = (pane.clone(), *sel);
            let visible = rail_features(sel);
            let count = visible.len();
            let playing = in_play().len();
            let total = features().len();
            let on_feature = pane == "feature";
            let scope = format!("{playing} of {total}");
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

    pressable(vec![inner], move || {
        match id {
            "overview" => console.show_overview(),
            "features" => console.show_features(),
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
#[component]
pub fn RailFeature(props: &RailFeatureProps) -> Element {
    let console = props.console;
    let index = props.index;
    let feats = features();
    let Some(f) = feats.get(index) else {
        return ui! { view {} };
    };
    let name = f.name.clone();
    let status = f.status;
    let fraction = f.fraction();
    let pct = f.pct_label();
    let arm = if props.selected { "on" } else { "off" };

    let inner: Element = ui! {
        view(style = RailInner()) {
            view(style = RailTitleRow()) {
                StatusDot(status = status)
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
                    tone = status_tone(status),
                    cap = ProgressCap::Rounded,
                )
            }
        }
    };

    pressable(vec![inner], move || {
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
    let total = features().len();
    let done = completed_count();
    let label = if done == 0 {
        format!("All {total} features \u{2192}")
    } else {
        format!("All {total} features, {done} complete \u{2192}")
    };

    let inner: Element = ui! {
        text(style = QuietLink()) { label }
    };
    pressable(vec![inner], move || {
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


