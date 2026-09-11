//! Manual edits from the console: the action menus that open them, the
//! modal that hosts each form or confirm, and the runner that sends
//! them.
//!
//! One shape for every edit. A menu entry or button calls
//! [`choose`], which seeds the form buffers from the model and opens
//! the surface (`Console::edit`). The form binds its fields to those
//! buffers — they live on `Console`, so a tick rebuilding the surface
//! around the modal cannot discard typed text. Submit copies the edit
//! into `Console::action` and bumps `action_seq`; [`ActionRunner`] is a
//! hole keyed on that counter, so the request's scope is owned by the
//! request and torn down only by the next one, never by the button
//! that made it (UX_GUIDELINES rule 25).
//!
//! Every write goes to a server function that calls the same store
//! method an agent's tool would. Nothing here decides what is allowed:
//! a refused write comes back as the store's own message and is shown
//! in the form.

use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use idea_ui::{push_toast, size, tone, typography_kind, variant, Button, Chip, Field, IdeaThemeRef,
    Menu, MenuItem, MenuSeparator, Modal, Spacer, Textarea, Typography};
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide};
use runtime_core::{
    component, pressable, rx, spawn_then, stylesheet, switch, ui, AlignItems, Cursor, Element,
    FlexDirection, FlexWrap, FontWeight, IdealystSchema, IntoElement, JustifyContent,
    PressableHandle, Ref, StyleApplication,
};

use crate::model::{self, features, want_by_id, Status};
use crate::state::{Console, Edit};
use crate::styles::SectionLabel;

// ---------------------------------------------------------------------
// Action menus
// ---------------------------------------------------------------------

/// One entry of an action menu: what it says and the edit it opens.
#[derive(Clone, PartialEq, Default)]
pub struct MenuEntry {
    pub label: String,
    pub edit: Edit,
    /// A destructive act — separated from the rest, and red.
    pub danger: bool,
}

impl MenuEntry {
    pub fn new(label: &str, edit: Edit) -> MenuEntry {
        MenuEntry { label: label.to_string(), edit, danger: false }
    }

    pub fn danger(label: &str, edit: Edit) -> MenuEntry {
        MenuEntry { label: label.to_string(), edit, danger: true }
    }
}

/// Props for [`ActionMenu`].
#[derive(Default, IdealystSchema)]
pub struct ActionMenuProps {
    /// Console state handles.
    pub console: Console,
    /// The menu's own id, so its open state survives a rebuild of the
    /// surface it sits on.
    pub id: String,
    /// The entries, in order. Destructive ones go last and are
    /// separated from the rest.
    pub entries: Vec<MenuEntry>,
}

/// The `⋯` control: an anchored menu of the edits a surface offers.
///
/// Earns its place from the second entry (UX_GUIDELINES rule 15): every
/// surface that carries one has several verbs, and a row click already
/// opens the primary thing.
#[component]
pub fn ActionMenu(props: &ActionMenuProps) -> Element {
    let console = props.console;
    let id = props.id.clone();
    let entries = props.entries.clone();
    let trigger: Ref<PressableHandle> = Ref::new();

    let press_id = id.clone();
    let button = pressable(
        vec![ui! { text(style = KebabGlyph()) { "\u{22ef}" } }],
        move || console.toggle_menu(&press_id),
    )
    .bind(trigger)
    .with_style(StyleApplication::new(kebab_box_style()))
    .into_element();

    let open_id = id.clone();
    let dismiss_id = id;
    let menu = switch(
        move || console.menu_open.get().as_deref() == Some(open_id.as_str()),
        move |open: &bool| {
            if !*open {
                return ui! { view {} };
            }
            let plain: Vec<MenuEntry> = entries.iter().filter(|e| !e.danger).cloned().collect();
            let danger: Vec<MenuEntry> = entries.iter().filter(|e| e.danger).cloned().collect();
            let (n_plain, n_danger) = (plain.len(), danger.len());
            let separated = n_plain > 0 && n_danger > 0;
            let close_id = dismiss_id.clone();
            let dismiss: Rc<dyn Fn()> = Rc::new(move || {
                if console.menu_open.get().as_deref() == Some(close_id.as_str()) {
                    console.menu_open.set(None);
                }
            });
            ui! {
                Menu(
                    target = Some(AnchorTarget::from(trigger)),
                    on_dismiss = Some(dismiss.clone()),
                    side = ElementSide::Below,
                    align = ElementAlign::End,
                ) {
                    for i in 0..n_plain {
                        MenuRow(console = console, entry = plain[i].clone())
                    }
                    if separated {
                        MenuSeparator()
                    }
                    for i in 0..n_danger {
                        MenuRow(console = console, entry = danger[i].clone())
                    }
                }
            }
        },
    );

    ui! {
        view(style = KebabAnchor()) {
            button
            menu
        }
    }
}

