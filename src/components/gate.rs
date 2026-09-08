//! The API key screen: where an operator hands the console the
//! credential a gated `mcpm-web` demands.
//!
//! It is a screen rather than a modal because pasting a key is a task
//! with its own state, and because it is what the console shows INSTEAD
//! of data when the host refuses it — there is nothing behind it to
//! dim (rule 9).

use std::rc::Rc;

use idea_ui::{size, typography_kind, variant, Button, Field, IdeaThemeRef, Spacer, Typography};
use runtime_core::{
    component, rx, stylesheet, switch, ui, AlignItems, Element, FlexDirection, FontWeight,
    IdealystSchema, JustifyContent,
};

use crate::state::Console;

/// Props for [`KeyGate`].
#[derive(Default, IdealystSchema)]
pub struct KeyGateProps {
    /// Console state handles.
    pub console: Console,
}

/// The key screen. Reached from the header's key pill, and shown
/// unbidden when the host has refused the key we hold.
#[component]
pub fn KeyGate(props: &KeyGateProps) -> Element {
    let console = props.console;
    let on_draft: Rc<dyn Fn(String)> = Rc::new(move |t| console.key_draft.set(t));
    let save: Rc<dyn Fn()> = Rc::new(move || console.save_key(console.key_draft.get()));

    // The heading answers "why am I looking at this", which is a
    // different question when the host just refused you than when you
    // came here to rotate a key.
    let head = switch(
        move || (console.denied.get(), console.api_key.get().is_empty()),
        move |state: &(bool, bool)| {
            let (denied, empty) = *state;
            let title = if denied && empty {
                "This host needs a key"
            } else if denied {
                "This key was refused"
            } else {
                "API key"
            };
            ui! {
                view(style = HeadCol()) {
                    Typography(
                        content = title,
                        kind = typography_kind::H2,
                        weight = Some(FontWeight::SemiBold),
                    )
                }
            }
        },
    );

    // The one instruction the reader cannot infer from the screen: the
    // command that produces the thing the field is asking for.
    let issue = "mcpm-mcp --issue-key --agent console --role console";

    let clear = switch(
        move || console.api_key.get().is_empty(),
        move |empty: &bool| {
            if *empty {
                return ui! { view {} };
            }
            let on_clear: Rc<dyn Fn()> = Rc::new(move || {
                console.key_draft.set(String::new());
                console.api_key.set(String::new());
            });
            ui! {
                Button(
                    label = "Forget key",
                    on_click = on_clear,
                    size = size::Sm,
                    variant = variant::Ghost,
                )
            }
        },
    );

    ui! {
        view(style = GateBox()) {
            view(style = GateCard()) {
                head
                Field(
                    value = console.key_draft,
                    on_change = on_draft,
                    label = Some("Key".to_string()),
                    placeholder = Some("mcpm_…".to_string()),
                )
                view(style = IssueRow()) {
                    Typography(
                        content = "Issue one on the server:",
                        kind = typography_kind::Caption,
                        muted = true,
                    )
                    text(style = crate::styles::MonoText()) { issue }
                }
                view(style = ActionRow()) {
                    clear
                    Spacer()
                    Button(
                        label = "Use this key",
                        on_click = save,
                        // Disabled until there is something to send, so
                        // the caveat rides the control rather than
                        // standing beside it as prose (rule 16).
                        disabled = rx!(console.key_draft.get().trim().is_empty()),
                    )
                }
            }
        }
    }
}

stylesheet! {
    pub GateBox<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            min_width: 0,
            min_height: 0,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            padding: t.spacing.xl(),
            background: t.color.background(),
        }
    }
}

stylesheet! {
    pub GateCard<IdeaThemeRef> {
        base(t) {
            width: 440,
            max_width: runtime_core::Length::Percent(100.0),
            flex_direction: FlexDirection::Column,
            gap: t.spacing.lg(),
            padding: t.spacing.xl(),
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.md(),
            background: t.color.surface(),
        }
    }
}

stylesheet! {
    pub HeadCol<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
            min_width: 0,
        }
    }
}

stylesheet! {
    pub IssueRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
            min_width: 0,
        }
    }
}

stylesheet! {
    pub ActionRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::KeyGate;
    use crate::state::use_console;

    /// The gate is what a locked-out operator sees, so a panic here
    /// would leave them with a blank page and no way to fix it — the
    /// one screen that must mount even when nothing else can.
    #[test]
    fn the_key_gate_mounts_in_both_of_its_moods() {
        for denied in [false, true] {
            let harness = host_mock::Harness::with_registry(crate::register_scene_extensions);
            let tree = harness.world.enter(|| {
                idea_ui::install_idea_theme(idea_ui::light_theme());
                let console = use_console();
                console.denied.set(denied);
                runtime_core::ui! { KeyGate(console = console) }
            });
            harness.mount(tree);
            harness.flush();
        }
    }
}
