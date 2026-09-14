//! The discussion on a feature, a want or a module: the comments so
//! far, and the composer that adds to them.
//!
//! One surface for all three places — the feature's Discussion tab and
//! the two drawers — so a thread reads the same wherever it is met. A
//! comment is a note, a question (which names who owes the answer and
//! holds the work until it lands), or an answer. Open questions carry
//! their own *Answer* control; the composer then replies to that
//! question rather than posting a fresh note.
//!
//! The composer's buffers live on `Console` (`comment_*`) and are
//! aimed at one subject at a time by the sync loop, so a tick that
//! rebuilds the thread cannot discard a half-written reply and a
//! draft never leaks from one subject to another. Posting runs through
//! the same `ActionRunner` hole every other write does.

use std::rc::Rc;

use idea_ui::{size, tone, typography_kind, variant, Badge, Button, Chip, Field, IdeaThemeRef,
    Spacer, Textarea, Typography};
use markdown::Markdown;
use runtime_core::{
    component, rx, spawn_then, stylesheet, switch, ui, AlignItems, Element, FlexDirection,
    FlexWrap, IdealystSchema,
};

use crate::components::attachments::AttachmentRow;
use crate::components::bits::Mono;
use crate::components::document::md_theme;
use crate::components::edits::{ActionMenu, MenuEntry};
use crate::model::{self, thread, Comment};
use crate::state::{Console, Edit, PickedBlob};
use crate::styles::{MonoTextSize, SectionLabel};

/// Props for [`Discussion`].
#[derive(Default, IdealystSchema)]
pub struct DiscussionProps {
    /// Console state handles.
    pub console: Console,
    /// The feature, want or module the thread belongs to.
    pub subject: String,
    /// Inside a drawer: a section with its own label, no outer pad.
    pub compact: bool,
}

/// The thread and its composer.
#[component]
pub fn Discussion(props: &DiscussionProps) -> Element {
    let console = props.console;
    let subject = props.subject.clone();
    let compact = props.compact;
    let loaded = model::has_thread(&subject);
    let comments: Rc<Vec<Comment>> = thread(&subject).unwrap_or_default();
    let count = comments.len();
    let has_comments = count > 0;
    let composer_subject = subject.clone();
    ui! {
        view(style = ThreadCol()) {
            if compact {
                text(style = SectionLabel()) { "Discussion" }
            }
            if !loaded {
                Typography(content = "Loading\u{2026}", kind = typography_kind::BodySm, muted = true)
            }
            if loaded && !has_comments {
                Typography(content = "Nothing said yet.", kind = typography_kind::BodySm, muted = true)
            }
            if has_comments {
                view(style = ThreadList()) {
                    for i in 0..count {
                        CommentRow(console = console, subject = subject.clone(), comment = comments[i].clone(), first = i == 0)
                    }
                }
            }
            Composer(console = console, subject = composer_subject.clone())
        }
    }
}

/// Props for [`CommentRow`].
#[derive(Default, IdealystSchema)]
pub struct CommentRowProps {
    /// Console state handles.
    pub console: Console,
    /// The thread's subject.
    pub subject: String,
    /// The comment.
    pub comment: Comment,
    /// First of its list — no rule above it.
    pub first: bool,
}