/// Props for [`MenuRow`].
#[derive(Default, IdealystSchema)]
pub struct MenuRowProps {
    /// Console state handles.
    pub console: Console,
    /// The entry this row is.
    pub entry: MenuEntry,
}

/// One row of an action menu.
#[component]
pub fn MenuRow(props: &MenuRowProps) -> Element {
    let console = props.console;
    let label = props.entry.label.clone();
    let edit = props.entry.edit.clone();
    let danger = props.entry.danger;
    let on_select: Rc<dyn Fn()> = Rc::new(move || choose(console, edit.clone()));
    let leading: Option<Element> = danger.then(|| ui! { text(style = DangerMark()) { "\u{2022}" } });
    ui! {
        MenuItem(label = label, on_select = on_select, leading = leading)
    }
}

/// Start an edit: seed the form from what the model holds now, then
/// open its surface — or, for an edit with nothing to ask, send it.
pub fn choose(console: Console, edit: Edit) {
    console.menu_open.set(None);
    match &edit {
        Edit::RenameFeature { feature } => {
            let name = feature_name(feature);
            console.seed_form(&name, "", "", "", "");
        }
        Edit::DescribeFeature { feature } => {
            let description = with_feature(feature, |f| f.description.clone()).unwrap_or_default();
            console.seed_form("", &description, "", "", "");
        }
        Edit::RenameModule { feature, module } => {
            let name = with_module(feature, module, |m| m.name.clone()).unwrap_or_default();
            console.seed_form(&name, "", "", "", "");
        }
        Edit::EditModule { feature, module } => {
            let (description, owns) = with_module(feature, module, |m| {
                (m.description.clone(), m.owns.join("\n"))
            })
            .unwrap_or_default();
            console.seed_form("", &description, "", &owns, "");
        }
        Edit::EditWant { want } => {
            let (body, tags) = want_by_id(want)
                .map(|w| {
                    (
                        w.body.clone(),
                        w.tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" "),
                    )
                })
                .unwrap_or_default();
            console.seed_form("", &body, "", "", &tags);
        }
        _ => console.seed_form("", "", "", "", ""),
    }
    if edit.is_modal() {
        console.open_edit(edit);
    } else {
        console.submit(edit);
    }
}

// ---------------------------------------------------------------------
// The modal
// ---------------------------------------------------------------------

/// Props for [`EditHost`].
#[derive(Default, IdealystSchema)]
pub struct EditHostProps {
    /// Console state handles.
    pub console: Console,
}

/// The one modal every form and confirm renders in. Always mounted;
/// `Console::edit` opens and closes it. Its content is rebuilt per
/// open and keyed on the edit, so re-targeting from one surface to
/// another swaps the form while the modal stays up.
#[component]
pub fn EditHost(props: &EditHostProps) -> Element {
    let console = props.console;
    let on_dismiss: Rc<dyn Fn()> = Rc::new(move || console.close_edit());
    ui! {
        Modal(
            open = rx!(console.edit.get().is_modal()),
            on_dismiss = Some(on_dismiss.clone()),
            width = 520.0,
            content = move || form_body(console),
        )
    }
}

