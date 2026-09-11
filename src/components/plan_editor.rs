//! The plan editor: compose a feature by hand — name, description,
//! whitepaper, and its modules with their tasks, prerequisites and
//! owned paths — and send it through the same `plan_feature` the
//! agents use.
//!
//! A screen, not a modal: a plan is a multi-step thing to write
//! (UX_GUIDELINES rule 9). Every buffer lives on `Console` — the
//! module slots are a fixed pool created with it (see
//! `state::PLAN_SLOTS`) — so a tick landing mid-edit rebuilds nothing
//! the reader is typing into.

use std::rc::Rc;

use idea_ui::{size, tone, typography_kind, variant, Button, Chip, Field, IdeaThemeRef, Spacer,
    Textarea, Typography};
use runtime_core::{
    component, rx, stylesheet, switch, ui, AlignItems, Element, FlexDirection, FlexWrap,
    FontWeight, IdealystSchema, JustifyContent,
};

use crate::state::{Console, Edit, PLAN_SLOTS};
use crate::styles::SectionLabel;

/// Props for [`PlanEditor`].
#[derive(Default, IdealystSchema)]
pub struct PlanEditorProps {
    /// Console state handles.
    pub console: Console,
}

/// The new-plan screen.
#[component]
pub fn PlanEditor(props: &PlanEditorProps) -> Element {
    let console = props.console;
    let on_name: Rc<dyn Fn(String)> = Rc::new(move |v| console.plan_name.set(v));
    let on_description: Rc<dyn Fn(String)> = Rc::new(move |v| console.plan_description.set(v));
    let on_whitepaper: Rc<dyn Fn(String)> = Rc::new(move |v| console.plan_whitepaper.set(v));
    let cancel: Rc<dyn Fn()> = Rc::new(move || {
        console.reset_plan_draft();
        console.show_features();
    });
    let create: Rc<dyn Fn()> = Rc::new(move || console.submit(Edit::CreatePlan));
    let add: Rc<dyn Fn()> = Rc::new(move || console.add_plan_module());

    // The module cards, rebuilt when a slot is added or removed. Their
    // fields bind to the slots' own buffers, so the rebuild costs
    // focus and nothing else.
    let modules = switch(
        move || console.plan_count.get(),
        move |&count: &usize| {
            ui! {
                view(style = ModuleList()) {
                    for i in 0..count, key = i {
                        ModuleSlotCard(console = console, index = i)
                    }
                }
            }
        },
    );

    let refusal = switch(
        move || console.form_error.get(),
        move |error: &String| {
            if error.is_empty() {
                return ui! { view {} };
            }
            let error = error.clone();
            ui! { text(style = RefusalLine()) { error } }
        },
    );

    ui! {
        view(style = ScreenBox()) {
            view(style = ScreenHead()) {
                Typography(content = "New plan", kind = typography_kind::H2, weight = Some(FontWeight::SemiBold))
                Spacer()
                Button(label = "Cancel", on_click = cancel, variant = variant::Ghost, size = size::Sm)
                Button(
                    label = "Create plan",
                    on_click = create,
                    loading = rx!(console.form_busy.get()),
                    disabled = rx!(console.plan_name.get().trim().is_empty()),
                )
            }
            scroll_view(style = ScreenScroll()) {
                view(style = ScreenPad()) {
                    view(style = FormCol()) {
                        Field(
                            value = console.plan_name,
                            on_change = on_name,
                            label = Some("Feature".to_string()),
                            placeholder = Some("What the feature is called".to_string()),
                        )
                        Textarea(
                            value = console.plan_description,
                            on_change = on_description,
                            label = Some("Description".to_string()),
                            placeholder = Some("What it is for, in a sentence or two.".to_string()),
                            rows = 2u32,
                            max_rows = 6u32,
                        )
                        Textarea(
                            value = console.plan_whitepaper,
                            on_change = on_whitepaper,
                            label = Some("Whitepaper".to_string()),
                            placeholder = Some("The plan as prose \u{2014} what you would tell a new hire. Markdown.".to_string()),
                            rows = 6u32,
                            max_rows = 30u32,
                        )
                    }
                    view(style = SectionHead()) {
                        text(style = SectionLabel()) { "Modules" }
                        Spacer()
                        Button(
                            label = "Add module",
                            on_click = add,
                            size = size::Sm,
                            variant = variant::Soft,
                            disabled = rx!(console.plan_count.get() >= PLAN_SLOTS),
                        )
                    }
                    modules
                    refusal
                }
            }
        }
    }
}

/// Props for [`ModuleSlotCard`].
#[derive(Default, IdealystSchema)]
pub struct ModuleSlotCardProps {
    /// Console state handles.
    pub console: Console,
    /// Which slot.
    pub index: usize,
}