/// One comment: who, when, what kind, the body, its files, and — on
/// an open question — the way to answer it.
#[component]
pub fn CommentRow(props: &CommentRowProps) -> Element {
    let console = props.console;
    let c = props.comment.clone();
    let subject = props.subject.clone();
    let id = c.id.clone();
    let author = c.author.clone();
    let posted = if c.edited { format!("{} \u{b7} edited", c.posted) } else { c.posted.clone() };
    let body = c.body.clone();
    let has_body = !body.trim().is_empty();
    let is_question = c.kind == "question";
    let is_answer = c.kind == "answer";
    let open = is_question && !c.resolved;
    let owed = if c.assigned_to.is_empty() { "anyone".to_string() } else { c.assigned_to.clone() };
    let status_line = if open {
        format!("Open \u{2014} owed by {owed}")
    } else if is_question {
        format!("Answered by {}", c.resolved_by)
    } else {
        String::new()
    };
    let has_status = !status_line.is_empty();
    let files = Rc::new(c.attachments.clone());
    let file_count = files.len();
    let has_files = file_count > 0;
    let theme = rx!(md_theme(console.dark.get()));

    // The author's own comments carry their edits. An answer, and an
    // answered question, are history — the store says so too; the
    // menu simply does not offer it.
    let own = author == model::you();
    let mut entries: Vec<MenuEntry> = Vec::new();
    if own && !is_answer {
        entries.push(MenuEntry::new(
            "Edit",
            Edit::EditComment { subject: subject.clone(), comment: id.clone() },
        ));
        if !is_question || open {
            entries.push(MenuEntry::danger(
                if is_question { "Withdraw question" } else { "Delete" },
                Edit::DeleteComment { subject: subject.clone(), comment: id.clone(), question: is_question },
            ));
        }
    }
    let has_menu = !entries.is_empty();
    let menu_id = format!("cmt:{id}");

    let answer_id = id.clone();
    let answer: Rc<dyn Fn()> = Rc::new(move || {
        console.comment_answering.set(Some(answer_id.clone()));
        console.comment_kind.set("note".to_string());
    });

    let row_style = if is_question {
        RowBox().kind(RowBoxKind::Question)
    } else if is_answer {
        RowBox().kind(RowBoxKind::Answer)
    } else {
        RowBox().kind(RowBoxKind::Note)
    };

    ui! {
        view(style = row_style.first(if props.first { RowBoxFirst::Yes } else { RowBoxFirst::No })) {
            view(style = HeadRow()) {
                Mono(content = author, size = MonoTextSize::Caption)
                if open {
                    Badge(label = "question", tone = tone::Warning)
                }
                if is_question && !open {
                    Badge(label = "answered", tone = tone::Success)
                }
                if is_answer {
                    Badge(label = "answer", tone = tone::Info)
                }
                Typography(content = posted, kind = typography_kind::Caption, muted = true)
                Spacer()
                if has_menu {
                    ActionMenu(console = console, id = menu_id.clone(), entries = entries.clone())
                }
            }
            if has_body {
                view(style = BodyBox()) {
                    Markdown(source = body, theme = theme)
                }
            }
            if has_files {
                view(style = FilesList()) {
                    for i in 0..file_count {
                        AttachmentRow(console = console, subject = subject.clone(), attachment = files[i].clone(), first = i == 0, in_comment = true)
                    }
                }
            }
            if has_status {
                view(style = StatusRow()) {
                    Typography(content = status_line, kind = typography_kind::Caption, muted = true)
                    if open {
                        Button(label = "Answer", on_click = answer, size = size::Sm, variant = variant::Outlined)
                    }
                }
            }
        }
    }
}

/// Props for [`Composer`].
#[derive(Default, IdealystSchema)]
pub struct ComposerProps {
    /// Console state handles.
    pub console: Console,
    /// The subject a post goes to.
    pub subject: String,
}

