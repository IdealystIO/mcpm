//! The module graph: a layered DAG. One column per depth — every
//! module sits right of everything it depends on — and an edge from
//! each prerequisite to what depends on it.
//!
//! The layout is computed here from known card geometry
//! ([`CARD_W`]/[`CARD_H`]) and the edges are absolutely-positioned
//! views inside one relatively-positioned canvas, drawn BEFORE the
//! cards so a line that has to cross a column passes under it.

use std::collections::HashMap;

use idea_ui::{typography_kind, IdeaThemeRef, Spacer, Typography};
use runtime_core::{
    component, stylesheet, ui, AlignItems, Element, FlexDirection, IdealystSchema, Length,
    Position, StyleRules, Tokenized,
};

use crate::components::bits::Hint;
use crate::components::module_card::{ModuleCard, CARD_H, CARD_W};
use crate::model::{features, Module, Status};
use crate::state::Console;

/// Gap between columns, in px. Wide enough to hold the vertical run of
/// every edge into that column on its own lane (see [`edges`]).
pub const COL_GAP: f32 = 72.0;
/// Gap between cards in a column.
pub const ROW_GAP: f32 = 16.0;
/// The canvas's inner padding. Part of the canvas, not the scroller,
/// so the scrollbars hug the pane edge (rule 3).
pub const PAD: f32 = 24.0;
/// Edge stroke width.
const LINE: f32 = 2.0;
/// Horizontal spacing between the vertical lanes inside a column gap.
const LANE_STEP: f32 = 6.0;
/// Inset of the first lane from the gap's left edge.
const LANE_INSET: f32 = 12.0;

/// Props for [`GraphView`].
#[derive(Default, IdealystSchema)]
pub struct GraphViewProps {
    /// Console state handles.
    pub console: Console,
    /// Feature index.
    pub feature: usize,
}