fn form_body(console: Console) -> Element {
    switch(
        move || console.edit.get(),
        move |edit: &Edit| match edit.clone() {
            Edit::RenameFeature { feature } => name_form(
                console,
                "Rename feature",
                "Name",
                "Save",
                Edit::RenameFeature { feature },
            ),
            Edit::DescribeFeature { feature } => text_form(
                console,
                "Describe feature",
                "Description",
                "What this feature is for, in a sentence or two.",
                "Save",
                Edit::DescribeFeature { feature },
            ),
            Edit::ShelveFeature { feature, shelve } => {
                let name = feature_name(&feature);
                confirm_form(
                    console,
                    if shelve { "Shelve feature" } else { "Unshelve feature" },
                    if shelve {
                        format!("'{name}' will be parked. Nothing in it can be claimed until it is unshelved.")
                    } else {
                        format!("'{name}' goes back to planning. Its ready modules can be claimed again.")
                    },
                    if shelve { "Shelve" } else { "Unshelve" },
                    false,
                    Edit::ShelveFeature { feature, shelve },
                )
            }
            Edit::DeleteFeature { feature } => {
                let name = feature_name(&feature);
                confirm_form(
                    console,
                    "Delete plan",
                    format!(
                        "'{name}' and every module in it will be removed. The wants it was \
                         composed from go back to the pool."
                    ),
                    "Delete plan",
                    true,
                    Edit::DeleteFeature { feature },
                )
            }
            Edit::AddModule { feature } => add_module_form(console, feature),
            Edit::RenameModule { feature, module } => name_form(
                console,
                "Rename module",
                "Name",
                "Save",
                Edit::RenameModule { feature, module },
            ),
            Edit::EditModule { feature, module } => edit_module_form(console, feature, module),
            Edit::RemoveModule { feature, module } => {
                let name = with_module(&feature, &module, |m| m.name.clone()).unwrap_or_default();
                confirm_form(
                    console,
                    "Remove module",
                    format!("'{name}' and its checklist will be removed from the plan. Modules that waited on it stop waiting."),
                    "Remove module",
                    true,
                    Edit::RemoveModule { feature, module },
                )
            }
            Edit::AddTask { feature, module } => name_form(
                console,
                "Add task",
                "Task",
                "Add task",
                Edit::AddTask { feature, module },
            ),
            Edit::RemoveTask { feature, task } => {
                let label = with_feature(&feature, |f| {
                    f.modules
                        .iter()
                        .flat_map(|m| m.tasks.iter())
                        .find(|t| t.id == task)
                        .map(|t| t.label.clone())
                })
                .flatten()
                .unwrap_or_default();
                confirm_form(
                    console,
                    "Remove task",
                    format!("'{label}' will be removed from the checklist."),
                    "Remove task",
                    true,
                    Edit::RemoveTask { feature, task },
                )
            }
            Edit::AddDependency { feature, module } => add_dependency_form(console, feature, module),
            Edit::RemoveDependency { feature, module, depends_on } => {
                let (a, b) = with_feature(&feature, |f| (f.module_name(&module), f.module_name(&depends_on)))
                    .unwrap_or_default();
                confirm_form(
                    console,
                    "Remove prerequisite",
                    format!("'{a}' will no longer wait on '{b}'."),
                    "Remove",
                    true,
                    Edit::RemoveDependency { feature, module, depends_on },
                )
            }
            Edit::EditWant { want } => edit_want_form(console, want),
            Edit::DeclineWant { want } => text_form(
                console,
                "Decline want",
                "Reason",
                "Why this idea is not going ahead.",
                "Decline",
                Edit::DeclineWant { want },
            ),
            Edit::DeleteWant { want } => {
                let body = want_by_id(&want).map(|w| w.body).unwrap_or_default();
                confirm_form(
                    console,
                    "Delete want",
                    format!("'{body}' will be removed from the pool."),
                    "Delete want",
                    true,
                    Edit::DeleteWant { want },
                )
            }
            Edit::None | Edit::CreatePlan | Edit::ReopenWant { .. } => ui! { view {} },
        },
    )
}

/// A form with one name field.
fn name_form(console: Console, title: &str, label: &str, verb: &str, edit: Edit) -> Element {
    let on_change: Rc<dyn Fn(String)> = Rc::new(move |v| console.form_name.set(v));
    let body: Element = ui! {
        view(style = FieldsCol()) {
            Field(
                value = console.form_name,
                on_change = on_change,
                label = Some(label.to_string()),
            )
        }
    };
    frame(console, title, body, verb, false, edit, move || {
        !console.form_name.get().trim().is_empty()
    })
}