/// Note or question, with files; or an answer to the question the
/// reader pressed *Answer* on.
#[component]
pub fn Composer(props: &ComposerProps) -> Element {
    let console = props.console;
    let subject = props.subject.clone();
    let on_body: Rc<dyn Fn(String)> = Rc::new(move |v| console.comment_draft.set(v));
    let on_assignee: Rc<dyn Fn(String)> = Rc::new(move |v| console.comment_assignee.set(v));
    let note: Rc<dyn Fn()> = Rc::new(move || console.comment_kind.set("note".to_string()));
    let question: Rc<dyn Fn()> = Rc::new(move || console.comment_kind.set("question".to_string()));
    let cancel_answer: Rc<dyn Fn()> = Rc::new(move || console.comment_answering.set(None));
    let post_subject = subject.clone();
    let post: Rc<dyn Fn()> = Rc::new(move || console.post_comment(&post_subject));
    let pick: Rc<dyn Fn()> = Rc::new(move || {
        if console.form_picking.get() {
            return;
        }
        console.form_picking.set(true);
        spawn_then(pick_many(), move |picked| {
            console.form_picking.set(false);
            match picked {
                Ok(more) if !more.is_empty() => {
                    console.comment_files.update(|files| {
                        let mut next = files.clone();
                        next.extend(more.iter().cloned());
                        next
                    });
                }
                Ok(_) => {}
                Err(why) => console.comment_error.set(why),
            }
        });
    });

    // What the composer is doing, as a line above it: keyed on the
    // answering target and the kind, which are the two things that
    // change its shape. The body field is outside this switch so a
    // kind change cannot rebuild it under the cursor.
    let head = switch(
        move || (console.comment_answering.get(), console.comment_kind.get()),
        move |(answering, kind): &(Option<String>, String)| match answering {
            Some(qid) => {
                let (who, what) = question_summary(qid);
                let line = format!("Answering {who}: {what}");
                ui! {
                    view(style = AnswerLine()) {
                        Typography(content = line, kind = typography_kind::Caption)
                        Spacer()
                        Button(label = "Cancel", on_click = cancel_answer.clone(), size = size::Sm, variant = variant::Ghost)
                    }
                }
            }
            None => {
                let asking = kind == "question";
                let assignee_change = on_assignee.clone();
                ui! {
                    view(style = KindRow()) {
                        Chip(label = "Note", selected = !asking, on_select = Some(note.clone()), tone = tone::Primary)
                        Chip(label = "Question", selected = asking, on_select = Some(question.clone()), tone = tone::Warning)
                        if asking {
                            view(style = AssigneeSlot()) {
                                Field(
                                    value = console.comment_assignee,
                                    on_change = assignee_change.clone(),
                                    placeholder = Some("Owed by (agent or person; blank = anyone)".to_string()),
                                    size = idea_ui::FieldSize::Sm,
                                )
                            }
                        }
                    }
                }
            }
        },
    );
    let files = switch(
        move || console.comment_files.get(),
        move |files: &Vec<PickedBlob>| {
            let count = files.len();
            let files = files.clone();
            ui! {
                view(style = FileChips()) {
                    for i in 0..count {
                        FileChip(console = console, index = i, label = format!("{} \u{b7} {}", files[i].name, api::size_label(files[i].bytes.len() as i64)))
                    }
                }
            }
        },
    );
    let refusal = switch(
        move || console.comment_error.get(),
        move |error: &String| {
            if error.is_empty() {
                return ui! { view {} };
            }
            let error = error.clone();
            ui! { text(style = RefusalLine()) { error } }
        },
    );
    ui! {
        view(style = ComposerBox()) {
            head
            Textarea(
                value = console.comment_draft,
                on_change = on_body,
                placeholder = Some("Say it here. Markdown.".to_string()),
                rows = 2u32,
                max_rows = 12u32,
            )
            files
            refusal
            view(style = FooterRow()) {
                Button(
                    label = "Attach",
                    on_click = pick,
                    variant = variant::Ghost,
                    size = size::Sm,
                    loading = rx!(console.form_picking.get()),
                )
                Spacer()
                Button(
                    label = rx!(match (console.comment_answering.get(), console.comment_kind.get().as_str()) {
                        (Some(_), _) => "Answer".to_string(),
                        (None, "question") => "Ask".to_string(),
                        _ => "Post".to_string(),
                    }),
                    on_click = post,
                    size = size::Sm,
                    loading = rx!(console.comment_busy.get()),
                    disabled = rx!(
                        console.comment_draft.get().trim().is_empty()
                            && console.comment_files.get().is_empty()
                    ),
                )
            }
        }
    }
}

/// Props for [`FileChip`].
#[derive(Default, IdealystSchema)]
pub struct FileChipProps {
    /// Console state handles.
    pub console: Console,
    /// Index into `comment_files`.
    pub index: usize,
    /// "name · size"
    pub label: String,
}

/// One file waiting in the composer; pressing it drops it.
#[component]
pub fn FileChip(props: &FileChipProps) -> Element {
    let console = props.console;
    let index = props.index;
    let label = format!("{} \u{d7}", props.label);
    let drop: Rc<dyn Fn()> = Rc::new(move || {
        console.comment_files.update(|files| {
            let mut next = files.clone();
            if index < next.len() {
                next.remove(index);
            }
            next
        });
    });
    ui! {
        Chip(label = label, selected = false, on_select = Some(drop.clone()), tone = tone::Neutral)
    }
}

