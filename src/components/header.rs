//! Console masthead: project identity on the left, liveness + agent
//! count + theme toggle on the right.

use std::rc::Rc;

use idea_ui::{size, typography_kind, variant, Button, Field, IdeaThemeRef, Popover, Typography};
use idea_ui_nav::sidebar_pinned;
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide};
use runtime_core::{
    component, current_breakpoint, presence, pressable, stylesheet, switch, ui, AlignItems, Easing,
    Element,
    FlexDirection, FontWeight, IdealystSchema, IntoElement, JustifyContent, PresenceAnim,
    PresenceState, PressableHandle, Ref, StyleApplication,
};

use crate::components::bits::{Mono, StatusDot};
use crate::components::sidebar::PIN_AT;
use crate::model::{active_agent_count, Status};
use crate::state::Console;
use crate::styles::MonoTextSize;

/// Props for [`Header`].
#[derive(Default, IdealystSchema)]
pub struct HeaderProps {
    /// Console state handles (dark-mode toggle lives here).
    pub console: Console,
}

/// The console masthead.
///
/// Built ONCE, with its live parts behind their own `switch`es. The
/// bar itself must not be rebuilt by a poll: it anchors the key
/// popover, and re-creating the anchor under an open popover moves it
/// out from under the pointer that opened it (rule 25).
#[component]
pub fn Header(props: &HeaderProps) -> Element {
    let console = props.console;

    let identity = switch(
        move || console.rev.get(),
        move |_: &u64| {
            let name = crate::model::project_name();
            ui! {
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
        },
    );

    // The socket's own state, not an inference from "a fetch worked
    // once": when it is open every committed event is already on its
    // way here, and when it is not the console is running on its
    // fallback poll and should say so.
    let live = switch(
        move || console.connected.get(),
        move |connected: &bool| {
            let (label, status) = if *connected {
                ("live", Status::Done)
            } else {
                ("polling", Status::Queued)
            };
            ui! {
                view(style = LivePill()) {
                    StatusDot(status = status)
                    Typography(content = label, kind = typography_kind::Caption, muted = true)
                }
            }
        },
    );

    let agents = switch(
        move || console.rev.get(),
        move |_: &u64| {
            let label = format!("{} agents live", active_agent_count());
            ui! {
                Typography(content = label, kind = typography_kind::Caption, muted = true)
            }
        },
    );

    ui! {
        view(style = HeaderBar()) {
            view(style = HeaderSide()) {
                NavButton(console = console)
                view(style = LogoBox()) {
                    text(style = LogoGlyph()) { "MCP" }
                }
                view(style = TitleCol()) {
                    identity
                }
            }
            view(style = HeaderSide()) {
                live
                agents
                KeyPill(console = console)
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

/// Props for [`NavButton`].
#[derive(Default, IdealystSchema)]
pub struct NavButtonProps {
    /// Console state handles.
    pub console: Console,
}

/// The way to the nav when the nav is a drawer.
///
/// Present only below the pin breakpoint, because above it the nav is
/// already on screen and a control that opens what you can see is a
/// control with nothing to do. The `switch` is keyed on the breakpoint
/// itself, so a resize past the threshold adds or removes the button
/// without touching the rest of the masthead.
#[component]
pub fn NavButton(props: &NavButtonProps) -> Element {
    let console = props.console;
    switch(
        move || {
            current_breakpoint().get();
            sidebar_pinned(PIN_AT)
        },
        move |pinned: &bool| {
            if *pinned {
                return ui! { view {} };
            }
            let inner: Element = ui! {
                text(style = NavGlyph()) { "\u{2261}" }
            };
            pressable(vec![inner], move || console.toggle_nav())
                .with_style(StyleApplication::new(nav_button_box_style()))
                .into_element()
        },
    )
}

stylesheet! {
    pub NavButtonBox<IdeaThemeRef> {
        base(t) {
            width: 30,
            height: 30,
            flex_shrink: 0.0,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            border_radius: t.radius.md(),
            cursor: runtime_core::Cursor::Pointer,
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
    pub NavGlyph<IdeaThemeRef> {
        base(t) {
            font_size: t.typography.body_lg_size(),
            color: t.color.text(),
        }
    }
}

/// Props for [`KeyPill`].
#[derive(Default, IdealystSchema)]
pub struct KeyPillProps {
    /// Console state handles.
    pub console: Console,
}

/// The API key affordance: says whether the console is holding one, and
/// opens the popover that changes it.
///
/// It shows in both postures on purpose. On an open loopback host "no
/// key" is the correct, expected state rather than a warning — which is
/// why it reads as plain muted text and not an alert (rule 21: nothing
/// here needs clearing).
#[component]
pub fn KeyPill(props: &KeyPillProps) -> Element {
    let console = props.console;
    let trigger: Ref<PressableHandle> = Ref::new();

    let label = switch(
        move || console.api_key.get().is_empty(),
        move |empty: &bool| {
            let label = if *empty { "no key" } else { "key set" };
            let status = if *empty { Status::Queued } else { Status::Done };
            ui! {
                view(style = PillRow()) {
                    StatusDot(status = status)
                    Typography(content = label, kind = typography_kind::Caption, muted = true)
                }
            }
        },
    );

    let pill = pressable(vec![label], move || console.show_key())
        .bind(trigger)
        .with_style(StyleApplication::new(key_pill_box_style()))
        .into_element();

    // `presence` owns the timing so the panel can fade and lift on the
    // way in and back out — gating the Popover on a bare `if` would
    // drop the subtree on the same frame it was told to close.
    let panel = presence(move || ui! { KeyPanel(console = console, anchor = Some(trigger)) })
        .present(move || console.key_open.get())
        .enter(PresenceAnim::new(
            PresenceState::default().opacity(0.0).translate_y(-4.0).scale(0.98),
            140,
            Easing::EaseOut,
        ))
        .exit(PresenceAnim::new(
            PresenceState::default().opacity(0.0).translate_y(-4.0).scale(0.98),
            110,
            Easing::EaseIn,
        ))
        .into_element();

    ui! {
        view(style = KeyAnchor()) {
            pill
            panel
        }
    }
}

/// Props for [`KeyPanel`].
#[derive(Default, IdealystSchema)]
pub struct KeyPanelProps {
    /// Console state handles.
    pub console: Console,
    /// The pill to hang off.
    pub anchor: Option<Ref<PressableHandle>>,
}

/// The key popover's contents: the field, and the two things you can
/// do with it.
///
/// A popover is a compact surface, not a card (rule 2) — so there is no
/// standing prose here explaining what a key is. The screen that DOES
/// explain it is [`crate::components::gate::KeyGate`], which is what a
/// refused console shows, where the reader has actually been stopped.
#[component]
pub fn KeyPanel(props: &KeyPanelProps) -> Element {
    let console = props.console;
    let Some(anchor) = props.anchor else {
        return ui! { view {} };
    };
    let dismiss: Rc<dyn Fn()> = Rc::new(move || console.dismiss_key());
    let on_draft: Rc<dyn Fn(String)> = Rc::new(move |t| console.key_draft.set(t));
    let save: Rc<dyn Fn()> = Rc::new(move || console.save_key(console.key_draft.get()));
    let forget: Rc<dyn Fn()> = Rc::new(move || {
        console.key_draft.set(String::new());
        console.save_key(String::new());
    });

    ui! {
        Popover(
            target = Some(AnchorTarget::from(anchor)),
            side = ElementSide::Below,
            align = ElementAlign::End,
            offset = 8.0,
            on_dismiss = Some(dismiss),
        ) {
            view(style = KeyPanelBox()) {
                text(style = crate::styles::SectionLabel()) { "Console key" }
                Field(
                    value = console.key_draft,
                    on_change = on_draft,
                    placeholder = Some("mcpm_\u{2026}".to_string()),
                )
                view(style = KeyActions()) {
                    Button(
                        label = "Use this key",
                        on_click = save,
                        size = size::Sm,
                        // The caveat rides the control rather than
                        // standing beside it as prose (rule 16).
                        disabled = runtime_core::rx!(
                            console.key_draft.get().trim() == console.api_key.get().trim()
                        ),
                    )
                    Button(
                        label = "Forget",
                        on_click = forget,
                        size = size::Sm,
                        variant = variant::Ghost,
                        disabled = runtime_core::rx!(console.api_key.get().is_empty()),
                    )
                }
            }
        }
    }
}

stylesheet! {
    pub KeyAnchor<IdeaThemeRef> {
        base(_t) {
            position: runtime_core::Position::Relative,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
        }
    }
}

// Tight, per rule 2 — the content defines the size.
stylesheet! {
    pub KeyPanelBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
            padding: t.spacing.md(),
            width: 288,
        }
    }
}

stylesheet! {
    pub KeyActions<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
        }
    }
}

stylesheet! {
    pub KeyPillBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.pill(),
            padding_vertical: 4,
            padding_horizontal: 10,
            cursor: runtime_core::Cursor::Pointer,
        }
        transitions {
            background: 160ms EaseOut,
            border_color: 160ms EaseOut,
            opacity: 160ms EaseOut,
        }
        state hovered(t) {
            border_color: t.color.border_hover(),
            background: t.color.surface_alt(),
        }
    }
}

stylesheet! {
    pub PillRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
        }
    }
}

stylesheet! {
    pub HeaderBar<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            // The masthead is chrome: it sizes to its content and never
            // pays for a taller body. Without this a pane that overflows
            // the page column shrinks the header instead of scrolling.
            flex_shrink: 0.0,
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
