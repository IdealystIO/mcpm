//! Hierarchy view: the Project → Feature → Stage → Module → Task tree
//! as collapsible rows. Stage rows toggle their modules; module rows
//! toggle their task checklists and open the drawer.

use idea_ui::{typography_kind, Badge, IdeaThemeRef, Progress, ProgressCap, Typography};
use runtime_core::{
    component, pressable, stylesheet, ui, AlignItems, Element, FlexDirection, FontWeight,
    IdealystSchema, IntoElement, StyleApplication, TextAlign,
};

use crate::components::bits::{Mono, StatusDot};
use crate::model::{features, Status};
use crate::state::Console;
use crate::styles::MonoTextSize;

/// Props for [`TreeView`].
#[derive(Default, IdealystSchema)]
pub struct TreeViewProps {
    /// Console state handles.
    pub console: Console,
    /// Feature index.
    pub feature: usize,
    /// Snapshot of toggled tree keys.
    pub toggled: Vec<String>,
    /// Open drawer target, for row highlight.
    pub selected: Option<(usize, usize)>,
}

/// The hierarchy view for one feature.
#[component]
pub fn TreeView(props: &TreeViewProps) -> Element {
    let console = props.console;
    let fi = props.feature;
    let toggled = props.toggled.clone();
    let selected = props.selected;
    let feats = features();
    let f = &feats[fi];
    let stage_count = f.stages.len();
    let fname = f.name.clone();
    let felapsed = f.elapsed.clone();
    let fstatus = f.status;
    let ffraction = f.fraction();

    ui! {
        view(style = TreeBox()) {
            TreeRow(
                console = console,
                caret = "▾",
                kind = "project",
                label = "control-center",
                status = Status::Planning,
                indent = 0usize,
                weight = 3usize,
                head = true,
            )
            TreeRow(
                console = console,
                caret = "▾",
                kind = "feature",
                label = fname,
                meta = felapsed,
                status = fstatus,
                indent = 1usize,
                weight = 2usize,
                bar = ffraction,
                bar_status = fstatus,
            )
            for si in 0..stage_count {
                StageRows(
                    console = console,
                    feature = fi,
                    stage = si,
                    toggled = toggled.clone(),
                    selected = selected,
                )
            }
        }
    }
}

/// Props for [`StageRows`].
#[derive(Default, IdealystSchema)]
pub struct StageRowsProps {
    /// Console state handles.
    pub console: Console,
    /// Feature index.
    pub feature: usize,
    /// Stage index.
    pub stage: usize,
    /// Snapshot of toggled tree keys.
    pub toggled: Vec<String>,
    /// Open drawer target.
    pub selected: Option<(usize, usize)>,
}

/// One stage's row plus (when open) its module subtrees.
#[component]
pub fn StageRows(props: &StageRowsProps) -> Element {
    let console = props.console;
    let (fi, si) = (props.feature, props.stage);
    let feats = features();
    let stage = &feats[fi].stages[si];
    let key = format!("s{si}");
    // Stages default OPEN; a toggle entry closes them.
    let open = !Console::is_toggled(&props.toggled, &key);
    let module_count = stage.modules.len();
    let toggled = props.toggled.clone();
    let selected = props.selected;

    ui! {
        view(style = TreeGroup()) {
            TreeRow(
                console = console,
                caret = if open { "▾" } else { "▸" },
                kind = format!("stage {:02}", si + 1),
                label = stage.name.to_string(),
                meta = stage.time.to_string(),
                status = stage.status,
                indent = 2usize,
                weight = 2usize,
                bar = stage.fraction(),
                bar_status = stage.status,
                toggle_key = key,
            )
            if open {
                for mi in 0..module_count {
                    ModuleRows(
                        console = console,
                        feature = fi,
                        stage = si,
                        module = mi,
                        toggled = toggled.clone(),
                        selected = selected,
                    )
                }
            }
        }
    }
}

/// Props for [`ModuleRows`].
#[derive(Default, IdealystSchema)]
pub struct ModuleRowsProps {
    /// Console state handles.
    pub console: Console,
    /// Feature index.
    pub feature: usize,
    /// Stage index.
    pub stage: usize,
    /// Module index.
    pub module: usize,
    /// Snapshot of toggled tree keys.
    pub toggled: Vec<String>,
    /// Open drawer target.
    pub selected: Option<(usize, usize)>,
}

