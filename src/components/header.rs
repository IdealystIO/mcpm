//! Console masthead: project identity on the left, liveness + agent
//! count + theme toggle on the right.

use idea_ui::{typography_kind, IdeaThemeRef, Typography};
use runtime_core::{
    component, stylesheet, ui, AlignItems, Element, FlexDirection, FontWeight, IdealystSchema,
    JustifyContent,
};

use crate::components::bits::{Mono, StatusDot};
use crate::model::{active_agent_count, Status};
use crate::state::Console;
use crate::styles::MonoTextSize;

/// Props for [`Header`].
#[derive(Default, IdealystSchema)]
pub struct HeaderProps {
    /// Console state handles (dark-mode toggle lives here).
    pub console: Console,
}

/// The console masthead. Rebuilt on every data revision so the project
/// name, liveness, and agent count track the store.
#[component]
pub fn Header(props: &HeaderProps) -> Element {
    let console = props.console;
    // Keyed on the connection too, so the live pill flips the moment
    // the socket opens or drops rather than at the next data change.
    runtime_core::switch(
        move || (console.rev.get(), console.connected.get()),
        move |_: &(u64, bool)| header_body(console),
    )
}

fn header_body(console: Console) -> Element {
    let name = crate::model::project_name();
    let agents = format!("{} agents live", active_agent_count());
    // The socket's own state, not an inference from "a fetch worked
    // once": when it is open every committed event is already on its
    // way here, and when it is not the console is running on its
    // fallback poll and should say so.
    let connected = console.connected.get();
    let live_label = if connected { "live" } else { "polling" };
    let live_status = if connected { Status::Done } else { Status::Queued };
    ui! {
        view(style = HeaderBar()) {
            view(style = HeaderSide()) {
                view(style = LogoBox()) {
                    text(style = LogoGlyph()) { "MCP" }
                }
                view(style = TitleCol()) {
                    view(style = TitleRow()) {
                        Typography(
                            content = name,
                            kind = typography_kind::BodyLg,
                            weight = Some(FontWeight::SemiBold),
                        )
                        view(style = BranchChip()) {
                            Mono(content = "mcpm", size = MonoTextSize::Overline)
                        }
                    }
                }
            }
            view(style = HeaderSide()) {
                view(style = LivePill()) {
                    StatusDot(status = live_status)
                    Typography(content = live_label, kind = typography_kind::Caption, muted = true)
                }
                Typography(content = agents, kind = typography_kind::Caption, muted = true)
                view(style = LivePill()) {
                    Typography(content = "dark", kind = typography_kind::Caption, muted = true)
                    toggle(
                        value = console.dark,
                        on_change = move |v| console.dark.set(v),
                    )
                }
            }
        }
    }
}

stylesheet! {
    pub HeaderBar<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            gap: t.spacing.xl(),
            padding_vertical: t.spacing.md(),
            padding_horizontal: t.spacing.xl(),
            border_bottom_width: 1.0,
            border_color: t.color.border(),
            background: t.color.surface(),
        }
    }
}

stylesheet! {
    pub HeaderSide<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.md(),
        }
    }
}

stylesheet! {
    pub LogoBox<IdeaThemeRef> {
        base(t) {
            width: 30,
            height: 30,
            border_radius: t.radius.md(),
            background: t.intent.primary.soft_bg(),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
        }
    }
}

stylesheet! {
    pub LogoGlyph<IdeaThemeRef> {
        base(t) {
            font_size: t.typography.overline_size(),
            font_weight: FontWeight::Bold,
            color: t.intent.primary.soft_text(),
        }
    }
}

stylesheet! {
    pub TitleCol<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: 2,
        }
    }
}

stylesheet! {
    pub TitleRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
        }
    }
}

stylesheet! {
    pub BranchChip<IdeaThemeRef> {
        base(t) {
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.sm(),
            padding_vertical: 2,
            padding_horizontal: 6,
        }
    }
}

stylesheet! {
    pub LivePill<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.pill(),
            padding_vertical: 4,
            padding_horizontal: 10,
        }
    }
}

stylesheet! {
    pub HeaderDivider<IdeaThemeRef> {
        base(t) {
            width: 1,
            height: 14,
            background: t.color.border(),
        }
    }
}
