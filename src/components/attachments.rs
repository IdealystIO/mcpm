//! Files attached to a feature or a want: the Files tab on a feature's
//! board, the Files section in a want's drawer, and the attach form.
//!
//! One row shape for both places, so a file reads the same wherever
//! it is met: its name, the description written for whoever reads it,
//! and one muted line of size, type, who and when. A row press opens
//! the file — through the host's download route, which carries the
//! console's key and 302s to a short-lived link — and the row's menu
//! carries the two edits (describe, remove).
//!
//! The bytes never come through a server function: a file is not a
//! JSON argument. The attach form holds the picked file on `Console`
//! (`form_file`) and the runner in `edits` POSTs it as multipart to
//! `api::upload_path` — the same hole every other write runs in, so
//! the request's scope is owned by the request, not by the button.

use std::rc::Rc;

use idea_ui::{size, tone, typography_kind, variant, Button, IdeaThemeRef, Spacer, Tag, Textarea,
    Typography};
use runtime_core::{
    component, rx, spawn_then, stylesheet, switch, ui, AlignItems, Element,
    FlexDirection, FontWeight, IdealystSchema, IntoElement, StyleApplication,
};

use crate::components::edits::{choose, ActionMenu, MenuEntry};
use crate::model::{self, want_by_id, Attachment};
use crate::state::{Console, Edit, PickedBlob};
use crate::styles::SectionLabel;
use crate::components::bits::tappable;

// ---------------------------------------------------------------------
// The feature's Files tab
// ---------------------------------------------------------------------

/// Props for [`FilesView`].
#[derive(Default, IdealystSchema)]
pub struct FilesViewProps {
    /// Console state handles.
    pub console: Console,
    /// Feature index.
    pub feature: usize,
}

/// A feature's files: its own, then the files of the wants it was
/// composed from (each marked with the idea it came in through).
///
/// The list is keyed on the files themselves: remade when one is
/// attached, described or removed, and left alone by everything else.
#[component]
pub fn FilesView(props: &FilesViewProps) -> Element {
    let console = props.console;
    let data = console.data;
    let fi = props.feature;
    let subject = data.feature(fi);
    let attach: Rc<dyn Fn()> = Rc::new(move || {
        choose(console, Edit::AttachFile { subject: subject().map(|f| f.id.clone()).unwrap_or_default() });
    });
    let list = switch(
        move || data.features.get().get(fi).map(|f| (f.id.clone(), f.attachments.clone())),
        move |held: &Option<(String, Rc<Vec<Attachment>>)>| {
            let (subject, files) = held.clone().unwrap_or_default();
            let count = files.len();
            let has_files = count > 0;
            ui! {
                view(style = FilesCol()) {
                    if has_files {
                        view(style = FilesCard()) {
                            for i in 0..count {
                                AttachmentRow(
                                    console = console,
                                    subject = subject.clone(),
                                    attachment = files[i].clone(),
                                    first = i == 0,
                                )
                            }
                        }
                    }
                    if !has_files {
                        Typography(content = "No files attached.", kind = typography_kind::BodySm, muted = true)
                    }
                }
            }
        },
    );

    ui! {
        scroll_view(style = FilesScroll()) {
            view(style = FilesPad()) {
                view(style = FilesCol()) {
                    view(style = FilesBar()) {
                        Spacer()
                        Button(label = "Attach file", on_click = attach, size = size::Sm)
                    }
                    list
                }
            }
        }
    }
}

// ---------------------------------------------------------------------
// The want drawer's Files section
// ---------------------------------------------------------------------

/// Props for [`FilesSection`].
#[derive(Default, IdealystSchema)]
pub struct FilesSectionProps {
    /// Console state handles.
    pub console: Console,
    /// The want's id.
    pub want: String,
}

