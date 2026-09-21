//! The module graph: a layered DAG. One column per depth — every
//! module sits right of everything it depends on — and an edge from
//! each prerequisite to what depends on it.
//!
//! The layout is computed here from known card geometry
//! ([`CARD_W`]/[`CARD_H`]) and the edges are absolutely-positioned
//! views inside one relatively-positioned canvas. A dependency that
//! skips a column is routed THROUGH it on a thin pass-through row of
//! its own, between the cards, rather than drawn under one; columns
//! are ordered by barycentre sweeps and each gap's lanes by crossing
//! cost, so the lines cross as little as a layered drawing allows.

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
pub const COL_GAP: f32 = 96.0;
/// Gap between cards in a column.
pub const ROW_GAP: f32 = 16.0;
/// The canvas's inner padding. Part of the canvas, not the scroller,
/// so the scrollbars hug the pane edge (rule 3).
pub const PAD: f32 = 24.0;
/// Edge stroke width.
const LINE: f32 = 2.0;
/// Horizontal spacing between the vertical lanes inside a column gap.
/// Wide enough that two lanes read as two lines, not one thick one.
const LANE_STEP: f32 = 8.0;
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
        // Two different blanks: the graph has not been read yet, or it
        // has and there is nothing in it.
        let word = if f.detail_loaded { "No modules planned." } else { "Loading the graph\u{2026}" };
        return ui! {
            view(style = GraphBlank()) {
                Typography(content = word, kind = typography_kind::BodySm, muted = true)
            }
        };
    }

    let layout = GraphLayout::compute(&f.modules);
    let segments = edges(&f.modules, &layout);
    let cards = layout.cards();
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
/// column, counting the pass-through rows of long edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    pub col: usize,
    pub row: usize,
}

/// One node of the layered graph: a module, or the point where a long
/// edge passes through a column it does not stop in. A pass-through
/// takes a thin row of its own, so the line crosses the column BETWEEN
/// cards rather than under one — and every long edge out of one
/// source shares that row through each column it crosses (a bus that
/// fans out only in the gap before its targets), so a module with
/// three far dependents costs one line across the middle, not three.
#[derive(Clone, Debug)]
struct Node {
    col: usize,
    row: usize,
    /// Top edge on the canvas; set once the column is ordered.
    y: f32,
    /// `Some` for a module, `None` for a pass-through.
    module: Option<usize>,
}

impl Node {
    fn h(&self) -> f32 {
        if self.module.is_some() { CARD_H } else { PASS_H }
    }
    fn mid(&self) -> f32 {
        self.y + self.h() / 2.0
    }
}

/// One hop of the layered graph, always between adjacent columns. A
/// direct dependency is one link; a dependency across k columns is k
/// links through k − 1 pass-through nodes, all carrying the same
/// `edge` so they draw in one tone.
#[derive(Clone, Copy, Debug)]
struct Link {
    from: usize,
    to: usize,
    edge: usize,
}

/// The computed placement of every module, indexed like
/// [`crate::model::Feature::modules`].
#[derive(Debug)]
pub struct GraphLayout {
    pub slots: Vec<Slot>,
    pub cols: usize,
    nodes: Vec<Node>,
    links: Vec<Link>,
    /// Tone per original edge, indexed by [`Link::edge`].
    tones: Vec<EdgeLineTone>,
    height: f32,
}

/// Height of a pass-through row: room for a line and its margins.
const PASS_H: f32 = 12.0;
/// How many barycentre sweeps order the columns: down, up, down.
const SWEEPS: usize = 3;

