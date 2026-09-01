//! Left rail: the want pool at the top (loose ideas, project-wide),
//! then one card per feature still in play.
//!
//! The rail is for steering work in flight, so a completed feature
//! drops out of it — on a long-lived project the finished ones are the
//! majority, and a list you have to scroll past to reach the thing you
//! are working on has stopped being a rail. They are all still one
//! click away on the Features screen, which the entry under the cards
//! opens and which says how many it is holding.

use idea_ui::{typography_kind, Badge, IdeaThemeRef, Progress, Spacer, Stack, StackAlign,
    StackAxis, StackGap, Typography};
use runtime_core::{
    component, pressable, stylesheet, switch, ui, Element, FlexDirection, FontWeight,
    IdealystSchema, IntoElement, StyleApplication,
};

use crate::components::bits::{Mono, StatusBadge};
use crate::model::{completed_count, features, rail_features};
use crate::state::Console;
use crate::styles::{status_tone, SectionLabel};

/// Props for [`Sidebar`].
#[derive(Default, IdealystSchema)]
pub struct SidebarProps {
    /// Console state handles.
    pub console: Console,
}

/// The left feature rail.
#[component]
pub fn Sidebar(props: &SidebarProps) -> Element {
    let console = props.console;
    // Rebuild the rail whenever the pane, the selection, or the data
    // changes.
    let pool = switch(
        move || (console.pane.get(), console.rev.get()),
        move |(pane, _rev): &(String, u64)| {
            let on_pool = pane == "wants";
            ui! {
                view(style = CardList()) {
                    WantPoolCard(console = console, selected = on_pool)
                }
            }
        },
    );
    // The knowledge base sits with the want pool above the features:
    // both are project-wide and neither belongs to the feature you
    // happen to have selected.
    let knowledge = switch(
        move || (console.pane.get(), console.rev.get()),
        move |(pane, _rev): &(String, u64)| {
            let on_knowledge = pane == "knowledge";
            ui! {
                view(style = CardList()) {
                    KnowledgeCard(console = console, selected = on_knowledge)
                }
            }
        },
    );
    let cards = switch(
        move || (console.pane.get(), console.feature.get(), console.rev.get()),
        move |(pane, sel, _rev): &(String, usize, u64)| {
            let (pane, sel) = (pane.clone(), *sel);
            let visible = rail_features(sel);
            let count = visible.len();
            let on_feature = pane == "feature";
            ui! {
                view(style = CardList()) {
                    for i in 0..count {
                        FeatureCard(
                            console = console,
                            index = visible[i],
                            selected = on_feature && visible[i] == sel,
                        )
                    }
                }
            }
        },
    );

    // Sits below the cards, where a reader who did not find what they
    // wanted in the rail is already looking.
    let all = switch(
        move || (console.pane.get(), console.rev.get()),
        move |(pane, _rev): &(String, u64)| {
            let on_all = pane == "features";
            ui! {
                view(style = CardList()) {
                    AllFeaturesCard(console = console, selected = on_all)
                }
            }
        },
    );
    ui! {
        view(style = SidebarBox()) {
            view(style = SidebarPad()) {
                text(style = SectionLabel()) { "Ideas" }
            }
            pool
            view(style = SidebarPad()) {
                text(style = SectionLabel()) { "Knowledge" }
            }
            knowledge
            view(style = SidebarPad()) {
                text(style = SectionLabel()) { "Features" }
            }
            scroll_view(style = SidebarScroll()) {
                cards
                all
            }
        }
    }
}

/// Props for [`KnowledgeCard`].
#[derive(Default, IdealystSchema)]
pub struct KnowledgeCardProps {
    /// Console state handles.
    pub console: Console,
    /// Whether the knowledge screen is the one showing.
    pub selected: bool,
}