/// A form with one longer text.
fn text_form(
    console: Console,
    title: &str,
    label: &str,
    placeholder: &str,
    verb: &str,
    edit: Edit,
) -> Element {
    let on_change: Rc<dyn Fn(String)> = Rc::new(move |v| console.form_text.set(v));
    let body: Element = ui! {
        view(style = FieldsCol()) {
            Textarea(
                value = console.form_text,
                on_change = on_change,
                label = Some(label.to_string()),
                placeholder = Some(placeholder.to_string()),
                rows = 3u32,
                max_rows = 10u32,
            )
        }
    };
    frame(console, title, body, verb, false, edit, move || {
        !console.form_text.get().trim().is_empty()
    })
}

/// A confirm: one sentence and the verb.
fn confirm_form(
    console: Console,
    title: &str,
    sentence: String,
    verb: &str,
    danger: bool,
    edit: Edit,
) -> Element {
    let body: Element = ui! { Typography(content = sentence, kind = typography_kind::Body) };
    frame(console, title, body, verb, danger, edit, || true)
}

fn add_module_form(console: Console, feature: String) -> Element {
    let on_name: Rc<dyn Fn(String)> = Rc::new(move |v| console.form_name.set(v));
    let on_text: Rc<dyn Fn(String)> = Rc::new(move |v| console.form_text.set(v));
    let on_lines: Rc<dyn Fn(String)> = Rc::new(move |v| console.form_lines.set(v));
    let on_owns: Rc<dyn Fn(String)> = Rc::new(move |v| console.form_owns.set(v));
    let candidates: Vec<(String, String)> =
        with_feature(&feature, |f| f.modules.iter().map(|m| (m.id.clone(), m.name.clone())).collect())
            .unwrap_or_default();
    let deps = pick_row(console, "Depends on", candidates, true);
    let body: Element = ui! {
        view(style = FieldsCol()) {
            Field(value = console.form_name, on_change = on_name, label = Some("Name".to_string()))
            Textarea(
                value = console.form_text,
                on_change = on_text,
                label = Some("Description".to_string()),
                rows = 2u32,
                max_rows = 8u32,
            )
            Textarea(
                value = console.form_lines,
                on_change = on_lines,
                label = Some("Tasks".to_string()),
                placeholder = Some("One task per line".to_string()),
                rows = 3u32,
                max_rows = 12u32,
            )
            Textarea(
                value = console.form_owns,
                on_change = on_owns,
                label = Some("Owns".to_string()),
                placeholder = Some("One path prefix per line, e.g. src/billing/".to_string()),
                rows = 2u32,
                max_rows = 6u32,
            )
            deps
        }
    };
    frame(console, "Add module", body, "Add module", false, Edit::AddModule { feature }, move || {
        !console.form_name.get().trim().is_empty()
    })
}

fn edit_module_form(console: Console, feature: String, module: String) -> Element {
    let on_text: Rc<dyn Fn(String)> = Rc::new(move |v| console.form_text.set(v));
    let on_owns: Rc<dyn Fn(String)> = Rc::new(move |v| console.form_owns.set(v));
    let body: Element = ui! {
        view(style = FieldsCol()) {
            Textarea(
                value = console.form_text,
                on_change = on_text,
                label = Some("Description".to_string()),
                rows = 3u32,
                max_rows = 10u32,
            )
            Textarea(
                value = console.form_owns,
                on_change = on_owns,
                label = Some("Owns".to_string()),
                placeholder = Some("One path prefix per line".to_string()),
                rows = 2u32,
                max_rows = 6u32,
            )
        }
    };
    frame(console, "Edit module", body, "Save", false, Edit::EditModule { feature, module }, || true)
}

fn add_dependency_form(console: Console, feature: String, module: String) -> Element {
    // Every other module that is not already a prerequisite.
    let candidates: Vec<(String, String)> = with_feature(&feature, |f| {
        let already: Vec<String> = f
            .module_index(&module)
            .map(|i| f.modules[i].depends_on.clone())
            .unwrap_or_default();
        f.modules
            .iter()
            .filter(|m| m.id != module && !already.contains(&m.id))
            .map(|m| (m.id.clone(), m.name.clone()))
            .collect()
    })
    .unwrap_or_default();
    let body = pick_row(console, "Waits on", candidates, false);
    frame(
        console,
        "Add prerequisite",
        body,
        "Add",
        false,
        Edit::AddDependency { feature, module },
        move || !console.form_pick.get().is_empty(),
    )
}