impl GraphLayout {
    /// Columns by depth, long edges split through pass-through rows,
    /// and each column ordered by a few barycentre sweeps (a node sits
    /// near the average row of its neighbours, first looking left,
    /// then right, then left again) so chains run straight, fan-outs
    /// sit beside their source, and lines cross as little as a layered
    /// drawing lets them. Modules arrive topologically sorted.
    pub fn compute(modules: &[Module]) -> Self {
        let index: HashMap<&str, usize> = modules
            .iter()
            .enumerate()
            .map(|(i, m)| (m.id.as_str(), i))
            .collect();
        let cols = modules.iter().map(|m| m.depth).max().unwrap_or(1);
        let mut nodes: Vec<Node> = modules
            .iter()
            .enumerate()
            .map(|(i, m)| Node { col: m.depth - 1, row: 0, y: 0.0, module: Some(i) })
            .collect();
        let mut links: Vec<Link> = Vec::new();
        let mut tones: Vec<EdgeLineTone> = Vec::new();
        // The bus rows: one pass-through per (source, column crossed).
        let mut buses: HashMap<(usize, usize), usize> = HashMap::new();
        for (ti, m) in modules.iter().enumerate() {
            for dep in &m.depends_on {
                let Some(&si) = index.get(dep.as_str()) else { continue };
                let (sc, tc) = (nodes[si].col, nodes[ti].col);
                if sc >= tc {
                    continue;
                }
                let edge = tones.len();
                tones.push(if m.status == Status::Done {
                    EdgeLineTone::Muted
                } else if modules[si].status == Status::Done {
                    EdgeLineTone::Open
                } else {
                    EdgeLineTone::Blocking
                });
                let mut from = si;
                for col in sc + 1..tc {
                    let d = *buses.entry((si, col)).or_insert_with(|| {
                        nodes.push(Node { col, row: 0, y: 0.0, module: None });
                        nodes.len() - 1
                    });
                    // The hop onto a shared bus is drawn once, in the
                    // tone of the first edge that needed it.
                    if !links.iter().any(|l| l.from == from && l.to == d) {
                        links.push(Link { from, to: d, edge });
                    }
                    from = d;
                }
                links.push(Link { from, to: ti, edge });
            }
        }

        // Column membership in plan order, then the sweeps.
        let mut by_col: Vec<Vec<usize>> = vec![Vec::new(); cols];
        for (i, n) in nodes.iter().enumerate() {
            by_col[n.col].push(i);
        }
        let set_rows = |nodes: &mut Vec<Node>, by_col: &[Vec<usize>]| {
            for members in by_col {
                for (row, &i) in members.iter().enumerate() {
                    nodes[i].row = row;
                }
            }
        };
        set_rows(&mut nodes, &by_col);
        for sweep in 0..SWEEPS {
            let down = sweep % 2 == 0;
            let order: Vec<usize> = if down { (1..cols).collect() } else { (0..cols.saturating_sub(1)).rev().collect() };
            for col in order {
                let key = |i: usize| -> f32 {
                    let rows: Vec<f32> = links
                        .iter()
                        .filter(|l| if down { l.to == i } else { l.from == i })
                        .map(|l| nodes[if down { l.from } else { l.to }].row as f32)
                        .collect();
                    if rows.is_empty() {
                        // Nothing to lean on this way: stay put.
                        nodes[i].row as f32
                    } else {
                        rows.iter().sum::<f32>() / rows.len() as f32
                    }
                };
                // Stable, so ties keep their current order.
                by_col[col].sort_by(|&a, &b| key(a).total_cmp(&key(b)));
                for (row, &i) in by_col[col].iter().enumerate() {
                    nodes[i].row = row;
                }
            }
        }

        // Positions: each column stacks its own rows, a card's height
        // for a module and a sliver for a pass-through.
        let mut height: f32 = 0.0;
        for members in &by_col {
            let mut y = PAD;
            for &i in members {
                nodes[i].y = y;
                y += nodes[i].h() + ROW_GAP;
            }
            height = height.max(y - ROW_GAP + PAD);
        }
        let slots = (0..modules.len()).map(|i| Slot { col: nodes[i].col, row: nodes[i].row }).collect();
        GraphLayout { slots, cols, nodes, links, tones, height }
    }

    /// Left edge of a column's cards.
    pub fn x(&self, col: usize) -> f32 {
        PAD + col as f32 * (CARD_W + COL_GAP)
    }

    /// Top edge of a module's card.
    pub fn y_of(&self, module: usize) -> f32 {
        self.nodes[module].y
    }

    /// Every module's card at its place.
    pub fn cards(&self) -> Vec<CardSlot> {
        (0..self.slots.len())
            .map(|module| CardSlot { module, x: self.x(self.slots[module].col), y: self.y_of(module) })
            .collect()
    }

    /// Canvas width, padding included.
    pub fn width(&self) -> f32 {
        PAD * 2.0 + self.cols as f32 * CARD_W + (self.cols - 1) as f32 * COL_GAP
    }