/// The files of one want, inside its drawer. The attach verb lives in
/// the drawer's own menu with the want's other edits; this is the list.
#[component]
pub fn FilesSection(props: &FilesSectionProps) -> Element {
    let console = props.console;
    let want = props.want.clone();
    let files: Vec<Attachment> = want_by_id(&want).map(|w| w.attachments).unwrap_or_default();
    let count = files.len();
    let has_files = count > 0;
    let files = Rc::new(files);
    ui! {
        view(style = SectionCol()) {
            text(style = SectionLabel()) { "Files" }
            if !has_files {
                Typography(content = "None.", kind = typography_kind::BodySm, muted = true)
            }
            if has_files {
                view(style = FilesList()) {
                    for i in 0..count {
                        AttachmentRow(
                            console = console,
                            subject = want.clone(),
                            attachment = files[i].clone(),
                            first = i == 0,
                        )
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------
// One row
// ---------------------------------------------------------------------

/// Props for [`AttachmentRow`].
#[derive(Default, IdealystSchema)]
pub struct AttachmentRowProps {
    /// Console state handles.
    pub console: Console,
    /// The subject the list belongs to — the feature or the want —
    /// which is what a refetch after an edit is keyed on.
    pub subject: String,
    /// The file.
    pub attachment: Attachment,
    /// Whether this is the first row of its card, which carries no
    /// rule above it.
    pub first: bool,
    /// Rendered inside the comment it arrived with, where saying so
    /// again would be noise.
    pub in_comment: bool,
}

/// One file: name, the description written for its reader, and a
/// muted meta line. Pressing the row opens the file; its menu carries
/// the edits.
#[component]
pub fn AttachmentRow(props: &AttachmentRowProps) -> Element {
    let console = props.console;
    let a = props.attachment.clone();
    let subject = props.subject.clone();
    let id = a.id.clone();
    let name = a.name.clone();
    let description = a.description.clone();
    let has_description = !description.is_empty();
    let meta = a.meta();
    // Where the file came in, when not straight onto this subject: the
    // idea it rode, the module it sits on, the comment it arrived with.
    let mut origins: Vec<String> = Vec::new();
    if !a.via_want.is_empty() {
        origins.push(format!("via: {}", truncate(&a.via_want, 48)));
    }
    if !a.via_module.is_empty() {
        origins.push(format!("in: {}", a.via_module));
    }
    if !a.comment_author.is_empty() && !props.in_comment {
        origins.push(format!("with a comment by {}", a.comment_author));
    }
    let has_via = !origins.is_empty();
    let origin_count = origins.len();
    let origins = Rc::new(origins);
    let first = if props.first { "yes" } else { "no" };

    // A file that came in through a want is the want's to edit: its
    // row on the feature's list opens the file and nothing else, and
    // the edits are on the want's own drawer.
    let own = a.via_want_id.is_empty();
    let entries = if own {
        vec![
            MenuEntry::new(
                "Edit description",
                Edit::DescribeAttachment { subject: subject.clone(), attachment: id.clone() },
            ),
            MenuEntry::danger(
                "Remove",
                Edit::RemoveAttachment { subject: subject.clone(), attachment: id.clone() },
            ),
        ]
    } else {
        Vec::new()
    };
    let menu_id = format!("att:{id}");
    let menu: Option<Element> =
        own.then(|| ui! { ActionMenu(console = console, id = menu_id.clone(), entries = entries.clone()) });

    let open_id = id.clone();
    let body: Element = ui! {
        view(style = RowInner()) {
            view(style = RowText()) {
                Typography(content = name, kind = typography_kind::BodySm, weight = Some(FontWeight::SemiBold))
                if has_description {
                    Typography(content = description, kind = typography_kind::BodySm)
                }
                Typography(content = meta, kind = typography_kind::Caption, muted = true)
                if has_via {
                    view(style = ViaRow()) {
                        for i in 0..origin_count {
                            Tag(label = origins[i].clone(), tone = tone::Neutral, variant = variant::Soft)
                        }
                    }
                }
            }
        }
    };
    let row = tappable(vec![body], move || open_attachment(console, &open_id))
        .with_style(StyleApplication::new(row_box_style()).with("first", first.to_string()))
        .into_element();

    ui! {
        view(style = RowShell()) {
            row
            if let Some(m) = menu {
                view(style = RowMenuSlot()) { m }
            }
        }
    }
}

/// Open a file in a new tab. Synchronous on purpose: a browser only
/// lets a page open a window inside the click that asked for it, so
/// the URL cannot wait on a round trip. The host's download route
/// does the round trip instead (a 302 to a short-lived link), and the
/// console's key rides its query string the way the event socket's
/// does — the same trade, made once already.
pub fn open_attachment(console: Console, id: &str) {
    let key = console.api_key.get();
    let url = format!("{}{}", crate::app::API_ORIGIN, api::download_path(id, &key));
    runtime_core::open_url(&url);
}

fn truncate(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push('\u{2026}');
    }
    out
}

// ---------------------------------------------------------------------
// The attach form
// ---------------------------------------------------------------------

/// The body of the attach form: the file (picked through the
/// platform's open dialog) and the description written for whoever
/// reads it. The frame — title, verb, refusal line — is `edits`'s.
///
/// The pick runs from the button's own handler. That is the one
/// exception rule 25 allows here: the browser opens its file dialog
/// only inside the click that asked for it, so the call cannot be
/// deferred to a hole — and nothing the pick writes (`form_picking`,
/// `form_file`) rebuilds the button, which reads them as live props.
pub fn attach_form_body(console: Console) -> Element {
    let on_text: Rc<dyn Fn(String)> = Rc::new(move |v| console.form_text.set(v));
    let pick: Rc<dyn Fn()> = Rc::new(move || {
        if console.form_picking.get() {
            return;
        }
        console.form_picking.set(true);
        spawn_then(pick_one(), move |picked| {
            console.form_picking.set(false);
            match picked {
                Ok(Some(blob)) => {
                    console.form_error.set(String::new());
                    console.form_file.set(Some(blob));
                }
                Ok(None) => {}
                Err(why) => console.form_error.set(why),
            }
        });
    });
    let picked = switch(
        move || console.form_file.get(),
        move |file: &Option<PickedBlob>| match file {
            Some(f) => {
                let line = format!("{} \u{b7} {}", f.name, api::size_label(f.bytes.len() as i64));
                ui! { Typography(content = line, kind = typography_kind::BodySm) }
            }
            None => ui! {
                Typography(content = "No file chosen.", kind = typography_kind::BodySm, muted = true)
            },
        },
    );
    ui! {
        view(style = FormCol()) {
            view(style = PickRow()) {
                Button(
                    label = rx!(if console.form_file.get().is_some() {
                        "Change file".to_string()
                    } else {
                        "Choose file".to_string()
                    }),
                    on_click = pick,
                    variant = variant::Outlined,
                    size = size::Sm,
                    loading = rx!(console.form_picking.get()),
                )
                picked
            }
            Textarea(
                value = console.form_text,
                on_change = on_text,
                label = Some("Description".to_string()),
                placeholder = Some(
                    "What this file is and what a reader should take from it. Agents read this \
                     before opening the file."
                        .to_string(),
                ),
                rows = 3u32,
                max_rows = 10u32,
            )
        }
    }
}

/// Present the open dialog and read the pick into memory. `Ok(None)`
/// is a dismissal. The whole file is read here because the request
/// that sends it is built later, elsewhere, and the picker's handle
/// cannot travel; the store's size cap bounds what that costs.
async fn pick_one() -> Result<Option<PickedBlob>, String> {
    use file_picker::{FilePicker, PickOutcome, PickRequest};
    let outcome = FilePicker::new()
        .pick(PickRequest::documents(Vec::<String>::new()))
        .await
        .map_err(|e| format!("Could not open the file picker: {e}"))?;
    let PickOutcome::Picked(files) = outcome else {
        return Ok(None);
    };
    let Some(file) = files.into_iter().next() else {
        return Ok(None);
    };
    let bytes = file.read_all().await.map_err(|e| format!("Could not read the file: {e}"))?;
    Ok(Some(PickedBlob {
        name: file.name().to_string(),
        content_type: file.mime().to_string(),
        bytes: Rc::new(bytes),
    }))
}

/// The multipart body the upload route reads: a `description` part and
/// a `file` part. Built by hand — three parts and a boundary is not a
/// library — and the boundary carries the file's own hash-free
/// randomness from the size and name, which is enough because the body
/// is ours and a collision only breaks our own upload.
pub fn multipart(blob: &PickedBlob, description: &str) -> (String, Vec<u8>) {
    let boundary = format!(
        "----mcpm-{}-{}",
        blob.bytes.len(),
        runtime_core::time::now_micros()
    );
    let name = blob.name.replace('"', "_");
    let content_type = if blob.content_type.is_empty() {
        "application/octet-stream"
    } else {
        blob.content_type.as_str()
    };
    let mut body: Vec<u8> = Vec::with_capacity(blob.bytes.len() + 512);
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"description\"\r\n\r\n",
    );
    body.extend_from_slice(description.as_bytes());
    body.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\n\
             Content-Type: {content_type}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&blob.bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

/// The name of an attachment, for a confirm sentence.
pub fn attachment_name(id: &str) -> String {
    model::attachment_by_id(id).map(|a| a.name).unwrap_or_default()
}

// ---------------------------------------------------------------------
// Styles
// ---------------------------------------------------------------------

stylesheet! {
    pub FilesScroll<IdeaThemeRef> {
        base(_t) {
            flex_grow: 1.0,
            min_height: 0,
        }
    }
}

stylesheet! {
    pub FilesPad<IdeaThemeRef> {
        base(t) {
            padding: t.spacing.xl(),
        }
    }
}

stylesheet! {
    pub FilesCol<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.md(),
            max_width: 760,
            min_width: 0,
        }
    }
}

stylesheet! {
    pub FilesBar<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
        }
    }
}

// Rows share one card surface (rule 13) and are separated by rules.
stylesheet! {
    pub FilesCard<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            padding_vertical: t.spacing.xs(),
            padding_horizontal: t.spacing.lg(),
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.md(),
            background: t.color.surface(),
            min_width: 0,
        }
    }
}