fn edit_want_form(console: Console, want: String) -> Element {
    let frozen = want_by_id(&want).is_some_and(|w| w.state == model::WantState::Promoted);
    let on_text: Rc<dyn Fn(String)> = Rc::new(move |v| console.form_text.set(v));
    let on_tags: Rc<dyn Fn(String)> = Rc::new(move |v| console.form_tags.set(v));
    // A composed want's wording is quoted by a plan, and the store
    // refuses to change it — so the field is not offered rather than
    // offered and refused.
    let body: Element = ui! {
        view(style = FieldsCol()) {
            if !frozen {
                Textarea(
                    value = console.form_text,
                    on_change = on_text,
                    label = Some("Want".to_string()),
                    rows = 2u32,
                    max_rows = 8u32,
                )
            }
            Field(
                value = console.form_tags,
                on_change = on_tags,
                label = Some("Tags".to_string()),
                placeholder = Some("#ux #billing".to_string()),
            )
        }
    };
    frame(console, "Edit want", body, "Save", false, Edit::EditWant { want }, move || {
        frozen || !console.form_text.get().trim().is_empty()
    })
}

/// A row of chips over `(id, label)` candidates writing into
/// `form_pick`. Multi-select for a new module's prerequisites; single
/// for adding one edge.
fn pick_row(
    console: Console,
    label: &str,
    candidates: Vec<(String, String)>,
    multi: bool,
) -> Element {
    let label = label.to_string();
    let count = candidates.len();
    let cands = Rc::new(candidates);
    switch(
        move || console.form_pick.get(),
        move |picked: &Vec<String>| {
            let picked = picked.clone();
            let cands = cands.clone();
            let label = label.clone();
            ui! {
                view(style = PickCol()) {
                    text(style = SectionLabel()) { label }
                    if count == 0 {
                        Typography(content = "No other modules.", kind = typography_kind::Caption, muted = true)
                    }
                    view(style = ChipRow()) {
                        for i in 0..count {
                            PickChip(
                                console = console,
                                id = cands[i].0.clone(),
                                label = cands[i].1.clone(),
                                selected = picked.contains(&cands[i].0),
                                multi = multi,
                            )
                        }
                    }
                }
            }
        },
    )
}

/// Props for [`PickChip`].
#[derive(Default, IdealystSchema)]
pub struct PickChipProps {
    /// Console state handles.
    pub console: Console,
    /// The candidate's id.
    pub id: String,
    /// Its name.
    pub label: String,
    /// Whether it is picked.
    pub selected: bool,
    /// Whether picking it keeps the others.
    pub multi: bool,
}

/// One candidate in a pick row.
#[component]
pub fn PickChip(props: &PickChipProps) -> Element {
    let console = props.console;
    let id = props.id.clone();
    let label = props.label.clone();
    let selected = props.selected;
    let multi = props.multi;
    let on_select: Rc<dyn Fn()> = Rc::new(move || {
        let id = id.clone();
        console.form_pick.update(|picked| {
            let mut next = if multi { picked.clone() } else { Vec::new() };
            match next.iter().position(|p| *p == id) {
                Some(at) => {
                    next.remove(at);
                }
                None => next.push(id.clone()),
            }
            next
        });
    });
    ui! {
        Chip(label = label, selected = selected, on_select = Some(on_select.clone()), tone = tone::Primary)
    }
}