/// Who asked the question being answered, and what — from the threads
/// on screen.
fn question_summary(question_id: &str) -> (String, String) {
    match model::comment_by_id(question_id) {
        Some(c) => (c.author, truncate(&c.body, 80)),
        None => ("a question".to_string(), String::new()),
    }
}

fn truncate(s: &str, max: usize) -> String {
    let first = s.lines().next().unwrap_or("");
    let mut out: String = first.chars().take(max).collect();
    if first.chars().count() > max || s.lines().count() > 1 {
        out.push('\u{2026}');
    }
    out
}

/// Present the open dialog for several files and read them in.
async fn pick_many() -> Result<Vec<PickedBlob>, String> {
    use file_picker::{FilePicker, PickOutcome, PickRequest};
    let outcome = FilePicker::new()
        .pick(PickRequest::documents(Vec::<String>::new()).multiple())
        .await
        .map_err(|e| format!("Could not open the file picker: {e}"))?;
    let PickOutcome::Picked(files) = outcome else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(files.len());
    for file in files {
        let bytes = file.read_all().await.map_err(|e| format!("Could not read {}: {e}", file.name()))?;
        out.push(PickedBlob {
            name: file.name().to_string(),
            content_type: file.mime().to_string(),
            bytes: Rc::new(bytes),
        });
    }
    Ok(out)
}

/// The multipart body the comment route reads: `kind`, `body`, and
/// either `assigned_to` or `answers`, then one `file` part per file.
pub fn comment_multipart(console: Console) -> (String, Vec<u8>) {
    let boundary = format!("----mcpm-cmt-{}", runtime_core::time::now_micros());
    let answering = console.comment_answering.get();
    let kind = match &answering {
        Some(_) => "answer".to_string(),
        None => console.comment_kind.get(),
    };
    let mut body: Vec<u8> = Vec::new();
    let mut text_part = |name: &str, value: &str| {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    };
    text_part("kind", &kind);
    text_part("body", &console.comment_draft.get());
    match &answering {
        Some(q) => text_part("answers", q),
        None if kind == "question" => text_part("assigned_to", console.comment_assignee.get().trim()),
        None => {}
    }
    for blob in console.comment_files.get() {
        let name = blob.name.replace('"', "_");
        let content_type = if blob.content_type.is_empty() {
            "application/octet-stream"
        } else {
            blob.content_type.as_str()
        };
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\n\
                 Content-Type: {content_type}\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(&blob.bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

// ---------------------------------------------------------------------
// Styles
// ---------------------------------------------------------------------

stylesheet! {
    pub ThreadCol<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.md(),
            min_width: 0,
        }
    }
}

stylesheet! {
    pub ThreadList<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            min_width: 0,
        }
    }
}

// A question and an answer are tinted at the edge, not boxed: the
// thread is one list and the tint is what the eye scans for.
stylesheet! {
    pub RowBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
            padding_vertical: t.spacing.sm(),
            padding_left: t.spacing.sm(),
            border_color: t.color.border(),
            border_left_width: 2.0,
            min_width: 0,
        }
        variant first {
            #[default]
            no(_t) { border_top_width: 1.0 }
            yes(_t) { border_top_width: 0.0 }
        }
        variant kind {
            #[default]
            note(t) { border_left_color: t.color.border() }
            question(t) { border_left_color: t.intent.warning.fg() }
            answer(t) { border_left_color: t.intent.info.fg() }
        }
    }
}

stylesheet! {
    pub HeadRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
            min_width: 0,
        }
    }
}

stylesheet! {
    pub BodyBox<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            min_width: 0,
        }
    }
}

stylesheet! {
    pub FilesList<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            min_width: 0,
        }
    }
}

stylesheet! {
    pub StatusRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.md(),
        }
    }
}

stylesheet! {
    pub ComposerBox<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
            padding: t.spacing.md(),
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.md(),
            background: t.color.surface(),
            min_width: 0,
        }
    }
}

stylesheet! {
    pub KindRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            flex_wrap: FlexWrap::Wrap,
            gap: t.spacing.xs(),
            min_width: 0,
        }
    }
}