/// One module of the plan being composed.
#[component]
pub fn ModuleSlotCard(props: &ModuleSlotCardProps) -> Element {
    let console = props.console;
    let index = props.index;
    let slot = console.plan_modules[index];
    let heading = format!("Module {}", index + 1);
    let on_name: Rc<dyn Fn(String)> = Rc::new(move |v| slot.name.set(v));
    let on_description: Rc<dyn Fn(String)> = Rc::new(move |v| slot.description.set(v));
    let on_tasks: Rc<dyn Fn(String)> = Rc::new(move |v| slot.tasks.set(v));
    let on_owns: Rc<dyn Fn(String)> = Rc::new(move |v| slot.owns.set(v));
    let remove: Rc<dyn Fn()> = Rc::new(move || console.remove_plan_module(index));

    // The prerequisite chips: every other slot in use, by its current
    // name. Keyed on those names, so a name typed into another card
    // shows up here as it is typed — this row holds no text field of
    // its own, so the rebuild costs nothing.
    let deps = switch(
        move || {
            let count = console.plan_count.get();
            let names: Vec<String> =
                (0..count).map(|i| console.plan_modules[i].name.get()).collect();
            (names, slot.deps.get())
        },
        move |state: &(Vec<String>, Vec<usize>)| {
            let (names, picked) = state.clone();
            let others: Vec<usize> = (0..names.len()).filter(|&i| i != index).collect();
            let n = others.len();
            ui! {
                view(style = DepsCol()) {
                    text(style = SectionLabel()) { "Depends on" }
                    if n == 0 {
                        Typography(content = "No other modules yet.", kind = typography_kind::Caption, muted = true)
                    }
                    view(style = ChipRow()) {
                        for i in 0..n {
                            DepChip(
                                console = console,
                                slot = index,
                                other = others[i],
                                label = if names[others[i]].trim().is_empty() {
                                    format!("Module {}", others[i] + 1)
                                } else {
                                    names[others[i]].clone()
                                },
                                selected = picked.contains(&others[i]),
                            )
                        }
                    }
                }
            }
        },
    );

    ui! {
        view(style = SlotCard()) {
            view(style = SlotHead()) {
                text(style = SectionLabel()) { heading }
                Spacer()
                Button(
                    label = "Remove",
                    on_click = remove,
                    size = size::Sm,
                    variant = variant::Ghost,
                    tone = tone::Danger,
                    disabled = rx!(console.plan_count.get() <= 1),
                )
            }
            Field(
                value = slot.name,
                on_change = on_name,
                label = Some("Name".to_string()),
            )
            Textarea(
                value = slot.description,
                on_change = on_description,
                label = Some("Description".to_string()),
                rows = 2u32,
                max_rows = 8u32,
            )
            Textarea(
                value = slot.tasks,
                on_change = on_tasks,
                label = Some("Tasks".to_string()),
                placeholder = Some("One task per line".to_string()),
                rows = 3u32,
                max_rows = 14u32,
            )
            Textarea(
                value = slot.owns,
                on_change = on_owns,
                label = Some("Owns".to_string()),
                placeholder = Some("Path prefixes this module writes to, one per line".to_string()),
                rows = 1u32,
                max_rows = 6u32,
            )
            deps
        }
    }
}

/// Props for [`DepChip`].
#[derive(Default, IdealystSchema)]
pub struct DepChipProps {
    /// Console state handles.
    pub console: Console,
    /// The slot whose prerequisites these are.
    pub slot: usize,
    /// The slot this chip names.
    pub other: usize,
    /// Its label.
    pub label: String,
    /// Whether it is a prerequisite.
    pub selected: bool,
}

/// One prerequisite candidate.
#[component]
pub fn DepChip(props: &DepChipProps) -> Element {
    let console = props.console;
    let (slot, other) = (props.slot, props.other);
    let label = props.label.clone();
    let selected = props.selected;
    let on_select: Rc<dyn Fn()> = Rc::new(move || {
        console.plan_modules[slot].deps.update(|deps| {
            let mut next = deps.clone();
            match next.iter().position(|&d| d == other) {
                Some(at) => {
                    next.remove(at);
                }
                None => next.push(other),
            }
            next
        });
    });
    ui! {
        Chip(label = label, selected = selected, on_select = Some(on_select.clone()), tone = tone::Primary)
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
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
            padding_vertical: t.spacing.lg(),
            padding_horizontal: t.spacing.xl(),
            border_bottom_width: 1.0,
            border_color: t.color.border(),
            background: t.color.surface(),
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    pub ScreenScroll<IdeaThemeRef> {
        base(_t) {
            flex_grow: 1.0,
            min_height: 0,
        }
    }
}

stylesheet! {
    pub ScreenPad<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.lg(),
            padding: t.spacing.xl(),
            max_width: 860,
        }
    }
}

stylesheet! {
    pub FormCol<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.md(),
        }
    }
}

stylesheet! {
    pub SectionHead<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
        }
    }
}

stylesheet! {
    pub ModuleList<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.md(),
        }
    }
}

stylesheet! {
    pub SlotCard<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
            padding: t.spacing.lg(),
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.lg(),
            background: t.color.surface(),
        }
    }
}

stylesheet! {
    pub SlotHead<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
        }
    }
}

stylesheet! {
    pub DepsCol<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
        }
    }
}

stylesheet! {
    pub ChipRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            gap: t.spacing.xs(),
        }
    }
}

stylesheet! {
    pub RefusalLine<IdeaThemeRef> {
        base(t) {
            font_size: t.typography.body_sm_size(),
            color: t.intent.danger.fg(),
        }
    }
}