/// The modal's frame: title, fields, the refusal line, and the footer
/// with Cancel and the verb. The primary button carries the busy state
/// and is disabled until `ready` says the form can be sent, so the
/// caveat rides the control (UX_GUIDELINES rule 16).
fn frame(
    console: Console,
    title: &str,
    body: Element,
    verb: &str,
    danger: bool,
    edit: Edit,
    ready: impl Fn() -> bool + 'static,
) -> Element {
    let title = title.to_string();
    let verb = verb.to_string();
    let cancel: Rc<dyn Fn()> = Rc::new(move || console.close_edit());
    let send: Rc<dyn Fn()> = Rc::new(move || console.submit(edit.clone()));
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
    let button: Element = if danger {
        ui! {
            Button(
                label = verb,
                on_click = send,
                tone = tone::Danger,
                loading = rx!(console.form_busy.get()),
                disabled = rx!(!ready()),
            )
        }
    } else {
        ui! {
            Button(
                label = verb,
                on_click = send,
                loading = rx!(console.form_busy.get()),
                disabled = rx!(!ready()),
            )
        }
    };
    ui! {
        view(style = FormCol()) {
            Typography(content = title, kind = typography_kind::H3, weight = Some(FontWeight::SemiBold))
            body
            refusal
            view(style = FooterRow()) {
                Spacer()
                Button(label = "Cancel", on_click = cancel, variant = variant::Ghost, size = size::Sm)
                button
            }
        }
    }
}

// ---------------------------------------------------------------------
// The runner
// ---------------------------------------------------------------------

/// Props for [`ActionRunner`].
#[derive(Default, IdealystSchema)]
pub struct ActionRunnerProps {
    /// Console state handles.
    pub console: Console,
}

type Pending = Pin<Box<dyn Future<Output = Result<api::WriteResult, server::ServerError>>>>;

/// The hole every submit runs in. Keyed on the request counter, so
/// the request owns its own scope and the button that pressed it may
/// come and go.
#[component]
pub fn ActionRunner(props: &ActionRunnerProps) -> Element {
    let console = props.console;
    switch(
        move || console.action_seq.get(),
        move |&seq: &u64| {
            if seq == 0 {
                return ui! { view {} };
            }
            // A switch build closure runs untracked, so this read
            // subscribes nothing.
            let edit = console.action.get();
            match request(console, &edit) {
                Ok(pending) => {
                    spawn_then(pending, move |result| finish(console, edit, result));
                }
                Err(why) => {
                    console.form_busy.set(false);
                    console.form_error.set(why);
                }
            }
            ui! { view {} }
        },
    )
}

/// Build the request an edit sends, from the form buffers. An `Err` is
/// a refusal the console can make on its own — an empty name, no
/// module — and never reaches the server.
fn request(console: Console, edit: &Edit) -> Result<Pending, String> {
    use api::PlanOpDto as Op;
    let name = console.form_name.get().trim().to_string();
    let text = console.form_text.get().trim().to_string();
    let lines = lines(&console.form_lines.get());
    let owns = lines_of(&console.form_owns.get());
    let pick = console.form_pick.get();
    let revise = |feature: &str, op: Op| -> Pending {
        Box::pin(api::revise_plan(feature.to_string(), vec![op]))
    };
    Ok(match edit.clone() {
        Edit::None => return Err("Nothing to send.".into()),
        Edit::CreatePlan => Box::pin(api::create_plan(plan_draft(console)?)),
        Edit::RenameFeature { feature } => revise(&feature, Op::Rename { id: feature.clone(), name }),
        Edit::DescribeFeature { feature } => {
            revise(&feature, Op::UpdateFeature { description: text })
        }
        Edit::ShelveFeature { feature, shelve } => Box::pin(api::shelve_plan(feature, shelve)),
        Edit::DeleteFeature { feature } => Box::pin(api::delete_plan(feature)),
        Edit::AddModule { feature } => revise(
            &feature,
            Op::AddModule { name, description: text, tasks: lines, depends_on: pick, owns },
        ),
        Edit::RenameModule { feature, module } => revise(&feature, Op::Rename { id: module, name }),
        Edit::EditModule { feature, module } => {
            revise(&feature, Op::UpdateModule { id: module, description: text, owns })
        }
        Edit::RemoveModule { feature, module } => revise(&feature, Op::Remove { id: module }),
        Edit::AddTask { feature, module } => {
            revise(&feature, Op::AddTask { module_id: module, name })
        }
        Edit::RemoveTask { feature, task } => revise(&feature, Op::Remove { id: task }),
        Edit::AddDependency { feature, module } => {
            let Some(depends_on) = pick.first().cloned() else {
                return Err("Pick the module to wait on.".into());
            };
            revise(&feature, Op::AddDependency { module_id: module, depends_on })
        }
        Edit::RemoveDependency { feature, module, depends_on } => {
            revise(&feature, Op::RemoveDependency { module_id: module, depends_on })
        }
        Edit::EditWant { want } => {
            Box::pin(api::edit_want(want, text, tags_of(&console.form_tags.get())))
        }
        Edit::DeclineWant { want } => {
            Box::pin(api::set_want_state(want, "declined".to_string(), text))
        }
        Edit::ReopenWant { want } => {
            Box::pin(api::set_want_state(want, "open".to_string(), String::new()))
        }
        Edit::DeleteWant { want } => Box::pin(api::delete_want(want)),
    })
}