// In the drawer the panel is already the surface, so the rows sit on
// it bare, separated by their rules — the drawer's other sections do
// the same, and a bordered box there would be a card in a card.
stylesheet! {
    pub FilesList<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            min_width: 0,
        }
    }
}

stylesheet! {
    pub SectionCol<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
        }
    }
}

// The row is a pressable and its menu is a sibling, not a child: a
// menu inside the pressable would open the file on every menu press.
stylesheet! {
    pub RowShell<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexStart,
            gap: t.spacing.sm(),
            min_width: 0,
        }
    }
}

stylesheet! {
    pub RowBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_width: 0,
            border_color: t.color.border(),
            cursor: runtime_core::Cursor::Pointer,
        }
        variant first {
            #[default]
            no(_t) { border_top_width: 1.0 }
            yes(_t) { border_top_width: 0.0 }
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
    pub RowInner<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.md(),
            padding_vertical: t.spacing.sm(),
            min_width: 0,
        }
    }
}

// The flexible side of the row (rule 22).
stylesheet! {
    pub RowText<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 2,
            min_width: 0,
            flex_grow: 1.0,
            flex_shrink: 1.0,
            overflow: runtime_core::Overflow::Hidden,
        }
    }
}

stylesheet! {
    pub RowMenuSlot<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            flex_shrink: 0.0,
            padding_top: t.spacing.xs(),
        }
    }
}