/// The module graph for one feature.
#[component]
pub fn GraphView(props: &GraphViewProps) -> Element {
    let console = props.console;
    let fi = props.feature;
    let feats = features();
    let f = &feats[fi];
    let count = f.modules.len();
    if count == 0 {
        return ui! {
            view(style = GraphBlank()) {
                Typography(content = "No modules planned.", kind = typography_kind::BodySm, muted = true)
            }
        };
    }

    let layout = GraphLayout::compute(&f.modules);
    let segments = edges(&f.modules, &layout);
    let cards: Vec<CardSlot> = layout
        .slots
        .iter()
        .enumerate()
        .map(|(module, slot)| CardSlot { module, x: layout.x(slot.col), y: layout.y(slot.row) })
        .collect();
    let mut canvas = StyleRules::default();
    canvas.position = Some(Position::Relative);
    canvas.width = Some(Tokenized::Literal(Length::Px(layout.width())));
    canvas.height = Some(Tokenized::Literal(Length::Px(layout.height())));
    canvas.flex_shrink = Some(Tokenized::Literal(0.0));
    // The strip between the two scrollers. A `scroll_view` nested
    // straight inside a horizontal one is sized to its parent's width
    // and clips the rest, so the outer never sees an overflow; a plain
    // view of the canvas's own width is what the outer scrolls. At
    // least the pane's width, so a narrow graph's vertical bar still
    // sits at the pane's right edge; a DEFINITE height, so the canvas
    // scrolls inside it instead of growing it (rule 23).
    let mut strip = StyleRules::default();
    strip.width = Some(Tokenized::Literal(Length::Px(layout.width())));
    strip.min_width = Some(Tokenized::Literal(Length::Percent(100.0)));
    strip.height = Some(Tokenized::Literal(Length::Percent(100.0)));
    strip.min_height = Some(Tokenized::Literal(Length::Px(0.0)));
    strip.flex_shrink = Some(Tokenized::Literal(0.0));
    strip.flex_direction = Some(FlexDirection::Column);

    ui! {
        view(style = GraphScroll()) {
            view(style = LegendRowBox()) {
                Spacer()
                Hint(
                    text = "A module sits right of everything it depends on.\n\
                            Green edge: that prerequisite is done. Amber edge: still open.\n\
                            A card's left edge is green when it can be claimed, amber while it waits.",
                )
            }
            // Two axes, two scrollers (rule 23). The horizontal one
            // fills the pane, so its bar sits at the pane's bottom
            // edge; the vertical one inside is pinned to its height.
            scroll_view(horizontal = true, style = StripScroll()) {
                view(style = strip) {
                    scroll_view(style = CanvasScroll()) {
                        // Edges first, so a line that has to cross a
                        // column passes under the cards in it.
                        view(style = canvas) {
                            for seg in segments, key = seg.id {
                                EdgeSegment(x = seg.x, y = seg.y, w = seg.w, h = seg.h, tone = seg.tone)
                            }
                            for card in cards, key = card.module {
                                GraphCard(console = console, feature = fi, x = card.x, y = card.y, module = card.module)
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Props for [`EdgeSegment`].
#[derive(Default, IdealystSchema)]
pub struct EdgeSegmentProps {
    /// Canvas-relative left edge, px.
    pub x: f32,
    /// Canvas-relative top edge, px.
    pub y: f32,
    /// Width, px.
    pub w: f32,
    /// Height, px.
    pub h: f32,
    /// Colour arm.
    pub tone: EdgeLineTone,
}

/// One straight piece of an edge, absolutely positioned on the canvas.
#[component]
pub fn EdgeSegment(props: &EdgeSegmentProps) -> Element {
    let rules = abs_rules(props.x, props.y, props.w, props.h);
    let tone = props.tone;
    ui! {
        view(style = rules) {
            view(style = EdgeLine().tone(tone)) {}
        }
    }
}

/// Props for [`GraphCard`].
#[derive(Default, IdealystSchema)]
pub struct GraphCardProps {
    /// Console state handles.
    pub console: Console,
    /// Feature index.
    pub feature: usize,
    /// Module index in [`crate::model::Feature::modules`].
    pub module: usize,
    /// Canvas-relative left edge, px.
    pub x: f32,
    /// Canvas-relative top edge, px.
    pub y: f32,
}

/// A module card at its slot on the canvas.
#[component]
pub fn GraphCard(props: &GraphCardProps) -> Element {
    let console = props.console;
    let (fi, mi) = (props.feature, props.module);
    let mut rules = abs_rules(props.x, props.y, CARD_W, CARD_H);
    // A flex column, so the card inside stretches to the slot.
    rules.flex_direction = Some(FlexDirection::Column);
    ui! {
        view(style = rules) {
            ModuleCard(console = console, feature = fi, module = mi)
        }
    }
}

/// A module's slot on the canvas, in px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CardSlot {
    pub module: usize,
    pub x: f32,
    pub y: f32,
}

/// Where one module sits: column = depth − 1, row = its place in that
/// column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    pub col: usize,
    pub row: usize,
}

/// The computed placement of every module, indexed like
/// [`crate::model::Feature::modules`].
#[derive(Debug)]
pub struct GraphLayout {
    pub slots: Vec<Slot>,
    pub cols: usize,
    pub rows: usize,
}

impl GraphLayout {
    /// Columns by depth; rows within a column ordered by the average
    /// row of the module's prerequisites (then by plan order), which
    /// keeps a chain straight and a fan-out next to its source.
    /// Modules arrive topologically sorted, so every prerequisite is
    /// already placed when its dependents are.
    pub fn compute(modules: &[Module]) -> Self {
        let index: HashMap<&str, usize> = modules
            .iter()
            .enumerate()
            .map(|(i, m)| (m.id.as_str(), i))
            .collect();
        let cols = modules.iter().map(|m| m.depth).max().unwrap_or(1);
        let mut by_col: Vec<Vec<usize>> = vec![Vec::new(); cols];
        for (i, m) in modules.iter().enumerate() {
            by_col[m.depth - 1].push(i);
        }
        let mut slots = vec![Slot { col: 0, row: 0 }; modules.len()];
        let mut rows = 0;
        for (col, members) in by_col.iter().enumerate() {
            let mut members = members.clone();
            if col > 0 {
                let placed = slots.clone();
                let key = |i: usize| -> f32 {
                    let prereq_rows: Vec<f32> = modules[i]
                        .depends_on
                        .iter()
                        .filter_map(|id| index.get(id.as_str()).copied())
                        .filter(|&j| placed[j].col < col)
                        .map(|j| placed[j].row as f32)
                        .collect();
                    if prereq_rows.is_empty() {
                        f32::MAX
                    } else {
                        prereq_rows.iter().sum::<f32>() / prereq_rows.len() as f32
                    }
                };
                // Stable, so ties keep plan order.
                members.sort_by(|&a, &b| key(a).total_cmp(&key(b)));
            }
            for (row, &i) in members.iter().enumerate() {
                slots[i] = Slot { col, row };
            }
            rows = rows.max(members.len());
        }
        GraphLayout { slots, cols, rows: rows.max(1) }
    }

    /// Left edge of a column's cards.
    pub fn x(&self, col: usize) -> f32 {
        PAD + col as f32 * (CARD_W + COL_GAP)
    }

    /// Top edge of a row's cards.
    pub fn y(&self, row: usize) -> f32 {
        PAD + row as f32 * (CARD_H + ROW_GAP)
    }

    /// Canvas width, padding included.
    pub fn width(&self) -> f32 {
        PAD * 2.0 + self.cols as f32 * CARD_W + (self.cols - 1) as f32 * COL_GAP
    }

    /// Canvas height, padding included.
    pub fn height(&self) -> f32 {
        PAD * 2.0 + self.rows as f32 * CARD_H + (self.rows - 1) as f32 * ROW_GAP
    }
}

/// One straight piece of an edge, in canvas px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    /// Position in the edge list — the reconciliation key.
    pub id: usize,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub tone: EdgeLineTone,
}

/// Every edge as three segments — out of the prerequisite, down or up
/// the gap before the dependent's column, into the dependent.
///
/// The vertical run sits on a lane chosen by the dependent's row, so
/// two edges into different modules never share one vertical and read
/// as a single line; edges into the SAME module share a lane and merge,
/// which is the fan-in a reader expects to see.
///
/// Tone follows the prerequisite: green once it is done (the gate is
/// satisfied on this edge), amber while it is open; muted once the
/// dependent itself is done.
pub fn edges(modules: &[Module], layout: &GraphLayout) -> Vec<Segment> {
    let index: HashMap<&str, usize> = modules
        .iter()
        .enumerate()
        .map(|(i, m)| (m.id.as_str(), i))
        .collect();
    let lanes = ((COL_GAP - LANE_INSET * 2.0) / LANE_STEP).max(1.0) as usize;
    let mut out = Vec::new();
    for (ti, m) in modules.iter().enumerate() {
        let t = layout.slots[ti];
        for dep in &m.depends_on {
            let Some(&si) = index.get(dep.as_str()) else { continue };
            let s = layout.slots[si];
            if s.col >= t.col {
                continue;
            }
            let tone = if m.status == Status::Done {
                EdgeLineTone::Muted
            } else if modules[si].status == Status::Done {
                EdgeLineTone::Open
            } else {
                EdgeLineTone::Blocking
            };
            let x1 = layout.x(s.col) + CARD_W;
            let y1 = layout.y(s.row) + CARD_H / 2.0;
            let x2 = layout.x(t.col);
            let y2 = layout.y(t.row) + CARD_H / 2.0;
            let xm = x2 - COL_GAP + LANE_INSET + (t.row % lanes) as f32 * LANE_STEP;
            let half = LINE / 2.0;
            let id = out.len();
            out.push(Segment { id, x: x1, y: y1 - half, w: xm - x1 + LINE, h: LINE, tone });
            out.push(Segment {
                id: id + 1,
                x: xm,
                y: y1.min(y2) - half,
                w: LINE,
                h: (y2 - y1).abs() + LINE,
                tone,
            });
            out.push(Segment { id: id + 2, x: xm, y: y2 - half, w: x2 - xm, h: LINE, tone });
        }
    }
    out
}

/// An absolutely-positioned box inside the canvas.
fn abs_rules(x: f32, y: f32, w: f32, h: f32) -> StyleRules {
    let mut rules = StyleRules::default();
    rules.position = Some(Position::Absolute);
    rules.left = Some(Tokenized::Literal(Length::Px(x)));
    rules.top = Some(Tokenized::Literal(Length::Px(y)));
    rules.width = Some(Tokenized::Literal(Length::Px(w)));
    rules.height = Some(Tokenized::Literal(Length::Px(h)));
    rules
}

// The graph takes the pane rather than sizing to its diagram, so the
// horizontal scrollbar lands at the bottom edge.
stylesheet! {
    pub GraphScroll<IdeaThemeRef> {
        base(_t) {
            flex_grow: 1.0,
            min_height: 0,
            min_width: 0,
            flex_direction: FlexDirection::Column,
        }
    }
}

stylesheet! {
    pub GraphBlank<IdeaThemeRef> {
        base(t) {
            padding: t.spacing.xl(),
        }
    }
}

// No padding here — it is inside the canvas, so the scrollbar hugs the
// container edge (rule 3).
stylesheet! {
    pub StripScroll<IdeaThemeRef> {
        base(_t) {
            min_width: 0,
            flex_grow: 1.0,
            min_height: 0,
        }
    }
}

// Takes the strip's height and scrolls the canvas within it.
stylesheet! {
    pub CanvasScroll<IdeaThemeRef> {
        base(_t) {
            flex_grow: 1.0,
            min_height: 0,
        }
    }
}

// Fills its absolutely-positioned segment; the colour is the theme's.
stylesheet! {
    pub EdgeLine<IdeaThemeRef> {
        base(_t) {
            width: Length::Percent(100.0),
            height: Length::Percent(100.0),
        }
        variant tone {
            #[default]
            muted(t) { background: t.color.border() }
            open(t) { background: t.intent.success.fg() }
            blocking(t) { background: t.intent.warning.fg() }
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
            padding_top: t.spacing.md(),
            padding_bottom: t.spacing.xs(),
            flex_shrink: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Module, Status};

    fn module(id: &str, depth: usize, deps: &[&str], status: Status) -> Module {
        Module {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            status,
            agent: String::new(),
            spawned: String::new(),
            depends_on: deps.iter().map(|d| d.to_string()).collect(),
            waiting_on: Vec::new(),
            owns: Vec::new(),
            depth,
            dispatchable: false,
            handoff: None,
            summary: None,
            block: None,
            tasks: Vec::new(),
            history: Vec::new(),
        }
    }

    // The worked scenario: a chain that fans out at the end. The two
    // leaves share a column, sit under their common prerequisite, and
    // every edge runs left to right.
    #[test]
    fn columns_follow_depth_and_edges_run_rightwards() {
        let modules = vec![
            module("schema", 1, &[], Status::Done),
            module("api", 2, &["schema"], Status::Running),
            module("app", 3, &["api"], Status::Queued),
            module("mcp", 3, &["api"], Status::Queued),
        ];
        let layout = GraphLayout::compute(&modules);
        assert_eq!(layout.cols, 3);
        assert_eq!(layout.rows, 2);
        assert_eq!(layout.slots[0], Slot { col: 0, row: 0 });
        assert_eq!(layout.slots[1], Slot { col: 1, row: 0 });
        assert_eq!(layout.slots[2], Slot { col: 2, row: 0 });
        assert_eq!(layout.slots[3], Slot { col: 2, row: 1 });
        let segs = edges(&modules, &layout);
        assert_eq!(segs.len(), 9, "three segments per edge");
        for seg in &segs {
            assert!(seg.w > 0.0 && seg.h > 0.0, "{seg:?}");
        }
        // The satisfied edge is green; the open ones are amber.
        assert_eq!(segs[0].tone, EdgeLineTone::Open);
        assert_eq!(segs[3].tone, EdgeLineTone::Blocking);
        // Nothing pokes outside the canvas.
        for seg in &segs {
            assert!(seg.x + seg.w <= layout.width() && seg.y + seg.h <= layout.height());
        }
    }

    // Two dependents in the same column, on different rows, get their
    // own vertical lanes — otherwise A→D and B→C read as one line.
    #[test]
    fn edges_into_different_rows_do_not_share_a_lane() {
        let modules = vec![
            module("a", 1, &[], Status::Done),
            module("b", 1, &[], Status::Done),
            module("c", 2, &["b"], Status::Queued),
            module("d", 2, &["a"], Status::Queued),
        ];
        let layout = GraphLayout::compute(&modules);
        let segs = edges(&modules, &layout);
        let verticals: Vec<f32> = segs.iter().filter(|s| s.w == LINE).map(|s| s.x).collect();
        assert_eq!(verticals.len(), 2);
        assert_ne!(verticals[0], verticals[1]);
    }
}
