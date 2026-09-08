//! Dependency-graph view: the feature agent fanning out through the
//! stage gates to its module subagents.

use idea_ui::{typography_kind, IdeaThemeRef, Spacer, Stack, StackAlign, StackAxis, StackGap,
    Typography};
use runtime_core::{
    component, stylesheet, ui, AlignItems, Element, FlexDirection, FontWeight,
    IdealystSchema, JustifyContent,
};

use crate::components::bits::{Hint, Mono, StatusBadge};
use crate::components::module_card::ModuleRow;
use crate::model::{features, Status};
use crate::state::Console;
use crate::styles::{MonoTextSize, SectionLabel};

/// Props for [`GraphView`].
#[derive(Default, IdealystSchema)]
pub struct GraphViewProps {
    /// Console state handles.
    pub console: Console,
    /// Feature index.
    pub feature: usize,
}

/// The dependency-graph view for one feature.
#[component]
pub fn GraphView(props: &GraphViewProps) -> Element {
    let console = props.console;
    let fi = props.feature;
    let feats = features();
    let f = &feats[fi];
    let agent = f.agent.to_string();
    let stage_count = f.stages.len();
    ui! {
        view(style = GraphScroll()) {
            view(style = GraphPad()) {
                view(style = LegendRowBox()) {
                    Spacer()
                    Hint(
                        text = "Green edge: gate satisfied, work released.\n\
                                Amber edge: gate blocking the stages downstream.\n\
                                Dimmed module: subagent not yet spawned.",
                    )
                }
                // The strip is a row of fixed-width stage nodes, so it
                // outgrows the viewport as soon as a feature has more
                // than a few stages. `scroll_view` is single-axis, so
                // the wide part gets its own horizontal scroller rather
                // than clipping inside the vertical one.
                scroll_view(horizontal = true, style = StripScroll()) {
                    view(style = GraphStrip()) {
                    view(style = AgentCell()) {
                        Stack(axis = StackAxis::Row, gap = StackGap::Xs, align = StackAlign::Center) {
                            text(style = SectionLabel()) { "Feature agent" }
                            Hint(text = "Plans the stages and spawns one subagent per module.")
                        }
                        Mono(content = agent, tone = crate::styles::MonoTextTone::Text)
                    }
                    for si in 0..stage_count {
                        GraphEdge(feature = fi, stage = si)
                        GraphStage(
                            console = console,
                            feature = fi,
                            stage = si,
                        )
                    }
                    }
                }
            }
        }
    }
}

/// Props for [`GraphEdge`].
#[derive(Default, IdealystSchema)]
pub struct GraphEdgeProps {
    /// Feature index.
    pub feature: usize,
    /// The stage this edge leads INTO.
    pub stage: usize,
}

/// The connector between the previous node and a stage card.
#[component]
pub fn GraphEdge(props: &GraphEdgeProps) -> Element {
    let feats = features();
    let f = &feats[props.feature];
    // The edge into stage N blocks while stage N-1 is unfinished.
    let blocking = props.stage > 0 && f.stages[props.stage - 1].status != Status::Done;
    let reached = !blocking
        && !matches!(f.stages[props.stage].status, Status::Queued);
    let arm = if blocking {
        GraphEdgeLineTone::Blocking
    } else if reached {
        GraphEdgeLineTone::Open
    } else {
        GraphEdgeLineTone::Idle
    };
    ui! {
        view(style = EdgeCell()) {
            view(style = GraphEdgeLine().tone(arm)) {}
        }
    }
}

/// Props for [`GraphStage`].
#[derive(Default, IdealystSchema)]
pub struct GraphStageProps {
    /// Console state handles.
    pub console: Console,
    /// Feature index.
    pub feature: usize,
    /// Stage index.
    pub stage: usize,
}

/// One stage node with its module subagent rows.
#[component]
pub fn GraphStage(props: &GraphStageProps) -> Element {
    let console = props.console;
    let (fi, si) = (props.feature, props.stage);
    let feats = features();
    let stage = &feats[fi].stages[si];
    let code = format!("STAGE {:02}", si + 1);
    let name = stage.name.clone();
    let status = stage.status;
    let module_count = stage.modules.len();
    let fanout = if module_count > 1 {
        format!("{module_count} subagents")
    } else {
        "1 subagent".to_string()
    };
    let arm = if status == Status::Running {
        StageNodeState::Active
    } else {
        StageNodeState::Idle
    };
    ui! {
        view(style = StageNode().state(arm)) {
            Stack(axis = StackAxis::Row, align = StackAlign::Center) {
                Mono(content = code, size = MonoTextSize::Overline)
                Spacer()
                StatusBadge(status = status)
            }
            Typography(
                content = name,
                kind = typography_kind::Body,
                weight = Some(FontWeight::SemiBold),
            )
            scroll_view(style = NodeModules()) {
                for mi in 0..module_count {
                    ModuleRow(
                        console = console,
                        feature = fi,
                        stage = si,
                        module = mi,
                    )
                }
            }
            text(style = FanoutNote()) { fanout }
        }
    }
}