stylesheet! {
    pub ViaRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: runtime_core::FlexWrap::Wrap,
            gap: t.spacing.xs(),
            padding_top: t.spacing.xs(),
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
    pub PickRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.md(),
            min_width: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{attach_form_body, multipart, AttachmentRow};
    use crate::model::Attachment;
    use crate::state::{use_console, PickedBlob};
    use std::rc::Rc;

    /// The row and the attach form mount through the real `realize`
    /// against a mock host with the app's own registrations — the
    /// same check the want screens make, for the same reason: a
    /// payload with no handler fails at mount, not at compile.
    #[test]
    fn the_file_surfaces_mount_with_the_apps_registrations() {
        let harness = host_mock::Harness::with_registry(crate::register_scene_extensions);
        let tree = harness.world.enter(|| {
            idea_ui::install_idea_theme(idea_ui::light_theme());
            let console = use_console();
            let own = Attachment {
                id: "att_1".into(),
                name: "spec.md".into(),
                description: "The spec.".into(),
                content_type: "text/markdown".into(),
                size: "1.2 KB".into(),
                added_by: "console".into(),
                added: "Sep 14 10:00".into(),
                ..Default::default()
            };
            let via = Attachment {
                id: "att_2".into(),
                name: "old-report.pdf".into(),
                via_want_id: "want_1".into(),
                via_want: "the export should look like the old report".into(),
                ..Default::default()
            };
            let form = attach_form_body(console);
            runtime_core::ui! {
                view {
                    AttachmentRow(console = console, subject = "feat_1".to_string(), attachment = own, first = true)
                    AttachmentRow(console = console, subject = "feat_1".to_string(), attachment = via, first = false)
                    form
                }
            }
        });
        harness.mount(tree);
        harness.flush();
    }

    /// The body the upload route parses: two parts, the file's bytes
    /// verbatim, and the boundary the content-type names.
    #[test]
    fn multipart_body_frames_both_parts() {
        let blob = PickedBlob {
            name: "a \"b\".csv".into(),
            content_type: String::new(),
            bytes: Rc::new(b"x,y\r\n1,2".to_vec()),
        };
        let (content_type, body) = multipart(&blob, "the sample");
        let boundary = content_type.strip_prefix("multipart/form-data; boundary=").expect("boundary");
        let text = String::from_utf8_lossy(&body);
        assert!(text.starts_with(&format!("--{boundary}\r\n")));
        assert!(text.contains("name=\"description\"\r\n\r\nthe sample\r\n"));
        assert!(text.contains("name=\"file\"; filename=\"a _b_.csv\"\r\nContent-Type: application/octet-stream\r\n\r\nx,y\r\n1,2\r\n"));
        assert!(text.ends_with(&format!("--{boundary}--\r\n")));
    }
}