    /// Canvas height, padding included.
    pub fn height(&self) -> f32 {
        self.height
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

/// A trunk: every link into one node of the next column, sharing the
/// vertical lane in the gap before it. Fan-in merges into one line,
/// which is what a reader expects to see.
struct Trunk {
    to: usize,
    /// Centre y of the target.
    yt: f32,
    /// Centre y of each source, with its link.
    stubs: Vec<(f32, Link)>,
}

impl Trunk {
    fn top(&self) -> f32 {
        self.stubs.iter().map(|(y, _)| *y).fold(self.yt, f32::min)
    }
    fn bottom(&self) -> f32 {
        self.stubs.iter().map(|(y, _)| *y).fold(self.yt, f32::max)
    }
    fn spans(&self, y: f32) -> bool {
        y > self.top() + 0.5 && y < self.bottom() - 0.5
    }
}

/// Crossings caused by putting `a`'s lane left of `b`'s: `a`'s final
/// run into its target crosses `b`'s vertical when it passes through
/// it, and each of `b`'s source stubs crosses `a`'s vertical when it
/// passes through that.
fn cost(a: &Trunk, b: &Trunk) -> usize {
    usize::from(b.spans(a.yt)) + b.stubs.iter().filter(|(y, _)| a.spans(*y)).count()
}

/// Lane order for one gap: start by target height, then place each
/// trunk where it adds the fewest crossings against those already
/// placed. Gaps hold a handful of trunks, so this stays cheap.
fn lane_order(trunks: &mut Vec<Trunk>) {
    trunks.sort_by(|a, b| a.yt.total_cmp(&b.yt));
    let mut placed: Vec<Trunk> = Vec::with_capacity(trunks.len());
    for t in trunks.drain(..) {
        let mut best = (usize::MAX, 0);
        for at in 0..=placed.len() {
            let c: usize = placed[..at].iter().map(|p| cost(p, &t)).sum::<usize>()
                + placed[at..].iter().map(|p| cost(&t, p)).sum::<usize>();
            if c < best.0 {
                best = (c, at);
            }
        }
        placed.insert(best.1, t);
    }
    *trunks = placed;
}

/// Every link as three segments — out of its source, down or up the
/// gap on its trunk's lane, into its target — plus, for a pass-through,
/// the run across the column it crosses. Lanes are assigned per gap by
/// [`lane_order`], and a gap that needs more lanes than it has room
/// for packs them closer rather than doubling up.
///
/// Tone follows the prerequisite: green once it is done (the gate is
/// satisfied on this edge), amber while it is open; muted once the
/// dependent itself is done.
pub fn edges(_modules: &[Module], layout: &GraphLayout) -> Vec<Segment> {
    let half = LINE / 2.0;
    let mut out: Vec<Segment> = Vec::new();
    let push = |x: f32, y: f32, w: f32, h: f32, tone: EdgeLineTone, out: &mut Vec<Segment>| {
        let id = out.len();
        out.push(Segment { id, x, y, w, h, tone });
    };
    for gap in 0..layout.cols.saturating_sub(1) {
        let x1 = layout.x(gap) + CARD_W;
        let x2 = layout.x(gap + 1);
        let mut trunks: Vec<Trunk> = Vec::new();
        for l in layout.links.iter().filter(|l| layout.nodes[l.from].col == gap) {
            let ys = layout.nodes[l.from].mid();
            match trunks.iter_mut().find(|t| t.to == l.to) {
                Some(t) => t.stubs.push((ys, *l)),
                None => trunks.push(Trunk { to: l.to, yt: layout.nodes[l.to].mid(), stubs: vec![(ys, *l)] }),
            }
        }
        lane_order(&mut trunks);
        let room = COL_GAP - LANE_INSET * 2.0;
        let step = if trunks.len() > 1 { LANE_STEP.min(room / (trunks.len() - 1) as f32) } else { 0.0 };
        for (lane, t) in trunks.iter().enumerate() {
            let xm = x2 - COL_GAP + LANE_INSET + lane as f32 * step;
            for (ys, l) in &t.stubs {
                let tone = layout.tones[l.edge];
                push(x1, ys - half, xm - x1 + LINE, LINE, tone, &mut out);
                push(xm, ys.min(t.yt) - half, LINE, (t.yt - ys).abs() + LINE, tone, &mut out);
                push(xm, t.yt - half, x2 - xm, LINE, tone, &mut out);
                // A pass-through keeps going across its column.
                if layout.nodes[t.to].module.is_none() {
                    push(x2, t.yt - half, CARD_W, LINE, tone, &mut out);
                }
            }
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
            summary: None,
            block: None,
            open_questions: Vec::new(),
            last_word: None,
            quiet_secs: -1,
            tasks: Vec::new(),
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

    // A dependency that skips a column is routed through it on a row of
    // its own: the line runs between the cards of that column, never
    // under one, and every card keeps a clear interior.
    #[test]
    fn a_long_edge_passes_between_cards_not_under_them() {
        let modules = vec![
            module("a", 1, &[], Status::Done),
            module("b", 2, &["a"], Status::Done),
            module("c", 3, &["a", "b"], Status::Queued),
        ];
        let layout = GraphLayout::compute(&modules);
        // Column 1 holds b and the pass-through of a→c, so the canvas is
        // taller than one card.
        assert!(layout.height() > PAD * 2.0 + CARD_H + ROW_GAP);
        let segs = edges(&modules, &layout);
        // a→b: 3; a→c: 3 into the pass-through, 1 across, 3 into c; b→c: 3.
        assert_eq!(segs.len(), 13);
        assert_no_segment_crosses_a_card(&modules, &layout, &segs);
    }

    // The real shape that motivated the router: twelve modules over six
    // columns with several two- and three-column dependencies. The
    // invariant is the same — no line under a card — and every lane
    // in a gap is distinct.
    #[test]
    fn the_divisions_feature_routes_clean() {
        let modules = vec![
            module("division-schema", 1, &[], Status::Done),
            module("catalogs-by-division", 2, &["division-schema"], Status::Done),
            module("projects-and-members", 2, &["division-schema"], Status::Done),
            module("division-filter-dim", 3, &["projects-and-members"], Status::Done),
            module("mcp-tools", 3, &["catalogs-by-division", "projects-and-members"], Status::Running),
            module("permission-scope", 3, &["catalogs-by-division", "projects-and-members"], Status::Running),
            module("crew-screens", 4, &["catalogs-by-division", "projects-and-members", "division-filter-dim"], Status::Queued),
            module("projects-division", 4, &["projects-and-members", "division-filter-dim"], Status::Running),
            module("settings-screens", 4, &["catalogs-by-division", "division-filter-dim"], Status::Running),
            module("noun-by-division", 5, &["projects-division", "settings-screens"], Status::Queued),
            module("permission-group-ui", 5, &["crew-screens", "permission-scope"], Status::Queued),
            module("gate-and-suite", 6, &["crew-screens", "projects-division", "permission-group-ui", "mcp-tools", "settings-screens", "noun-by-division"], Status::Queued),
        ];
        let layout = GraphLayout::compute(&modules);
        assert_eq!(layout.cols, 6);
        let segs = edges(&modules, &layout);
        assert_no_segment_crosses_a_card(&modules, &layout, &segs);
        // Within one gap, distinct targets sit on distinct lanes.
        for gap in 0..layout.cols - 1 {
            let x_lo = layout.x(gap) + CARD_W;
            let x_hi = layout.x(gap + 1);
            let mut lanes: Vec<f32> = segs
                .iter()
                .filter(|s| s.w == LINE && s.x > x_lo && s.x < x_hi)
                .map(|s| s.x)
                .collect();
            lanes.sort_by(f32::total_cmp);
            lanes.dedup();
            let targets = layout
                .links
                .iter()
                .filter(|l| layout.nodes[l.from].col == gap)
                .map(|l| l.to)
                .collect::<std::collections::HashSet<_>>();
            assert_eq!(lanes.len(), targets.len(), "gap {gap}: one lane per target");
            for pair in lanes.windows(2) {
                assert!(pair[1] - pair[0] >= LINE + 2.0, "gap {gap}: lanes touch: {lanes:?}");
            }
        }
        for seg in &segs {
            assert!(seg.x >= 0.0 && seg.y >= 0.0);
            assert!(seg.x + seg.w <= layout.width() && seg.y + seg.h <= layout.height(), "{seg:?}");
        }
    }

    // Two far dependents of one source share its pass-through row and
    // split only in the gap before their column.
    #[test]
    fn long_edges_from_one_source_share_a_bus() {
        let modules = vec![
            module("a", 1, &[], Status::Done),
            module("b", 2, &["a"], Status::Done),
            module("c", 3, &["a", "b"], Status::Queued),
            module("d", 3, &["a", "b"], Status::Queued),
        ];
        let layout = GraphLayout::compute(&modules);
        // Column 1: b and ONE bus for a's two far edges.
        assert_eq!(layout.nodes.iter().filter(|n| n.col == 1).count(), 2);
        let segs = edges(&modules, &layout);
        assert_no_segment_crosses_a_card(&modules, &layout, &segs);
        // a→b 3; a→bus 3 + 1 across; bus→c 3; bus→d 3; b→c 3; b→d 3.
        assert_eq!(segs.len(), 19);
    }

    fn assert_no_segment_crosses_a_card(modules: &[Module], layout: &GraphLayout, segs: &[Segment]) {
        for card in layout.cards() {
            let (l, t, r, b) = (card.x, card.y, card.x + CARD_W, card.y + CARD_H);
            for s in segs {
                let (sl, st, sr, sb) = (s.x, s.y, s.x + s.w, s.y + s.h);
                let overlaps = sl < r - 0.5 && sr > l + 0.5 && st < b - 0.5 && sb > t + 0.5;
                assert!(!overlaps, "segment {s:?} crosses card '{}'", modules[card.module].name);
            }
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