/// The way into the knowledge base.
///
/// It carries no count. Unlike the pool's loose-want number, a total
/// here is not something anyone acts on — nobody drains the knowledge
/// base — and the console would have to fetch it on every poll to
/// print it (see `KnowledgeView`: the base is queried, not snapshotted).
#[component]
pub fn KnowledgeCard(props: &KnowledgeCardProps) -> Element {
    let console = props.console;
    let inner: Element = ui! {
        view(style = CardInner()) {
            Stack(axis = StackAxis::Row, gap = StackGap::Sm, align = StackAlign::Center) {
                view(style = TitleSlot()) {
                    Typography(
                        content = "Knowledge base",
                        kind = typography_kind::BodySm,
                        weight = Some(FontWeight::SemiBold),
                    )
                }
                Spacer()
            }
            Mono(content = "conventions, decisions, gotchas")
        }
    };

    let arm = if props.selected { "on" } else { "off" };
    pressable(vec![inner], move || console.show_knowledge())
        .with_style(
            StyleApplication::new(feature_card_box_style()).with("selected", arm.to_string()),
        )
        .into_element()
}

/// Props for [`AllFeaturesCard`].
#[derive(Default, IdealystSchema)]
pub struct AllFeaturesCardProps {
    /// Console state handles.
    pub console: Console,
    /// Whether the features screen is the one showing.
    pub selected: bool,
}

/// The way to the features the rail is not showing.
///
/// Its subtitle is a count, not an explanation of the rail's behaviour:
/// "12 complete" is a fact the reader can act on, where "completed
/// features are hidden here to reduce clutter" would be the app
/// narrating its own logic (rule 19).
#[component]
pub fn AllFeaturesCard(props: &AllFeaturesCardProps) -> Element {
    let console = props.console;
    let total = features().len();
    let done = completed_count();
    let summary = if total == 0 {
        "nothing planned yet".to_string()
    } else if done == 0 {
        format!("{total} in total")
    } else {
        format!("{total} in total \u{b7} {done} complete")
    };

    let inner: Element = ui! {
        view(style = CardInner()) {
            Stack(axis = StackAxis::Row, gap = StackGap::Sm, align = StackAlign::Center) {
                view(style = TitleSlot()) {
                    Typography(
                        content = "All features",
                        kind = typography_kind::BodySm,
                        weight = Some(FontWeight::SemiBold),
                    )
                }
                Spacer()
            }
            Mono(content = summary)
        }
    };

    let arm = if props.selected { "on" } else { "off" };
    pressable(vec![inner], move || console.show_features())
        .with_style(
            StyleApplication::new(feature_card_box_style()).with("selected", arm.to_string()),
        )
        .into_element()
}

/// Props for [`WantPoolCard`].
#[derive(Default, IdealystSchema)]
pub struct WantPoolCardProps {
    /// Console state handles.
    pub console: Console,
    /// Whether the pool pane is the one showing.
    pub selected: bool,
}

/// The want pool entry: the project-wide inbox of loose ideas that
/// features get composed out of.
#[component]
pub fn WantPoolCard(props: &WantPoolCardProps) -> Element {
    let console = props.console;
    let (loose, composed, declined) = crate::model::want_counts();
    let summary = if loose + composed + declined == 0 {
        "nothing captured yet".to_string()
    } else {
        format!("{loose} loose · {composed} composed")
    };
    let loose_label = format!("{loose}");

    let inner: Element = ui! {
        view(style = CardInner()) {
            Stack(axis = StackAxis::Row, gap = StackGap::Sm, align = StackAlign::Center) {
                view(style = TitleSlot()) {
                    Typography(
                        content = "Want pool",
                        kind = typography_kind::BodySm,
                        weight = Some(FontWeight::SemiBold),
                    )
                }
                Spacer()
                if loose > 0 {
                    view(style = BadgeSlot()) {
                        Badge(label = loose_label, tone = idea_ui::tone::Warning)
                    }
                }
            }
            Mono(content = summary)
        }
    };

    let arm = if props.selected { "on" } else { "off" };
    pressable(vec![inner], move || console.show_wants())
        .with_style(
            StyleApplication::new(feature_card_box_style()).with("selected", arm.to_string()),
        )
        .into_element()
}