/// The plan editor's buffers as a draft, or the reason they are not
/// one yet.
fn plan_draft(console: Console) -> Result<api::PlanDraft, String> {
    let name = console.plan_name.get().trim().to_string();
    if name.is_empty() {
        return Err("The plan needs a name.".into());
    }
    let count = console.plan_count.get();
    let slots = &console.plan_modules[..count];
    let names: Vec<String> = slots.iter().map(|s| s.name.get().trim().to_string()).collect();
    let mut modules = Vec::new();
    for (i, slot) in slots.iter().enumerate() {
        if names[i].is_empty() {
            return Err(format!("Module {} needs a name.", i + 1));
        }
        modules.push(api::ModuleDraft {
            name: names[i].clone(),
            description: slot.description.get().trim().to_string(),
            tasks: lines(&slot.tasks.get()),
            depends_on: slot
                .deps
                .get()
                .iter()
                .filter_map(|&d| names.get(d).filter(|n| !n.is_empty()).cloned())
                .collect(),
            owns: lines_of(&slot.owns.get()),
        });
    }
    Ok(api::PlanDraft {
        name,
        description: console.plan_description.get().trim().to_string(),
        whitepaper: console.plan_whitepaper.get().trim().to_string(),
        modules,
    })
}

/// What happens when the server answers.
fn finish(console: Console, edit: Edit, result: Result<api::WriteResult, server::ServerError>) {
    console.form_busy.set(false);
    match result {
        Ok(done) => {
            push_toast(done.message, tone::Success);
            console.close_edit();
            // The board, and the feature the edit touched, are stale.
            console.refresh.update(|n| n + 1);
            match edit {
                Edit::CreatePlan => {
                    console.reset_plan_draft();
                    console.open_after.set(Some(done.id));
                    console.show_features();
                }
                Edit::DeleteFeature { .. } => console.show_features(),
                Edit::DeleteWant { .. } | Edit::RemoveModule { .. } => console.close_drawer(),
                _ => {}
            }
        }
        Err(err) => {
            let why = refusal(&err);
            if edit.is_modal() || edit == Edit::CreatePlan {
                console.form_error.set(why);
            } else {
                push_toast(why, tone::Danger);
            }
        }
    }
}

/// The store's own words for a refused write, without the transport's
/// framing around them.
fn refusal(err: &server::ServerError) -> String {
    match err {
        server::ServerError::Failed(message) => message.clone(),
        server::ServerError::Server { message, .. } => message.clone(),
        other => other.to_string(),
    }
}

/// Non-empty trimmed lines.
fn lines(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
}