// The graph is a canvas: it takes the pane rather than sizing to its
// diagram, so the horizontal scrollbar lands at the bottom edge.
//
// A plain view and not a `scroll_view`, deliberately. Wrapping this in a
// vertical scroller would make everything inside auto-height again —
// scroll content sizes to itself — and the strip's `flex_grow` would
// have nothing to grow against. The vertical overflow lives one level
// in, on each stage node's module list, where it can actually be bounded.
stylesheet! {
    pub GraphScroll<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            min_height: 0,
            min_width: 0,
            flex_direction: FlexDirection::Column,
        }
    }
}

stylesheet! {
    pub GraphPad<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.md(),
            // Horizontal padding belongs to the strip, which scrolls;
            // putting it here would inset the scrollbar from the pane
            // edge and clip the first node against it (rule 3).
            padding_top: t.spacing.lg(),
            padding_horizontal: 0,
            // Fills the pane so the strip below can too, which is what
            // puts its horizontal scrollbar at the bottom of the screen
            // rather than under the diagram.
            flex_grow: 1.0,
            min_height: 0,
        }
    }
}

// No padding here — it lives on `GraphStrip` inside, so the scrollbar
// hugs the container edge (rule 3).
stylesheet! {
    pub StripScroll<IdeaThemeRef> {
        base(t) {
            min_width: 0,
            flex_grow: 1.0,
            min_height: 0,
        }
    }
}

// Not a card. A surface that has to be as wide as its content is not a
// container the reader can see the edges of — at eight stages its right
// border sits two screens away, so the border and background only ever
// read as "the diagram is broken out of its box". The nodes are cards;
// the strip is just the row they sit in.
stylesheet! {
    pub GraphStrip<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Stretch,
            // A DEFINITE height, not a minimum. `min_height` lets the
            // row grow to its tallest lane, which then makes the whole
            // board scroll vertically and pushes the horizontal
            // scrollbar off the bottom of the screen — the exact bug
            // this nesting exists to fix, reappearing as soon as one
            // lane is deep. Pinned to the viewport, the lanes are
            // bounded and their card lists scroll instead.
            height: runtime_core::Length::Percent(100.0),
            // Padding on the content, so the scrollbar hugs the
            // container edge (rule 3).
            padding_vertical: t.spacing.sm(),
            padding_horizontal: t.spacing.xl(),
        }
    }
}

stylesheet! {
    pub AgentCell<IdeaThemeRef> {
        base(t) {
            width: 150,
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            justify_content: JustifyContent::Center,
            gap: t.spacing.sm(),
            padding_right: t.spacing.lg(),
        }
    }
}

stylesheet! {
    pub EdgeCell<IdeaThemeRef> {
        base(t) {
            width: 34,
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            justify_content: JustifyContent::Center,
        }
    }
}

stylesheet! {
    pub GraphEdgeLine<IdeaThemeRef> {
        base(t) {
            height: 2,
        }
        variant tone {
            #[default]
            idle(t) { background: t.color.border() }
            open(t) { background: t.intent.success.fg() }
            blocking(t) { background: t.intent.warning.fg() }
        }
    }
}

stylesheet! {
    pub StageNode<IdeaThemeRef> {
        base(t) {
            width: 270,
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
            padding: t.spacing.md(),
            border_width: 1.0,
            border_radius: t.radius.md(),
            // Bounded by the strip, so the module list inside has a
            // definite height to scroll within.
            min_height: 0,
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
        }
    }
}

stylesheet! {
    pub NodeModules<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
            padding_top: 2,
            // Takes the slack the node's header leaves, and no more:
            // `min_height: 0` is what lets a flex child shrink below its
            // content and therefore scroll at all.
            flex_grow: 1.0,
            min_height: 0,
        }
    }
}

stylesheet! {
    pub FanoutNote<IdeaThemeRef> {
        base(t) {
            font_size: 9,
            text_transform: runtime_core::TextTransform::Uppercase,
            letter_spacing: 0.8,
            color: t.color.text_muted(),
            text_align: runtime_core::TextAlign::Center,
            padding_top: 2,
        }
    }
}

stylesheet! {
    pub LegendRowBox<IdeaThemeRef> {
        base(t) {
            padding_horizontal: t.spacing.xl(),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.xs(),
            padding_bottom: t.spacing.sm(),
        }
    }
}