/// Props for [`FeatureCard`].
#[derive(Default, IdealystSchema)]
pub struct FeatureCardProps {
    /// Console state handles.
    pub console: Console,
    /// Index into [`features`].
    pub index: usize,
    /// Whether this card is the selected feature.
    pub selected: bool,
}

/// One selectable feature card in the rail.
#[component]
pub fn FeatureCard(props: &FeatureCardProps) -> Element {
    let console = props.console;
    let index = props.index;
    let feats = features();
    let f = &feats[index];
    let name = f.name.clone();
    let status = f.status;
    let fraction = f.fraction();
    let agent = f.agent.to_string();
    let pct = f.pct_label();

    let inner: Element = ui! {
        view(style = CardInner()) {
            Stack(axis = StackAxis::Row, gap = StackGap::Sm, align = StackAlign::Center) {
                view(style = TitleSlot()) {
                    Typography(
                        content = name,
                        kind = typography_kind::BodySm,
                        weight = Some(FontWeight::SemiBold),
                    )
                }
                Spacer()
                view(style = BadgeSlot()) {
                    StatusBadge(status = status)
                }
            }
            Progress(value = fraction, tone = status_tone(status))
            Stack(axis = StackAxis::Row, gap = StackGap::Sm, align = StackAlign::Center) {
                view(style = TitleSlot()) {
                    Mono(content = agent)
                }
                Spacer()
                view(style = BadgeSlot()) {
                    Mono(content = pct)
                }
            }
        }
    };

    let arm = if props.selected { "on" } else { "off" };
    pressable(vec![inner], move || console.select_feature(index))
        .with_style(
            StyleApplication::new(feature_card_box_style()).with("selected", arm.to_string()),
        )
        .into_element()
}

stylesheet! {
    pub SidebarBox<IdeaThemeRef> {
        base(t) {
            width: 264,
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            border_right_width: 1.0,
            border_color: t.color.border(),
            background: t.color.surface(),
            min_height: 0,
        }
    }
}

stylesheet! {
    pub SidebarPad<IdeaThemeRef> {
        base(t) {
            padding_top: t.spacing.lg(),
            padding_horizontal: t.spacing.lg(),
            padding_bottom: t.spacing.sm(),
        }
    }
}

stylesheet! {
    pub SidebarScroll<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            min_height: 0,
        }
    }
}

stylesheet! {
    pub CardList<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
            padding_horizontal: t.spacing.sm(),
            padding_bottom: t.spacing.md(),
        }
    }
}

stylesheet! {
    pub FeatureCardBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            border_width: 1.0,
            border_radius: t.radius.md(),
            cursor: runtime_core::Cursor::Pointer,
            // A name with no break opportunity in it — an id, a slug —
            // cannot wrap however much it is allowed to shrink. Clip it
            // at the card's own edge rather than let it draw across the
            // sidebar's border.
            overflow: runtime_core::Overflow::Hidden,
        }
        variant selected {
            #[default]
            off(t) {
                border_color: t.color.border(),
                background: t.color.surface(),
            }
            on(t) {
                border_color: t.intent.primary.border(),
                background: t.intent.primary.soft_bg(),
            }
        }
        state hovered(t) {
            border_color: t.color.border_hover(),
        }
    }
}

// The two halves of a card's title row. A `text` node in a flex row
// takes its intrinsic (unwrapped) width by default, so a long feature
// name pushed the badge — and itself — straight through the card's
// right edge. `min_width: 0` is what lets the title shrink below that
// intrinsic width and wrap instead; without it `flex_shrink` has
// nothing to shrink against. The badge must NOT shrink: a status pill
// squeezed to three letters is worse than a title on two lines.
stylesheet! {
    pub TitleSlot<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            min_width: 0,
            flex_grow: 1.0,
            flex_shrink: 1.0,
        }
    }
}

stylesheet! {
    pub BadgeSlot<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: runtime_core::AlignItems::Center,
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    pub CardInner<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
            padding: t.spacing.md(),
        }
    }
}





