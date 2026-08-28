//! Stage-pipeline board: one lane per stage, gate markers between
//! lanes, module cards inside. Lanes scroll horizontally.

use idea_ui::{typography_kind, IdeaThemeRef, Progress, Spacer, Stack, StackAlign, StackAxis, Typography};
use runtime_core::{
    component, stylesheet, ui, AlignItems, Element, FlexDirection, FontWeight, IdealystSchema,
    TextAlign,
};

use crate::components::bits::{Mono, StatusBadge};
use crate::components::module_card::ModuleCard;
use crate::model::{features, Status};
use crate::state::Console;
use crate::styles::{status_tone, MonoTextSize};

/// Props for [`BoardView`].
#[derive(Default, IdealystSchema)]
pub struct BoardViewProps {
    /// Console state handles.
    pub console: Console,
    /// Feature index.
    pub feature: usize,
    /// Open drawer target, for card highlight.
    pub selected: Option<(usize, usize)>,
}

/// The stage-pipeline board for one feature.
#[component]
pub fn BoardView(props: &BoardViewProps) -> Element {
    let console = props.console;
    let fi = props.feature;
    let selected = props.selected;
    let count = features()[fi].stages.len();
    ui! {
        scroll_view(style = BoardScroll()) {
            view(style = LaneRow()) {
                for si in 0..count {
                    StageLane(console = console, feature = fi, stage = si, selected = selected)
                    if si + 1 < count {
                        GateMarker(feature = fi, stage = si)
                    }
                }
            }
        }
    }
}

/// Props for [`StageLane`] .
#[derive(Default, IdealystSchema)]
pub struct StageLaneProps {
    /// Console state handles.
    pub console: Console,
    /// Feature index.
    pub feature: usize,
    /// Stage index.
    pub stage: usize,
    /// Open drawer target, for card highlight.
    pub selected: Option<(usize, usize)>,
}

/// One stage lane on the board.
#[component]
pub fn StageLane(props: &StageLaneProps) -> Element {
    let console = props.console;
    let (fi, si) = (props.feature, props.stage);
    let feats = features();
    let stage = &feats[fi].stages[si];

    let code = format!("STAGE {:02}", si + 1);
    let name = stage.name.clone();
    let status = stage.status;
    let time = stage.time.clone();
    let done_label = stage.done_label();
    let fraction = stage.fraction();
    let concurrency = stage.concurrency_note();
    let module_count = stage.modules.len();
    let selected = props.selected;
    let lane_arm = match status {
        Status::Running => StageBoxState::Active,
        Status::Blocked | Status::Queued => StageBoxState::Locked,
        _ => StageBoxState::Idle,
    };

    ui! {
        view(style = StageBox().state(lane_arm)) {
            view(style = LaneHead()) {
                Stack(axis = StackAxis::Row, align = StackAlign::Center) {
                    Mono(content = code, size = MonoTextSize::Overline)
                    Spacer()
                    StatusBadge(status = status)
                }
                Typography(
                    content = name,
                    kind = typography_kind::BodyLg,
                    weight = Some(FontWeight::SemiBold),
                )
                Stack(axis = StackAxis::Row, align = StackAlign::Center) {
                    Typography(content = time, kind = typography_kind::Caption, muted = true)
                    Spacer()
                    Typography(content = done_label, kind = typography_kind::Caption, muted = true)
                }
                Progress(value = fraction, tone = status_tone(status))
            }
            view(style = LaneCards()) {
                for mi in 0..module_count {
                    ModuleCard(
                        console = console,
                        feature = fi,
                        stage = si,
                        module = mi,
                        selected = selected == Some((si, mi)),
                    )
                }
            }
            text(style = ConcurrencyNote()) { concurrency }
        }
    }
}

/// Props for [`GateMarker`].
#[derive(Default, IdealystSchema)]
pub struct GateMarkerProps {
    /// Feature index.
    pub feature: usize,
    /// The stage on the LEFT of this gate.
    pub stage: usize,
}

/// The gate marker between two lanes: an arrow that reads green when
/// the left stage is done (gate open) and muted otherwise.
#[component]
pub fn GateMarker(props: &GateMarkerProps) -> Element {
    let feats = features();
    let open = feats[props.feature].stages[props.stage].status == Status::Done;
    let arrow_style = GateArrow().open(if open { GateArrowOpen::Yes } else { GateArrowOpen::No });
    ui! {
        view(style = GateCol()) {
            text(style = arrow_style) { "→" }
            text(style = GateWord()) { "gate" }
        }
    }
}

stylesheet! {
    pub BoardScroll<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            min_height: 0,
        }
    }
}

stylesheet! {
    pub LaneRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexStart,
            gap: t.spacing.md(),
            padding: t.spacing.xl(),
        }
    }
}

stylesheet! {
    pub StageBox<IdeaThemeRef> {
        base(t) {
            width: 300,
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            gap: t.spacing.md(),
            padding: t.spacing.md(),
            border_width: 1.0,
            border_radius: t.radius.lg(),
        }
        variant state {
            #[default]
            idle(t) {
                border_color: t.color.border(),
                background: t.color.surface_alt(),
            }
            active(t) {
                border_color: t.intent.info.border(),
                background: t.intent.info.soft_bg(),
            }
            locked(t) {
                border_color: t.color.border(),
                background: t.color.surface_alt(),
                opacity: 0.85,
            }
        }
    }
}

stylesheet! {
    pub LaneHead<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
        }
    }
}


stylesheet! {
    pub LaneCards<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
        }
    }
}

stylesheet! {
    pub ConcurrencyNote<IdeaThemeRef> {
        base(t) {
            font_size: t.typography.overline_size(),
            color: t.color.text_muted(),
            text_align: TextAlign::Center,
        }
    }
}

stylesheet! {
    pub GateCol<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            gap: t.spacing.xs(),
            padding_top: 64,
            width: 26,
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    pub GateArrow<IdeaThemeRef> {
        base(t) {
            font_size: 16,
        }
        variant open {
            #[default]
            no(t) { color: t.color.border_strong() }
            yes(t) { color: t.intent.success.fg() }
        }
    }
}

stylesheet! {
    pub GateWord<IdeaThemeRef> {
        base(t) {
            font_size: 9,
            text_transform: runtime_core::TextTransform::Uppercase,
            letter_spacing: 1.0,
            color: t.color.text_muted(),
        }
    }
}