/// One module's row plus (when open) its task rows.
#[component]
pub fn ModuleRows(props: &ModuleRowsProps) -> Element {
    let console = props.console;
    let (fi, si, mi) = (props.feature, props.stage, props.module);
    let feats = features();
    let m = &feats[fi].stages[si].modules[mi];
    let key = format!("m{si}.{mi}");
    // Modules default CLOSED; a toggle entry opens their tasks.
    let open = Console::is_toggled(&props.toggled, &key);
    let task_count = m.tasks.len();
    let tasks: Vec<(String, bool, bool)> = m
        .tasks
        .iter()
        .map(|t| (t.label.to_string(), t.done, t.added))
        .collect();
    let _ = task_count;

    ui! {
        view(style = TreeGroup()) {
            TreeRow(
                console = console,
                caret = if open { "▾" } else { "▸" },
                kind = "module",
                label = m.name.to_string(),
                meta = m.agent.to_string(),
                status = m.status,
                indent = 3usize,
                weight = 1usize,
                bar = m.fraction(),
                bar_status = m.status,
                tag = if m.status == Status::Violation { "rejected" } else { "" }.to_string(),
                tag_status = Status::Violation,
                toggle_key = key,
                open_stage = si as i32,
                open_module = mi as i32,
                selected = props.selected == Some((si, mi)),
            )
            if open {
                for (label, done, added) in tasks {
                    TreeRow(
                        console = console,
                        kind = "task",
                        label = label,
                        meta = if done { "checked off" } else { "open" }.to_string(),
                        status = if done { Status::Done } else { Status::Queued },
                        indent = 4usize,
                        strike = done,
                        tag = if added { "agent-added" } else { "" }.to_string(),
                        tag_status = Status::Running,
                    )
                }
            }
        }
    }
}

/// Props for [`TreeRow`].
#[derive(Default, IdealystSchema)]
pub struct TreeRowProps {
    /// Console state handles.
    pub console: Console,
    /// Disclosure caret glyph ("▾" / "▸" / "").
    pub caret: &'static str,
    /// Kind column label ("project", "feature", "stage 01", …).
    pub kind: String,
    /// Row label.
    pub label: String,
    /// Right-aligned metadata.
    pub meta: String,
    /// Status for the row's dot.
    pub status: Status,
    /// Indent depth 0..=4.
    pub indent: usize,
    /// Label weight: 0 normal, 1 medium, 2 semibold, 3 bold.
    pub weight: usize,
    /// Strike the label (done tasks).
    pub strike: bool,
    /// Highlight as the drawer's module.
    pub selected: bool,
    /// Header row (project) gets the alt background.
    pub head: bool,
    /// Task fraction for the trailing bar; negative = no bar.
    pub bar: f32,
    /// Tone source for the trailing bar.
    pub bar_status: Status,
    /// Optional trailing tag label; empty = none.
    pub tag: String,
    /// Tone source for the tag.
    pub tag_status: Status,
    /// Tree key to toggle on press; empty = not toggleable.
    pub toggle_key: String,
    /// Drawer stage index to open on press; negative = none.
    pub open_stage: i32,
    /// Drawer module index to open on press; negative = none.
    pub open_module: i32,
}

/// One row of the hierarchy view.
#[component]
pub fn TreeRow(props: &TreeRowProps) -> Element {
    let console = props.console;
    let caret = props.caret;
    let kind = props.kind.clone();
    let label = props.label.clone();
    let meta = props.meta.clone();
    let status = props.status;
    let has_bar = props.bar >= 0.0 && !props.head && props.kind != "task";
    let bar = props.bar.max(0.0);
    let bar_tone = crate::styles::status_tone(props.bar_status);
    let tag = props.tag.clone();
    let has_tag = !tag.is_empty();
    let tag_status = props.tag_status;
    let weight_arm = match props.weight {
        0 => RowLabelWeight::Normal,
        1 => RowLabelWeight::Medium,
        2 => RowLabelWeight::Semibold,
        _ => RowLabelWeight::Bold,
    };
    let label_style = RowLabel()
        .strike(if props.strike {
            RowLabelStrike::Yes
        } else {
            RowLabelStrike::No
        })
        .weight(weight_arm);

    let inner: Element = ui! {
        view(style = RowInnerSheet().indent(indent_arm(props.indent))) {
            view(style = CaretCell()) {
                Mono(content = caret.to_string(), size = MonoTextSize::Overline)
            }
            StatusDot(status = status)
            view(style = KindCell()) {
                Mono(content = kind, size = MonoTextSize::Overline)
            }
            view(style = LabelCell()) {
                text(style = label_style) { label }
            }
            if has_tag {
                StatusTag(label = tag, status = tag_status)
            }
            view(style = MetaCell()) {
                Typography(
                    content = meta,
                    kind = typography_kind::Caption,
                    muted = true,
                    align = TextAlign::Right,
                )
            }
            view(style = BarCell()) {
                if has_bar {
                    Progress(value = bar, tone = bar_tone, cap = ProgressCap::Rounded)
                }
            }
        }
    };

    let toggle_key = props.toggle_key.clone();
    let (os, om) = (props.open_stage, props.open_module);
    let clickable = !toggle_key.is_empty();
    let arm = if props.head {
        "head"
    } else if props.selected {
        "selected"
    } else {
        "plain"
    };
    if clickable {
        pressable(vec![inner], move || {
            console.toggle(&toggle_key);
            if os >= 0 && om >= 0 {
                console.open_module(os as usize, om as usize);
            }
        })
        .with_style(StyleApplication::new(tree_row_box_style()).with("bg", arm.to_string()))
        .into_element()
    } else {
        let style = TreeRowBox().bg(match arm {
            "head" => TreeRowBoxBg::Head,
            "selected" => TreeRowBoxBg::Selected,
            _ => TreeRowBoxBg::Plain,
        });
        ui! {
            view(style = style) { inner }
        }
    }
}