stylesheet! {
    pub AssigneeSlot<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_width: 180,
        }
    }
}

stylesheet! {
    pub AnswerLine<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
            min_width: 0,
        }
    }
}

stylesheet! {
    pub FileChips<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            gap: t.spacing.xs(),
        }
    }
}

stylesheet! {
    pub FooterRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
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

#[cfg(test)]
mod tests {
    use super::{CommentRow, Composer};
    use crate::model::{Attachment, Comment};
    use crate::state::use_console;

    /// The thread's rows and the composer mount through the real
    /// `realize` with the app's registrations — the markdown body is
    /// a scene-registry payload, and an unregistered one fails at
    /// mount, not at compile.
    #[test]
    fn the_discussion_surfaces_mount_with_the_apps_registrations() {
        let harness = host_mock::Harness::with_registry(crate::register_scene_extensions);
        let tree = harness.world.enter(|| {
            idea_ui::install_idea_theme(idea_ui::light_theme());
            let console = use_console();
            let note = Comment {
                id: "cmt_1".into(),
                kind: "note".into(),
                body: "A **note** with a file.".into(),
                author: "human".into(),
                posted: "Sep 14 10:00".into(),
                attachments: vec![Attachment { id: "att_1".into(), name: "a.txt".into(), ..Default::default() }],
                ..Default::default()
            };
            let question = Comment {
                id: "cmt_2".into(),
                kind: "question".into(),
                body: "Which currency?".into(),
                author: "manager".into(),
                assigned_to: "nicho".into(),
                posted: "Sep 14 10:01".into(),
                ..Default::default()
            };
            let answer = Comment {
                id: "cmt_3".into(),
                kind: "answer".into(),
                body: "USD.".into(),
                author: "nicho".into(),
                answers: "cmt_2".into(),
                posted: "Sep 14 10:02".into(),
                ..Default::default()
            };
            runtime_core::ui! {
                view {
                    CommentRow(console = console, subject = "feat_1".to_string(), comment = note, first = true)
                    CommentRow(console = console, subject = "feat_1".to_string(), comment = question, first = false)
                    CommentRow(console = console, subject = "feat_1".to_string(), comment = answer, first = false)
                    Composer(console = console, subject = "feat_1".to_string())
                }
            }
        });
        harness.mount(tree);
        harness.flush();
    }

    /// The multipart body the comment route parses. Signal writes land
    /// on the next flush, so each step is a turn of its own.
    #[test]
    fn comment_multipart_carries_kind_body_target_and_files() {
        let harness = host_mock::Harness::with_registry(crate::register_scene_extensions);
        let console = harness.world.enter(|| {
            let console = use_console();
            console.comment_kind.set("question".into());
            console.comment_assignee.set(" manager ".into());
            console.comment_draft.set("Which one?".into());
            console.comment_files.set(vec![crate::state::PickedBlob {
                name: "a.csv".into(),
                content_type: "text/csv".into(),
                bytes: std::rc::Rc::new(b"1,2".to_vec()),
            }]);
            console
        });
        harness.flush();
        harness.world.enter(|| {
            let (ct, body) = super::comment_multipart(console);
            let text = String::from_utf8_lossy(&body);
            assert!(ct.starts_with("multipart/form-data; boundary="));
            assert!(text.contains("name=\"kind\"\r\n\r\nquestion\r\n"));
            assert!(text.contains("name=\"body\"\r\n\r\nWhich one?\r\n"));
            assert!(text.contains("name=\"assigned_to\"\r\n\r\nmanager\r\n"));
            assert!(text.contains("name=\"file\"; filename=\"a.csv\"\r\nContent-Type: text/csv\r\n\r\n1,2\r\n"));
            // Answering overrides the kind and names the question.
            console.comment_answering.set(Some("cmt_9".into()));
        });
        harness.flush();
        harness.world.enter(|| {
            let (_, body) = super::comment_multipart(console);
            let text = String::from_utf8_lossy(&body);
            assert!(text.contains("name=\"kind\"\r\n\r\nanswer\r\n"));
            assert!(text.contains("name=\"answers\"\r\n\r\ncmt_9\r\n"));
            assert!(!text.contains("assigned_to"));
        });
    }
}
