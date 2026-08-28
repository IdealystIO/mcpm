//! Left rail: the want pool at the top (loose ideas, project-wide),
//! then one selectable card per feature.

use idea_ui::{typography_kind, Badge, IdeaThemeRef, Progress, Spacer, Stack, StackAlign,
    StackAxis, StackGap, Typography};
use runtime_core::{
    component, pressable, stylesheet, switch, ui, Element, FlexDirection, FontWeight,
    IdealystSchema, IntoElement, StyleApplication,
};

use crate::components::bits::{Mono, StatusBadge};
use crate::model::features;
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
    let cards = switch(
        move || (console.pane.get(), console.feature.get(), console.rev.get()),
        move |(pane, sel, _rev): &(String, usize, u64)| {
            let (pane, sel) = (pane.clone(), *sel);
            let count = features().len();
            let on_feature = pane == "feature";
            ui! {
                view(style = CardList()) {
                    for i in 0..count {
                        FeatureCard(console = console, index = i, selected = on_feature && i == sel)
                    }
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
                text(style = SectionLabel()) { "Features" }
            }
            scroll_view(style = SidebarScroll()) { cards }
        }
    }
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
                Typography(
                    content = "Want pool",
                    kind = typography_kind::BodySm,
                    weight = Some(FontWeight::SemiBold),
                )
                Spacer()
                if loose > 0 {
                    Badge(label = loose_label, tone = idea_ui::tone::Warning)
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
                Typography(
                    content = name,
                    kind = typography_kind::BodySm,
                    weight = Some(FontWeight::SemiBold),
                )
                Spacer()
                StatusBadge(status = status)
            }
            Progress(value = fraction, tone = status_tone(status))
            Stack(axis = StackAxis::Row, align = StackAlign::Center) {
                Mono(content = agent)
                Spacer()
                Mono(content = pct)
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

stylesheet! {
    pub CardInner<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
            padding: t.spacing.md(),
        }
    }
}