fn indent_arm(indent: usize) -> RowInnerSheetIndent {
    match indent {
        0 => RowInnerSheetIndent::I0,
        1 => RowInnerSheetIndent::I1,
        2 => RowInnerSheetIndent::I2,
        3 => RowInnerSheetIndent::I3,
        _ => RowInnerSheetIndent::I4,
    }
}

/// Props for [`StatusTag`].
#[derive(Default, IdealystSchema)]
pub struct StatusTagProps {
    /// Tag text.
    pub label: String,
    /// Status whose tone the tag takes.
    pub status: Status,
}

/// Small toned tag used on tree rows ("gated", "rejected", "agent-added").
#[component]
pub fn StatusTag(props: &StatusTagProps) -> Element {
    let label = props.label.clone();
    let tone = crate::styles::status_tone(props.status);
    ui! {
        Badge(label = label, tone = tone)
    }
}

stylesheet! {
    pub TreeBox<IdeaThemeRef> {
        base(t) {
            max_width: 1040,
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.lg(),
            background: t.color.surface(),
            overflow: runtime_core::Overflow::Hidden,
            flex_direction: FlexDirection::Column,
        }
    }
}

stylesheet! {
    pub TreeGroup<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
        }
    }
}

stylesheet! {
    pub TreeRowBox<IdeaThemeRef> {
        base(t) {
            border_top_width: 1.0,
            border_color: t.color.border(),
        }
        variant bg {
            #[default]
            plain(t) { background: t.color.surface() }
            head(t) { background: t.color.surface_alt() }
            selected(t) { background: t.intent.primary.soft_bg() }
        }
        state hovered(t) {
            background: t.color.surface_alt(),
        }
    }
}

stylesheet! {
    pub RowInnerSheet<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.md(),
            padding_vertical: 10,
            padding_right: t.spacing.lg(),
        }
        variant indent {
            #[default]
            i0(t) { padding_left: 16 }
            i1(t) { padding_left: 38 }
            i2(t) { padding_left: 60 }
            i3(t) { padding_left: 86 }
            i4(t) { padding_left: 116 }
        }
    }
}

stylesheet! {
    pub CaretCell<IdeaThemeRef> {
        base(t) { width: 14, flex_shrink: 0.0 }
    }
}

stylesheet! {
    pub KindCell<IdeaThemeRef> {
        base(t) { width: 66, flex_shrink: 0.0 }
    }
}

stylesheet! {
    pub LabelCell<IdeaThemeRef> {
        base(t) { flex_grow: 1.0, min_width: 0 }
    }
}

stylesheet! {
    pub MetaCell<IdeaThemeRef> {
        base(t) { width: 120, flex_shrink: 0.0 }
    }
}

stylesheet! {
    pub BarCell<IdeaThemeRef> {
        base(t) { width: 120, flex_shrink: 0.0 }
    }
}

stylesheet! {
    pub RowLabel<IdeaThemeRef> {
        base(t) {
            font_size: t.typography.body_size(),
            color: t.color.text(),
        }
        variant strike {
            #[default]
            no(t) { strikethrough: false }
            yes(t) {
                strikethrough: true,
                color: t.color.text_muted(),
            }
        }
        variant weight {
            #[default]
            normal(t) { font_weight: FontWeight::Normal }
            medium(t) { font_weight: FontWeight::Medium }
            semibold(t) { font_weight: FontWeight::SemiBold }
            bold(t) { font_weight: FontWeight::Bold }
        }
    }
}