/// Path prefixes: one per line, or several per line separated by
/// commas — either way people write them.
fn lines_of(text: &str) -> Vec<String> {
    text.split(|c: char| c == '\n' || c == ',')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// `#ux #billing` (or `ux, billing`) as a tag list. Normalization —
/// case, punctuation — is the store's, at write time.
fn tags_of(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.split(|c: char| c.is_whitespace() || c == ',') {
        let tag = raw.trim().trim_start_matches('#').trim();
        if !tag.is_empty() && !out.iter().any(|t| t == tag) {
            out.push(tag.to_string());
        }
    }
    out
}

// ---------------------------------------------------------------------
// Model lookups
// ---------------------------------------------------------------------

fn with_feature<T>(feature: &str, f: impl FnOnce(&model::Feature) -> T) -> Option<T> {
    let feats = features();
    feats.iter().find(|x| x.id == feature).map(f)
}

fn with_module<T>(feature: &str, module: &str, f: impl FnOnce(&model::Module) -> T) -> Option<T> {
    with_feature(feature, |feat| feat.modules.iter().find(|m| m.id == module).map(f)).flatten()
}

fn feature_name(feature: &str) -> String {
    with_feature(feature, |f| f.name.clone()).unwrap_or_default()
}

/// The entries a feature's own menu offers. Shelving is offered while
/// the feature is not complete; deletion always — the store says no
/// when work has happened, in its own words.
pub fn feature_entries(feature: &str) -> Vec<MenuEntry> {
    let (status, shelved) = with_feature(feature, |f| (f.status, f.shelved)).unwrap_or_default();
    let id = feature.to_string();
    let mut entries = vec![
        MenuEntry::new("Rename", Edit::RenameFeature { feature: id.clone() }),
        MenuEntry::new("Edit description", Edit::DescribeFeature { feature: id.clone() }),
        MenuEntry::new("Add module", Edit::AddModule { feature: id.clone() }),
    ];
    if status != Status::Done {
        entries.push(MenuEntry::new(
            if shelved { "Unshelve" } else { "Shelve" },
            Edit::ShelveFeature { feature: id.clone(), shelve: !shelved },
        ));
    }
    entries.push(MenuEntry::danger("Delete plan", Edit::DeleteFeature { feature: id }));
    entries
}

/// The entries a module's drawer offers.
pub fn module_entries(feature: &str, module: &str) -> Vec<MenuEntry> {
    let (f, m) = (feature.to_string(), module.to_string());
    vec![
        MenuEntry::new("Rename", Edit::RenameModule { feature: f.clone(), module: m.clone() }),
        MenuEntry::new("Edit description & owns", Edit::EditModule { feature: f.clone(), module: m.clone() }),
        MenuEntry::new("Add task", Edit::AddTask { feature: f.clone(), module: m.clone() }),
        MenuEntry::new("Add prerequisite", Edit::AddDependency { feature: f.clone(), module: m.clone() }),
        MenuEntry::danger("Remove module", Edit::RemoveModule { feature: f, module: m }),
    ]
}

/// The entries a want's drawer offers. A composed want keeps its
/// wording and cannot be declined or deleted, so only retagging is
/// offered on it.
pub fn want_entries(want: &model::Want) -> Vec<MenuEntry> {
    let id = want.id.clone();
    match want.state {
        model::WantState::Promoted => vec![MenuEntry::new("Edit tags", Edit::EditWant { want: id })],
        model::WantState::Declined => vec![
            MenuEntry::new("Edit", Edit::EditWant { want: id.clone() }),
            MenuEntry::new("Reopen", Edit::ReopenWant { want: id.clone() }),
            MenuEntry::danger("Delete", Edit::DeleteWant { want: id }),
        ],
        model::WantState::Open => vec![
            MenuEntry::new("Edit", Edit::EditWant { want: id.clone() }),
            MenuEntry::new("Decline\u{2026}", Edit::DeclineWant { want: id.clone() }),
            MenuEntry::danger("Delete", Edit::DeleteWant { want: id }),
        ],
    }
}

// ---------------------------------------------------------------------
// Styles
// ---------------------------------------------------------------------

stylesheet! {
    pub KebabAnchor<IdeaThemeRef> {
        base(_t) {
            position: runtime_core::Position::Relative,
            flex_direction: FlexDirection::Row,
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    pub KebabBox<IdeaThemeRef> {
        base(t) {
            width: 30,
            height: 30,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            border_radius: t.radius.md(),
            cursor: Cursor::Pointer,
        }
        transitions {
            background: 120ms EaseOut,
        }
        state hovered(t) {
            background: t.color.surface_alt(),
        }
    }
}

stylesheet! {
    pub KebabGlyph<IdeaThemeRef> {
        base(t) {
            font_size: 18,
            color: t.color.text_muted(),
        }
    }
}

stylesheet! {
    pub DangerMark<IdeaThemeRef> {
        base(t) {
            color: t.intent.danger.fg(),
            font_size: 14,
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
    pub FieldsCol<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.md(),
        }
    }
}

stylesheet! {
    pub FooterRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
            padding_top: t.spacing.sm(),
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

stylesheet! {
    pub PickCol<IdeaThemeRef> {
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
