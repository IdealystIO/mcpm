//! `app()` — composes the MCP Project Console: masthead, feature rail,
//! main pane, and the right-hand detail drawers — and keeps it live: the
//! console polls the mcpm-web server (which reads the same Postgres
//! store the MCP tools write) and re-renders whenever the data changes.

use idea_ui::{dark_theme, install_idea_theme_reactive, light_theme, IdeaThemeRef};
use idea_ui_nav::AppShell;
use runtime_core::{presence, raf_loop_scoped, spawn_then, stylesheet, switch, ui, Breakpoint,
    Easing, Element, FlexDirection, IntoElement, Position, PresenceAnim};

use crate::components::drawer::{Drawer, WantDrawer};
use crate::components::gate::KeyGate;
use crate::components::header::Header;
use crate::components::knowledge::KnowledgeDrawer;
use crate::components::main_pane::MainPane;
use crate::components::sidebar::Sidebar;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::model;
use crate::state::{use_console_live, Console};

/// Where the mcpm-web API listens
/// (`cargo run -p api --bin mcpm-web --features server`).
///
/// Baked into the wasm at build time, so it must be set for any build
/// served from somewhere other than the machine running `mcpm-web`:
/// the default points every visitor's browser at THEIR OWN loopback,
/// where the page loads perfectly and every call fails against nothing
/// — on the visitor's machine, so the server logs show a healthy host
/// with no traffic. Set `MCPM_API_ORIGIN` to the deployment's own
/// origin when building a hosted console; the devcontainer path needs
/// nothing.
const API_ORIGIN: &str = match option_env!("MCPM_API_ORIGIN") {
    Some(origin) => origin,
    None => "http://127.0.0.1:3210",
};
/// Poll cadence — the FALLBACK, not the primary path. Every committed
/// event arrives over `watch_events` within a frame or two; this is
/// what keeps the console correct if that socket is down (the host
/// restarted, a proxy dropped the upgrade, LISTEN failed). A poll
/// refreshes everything on screen, as if a tick had named all of it.
const POLL_MICROS: u64 = 30_000_000;
/// How many ledger entries one page of a feature's activity feed holds.
const FEED_PAGE: i64 = 50;
/// How many wants one page of the pool holds. The server's
/// `search_wants` pages by the same number; the wants screen's pager
/// reads it from here.
pub const POOL_PAGE: usize = 20;

// Drawer motion. The backdrop fades over the whole overlay; the panel
// itself slides (see `drawer::panel_motion`). Out is quicker than in —
// a dismissal should feel like it obeyed you, not like it is being
// reconsidered.
/// Backdrop fade-in.
pub const BACKDROP_IN_MS: u32 = 180;
/// Backdrop fade-out.
pub const BACKDROP_OUT_MS: u32 = 140;

/// The nav's width, in px. Owned here because `AppShell` bakes it into
/// its panel and content-offset sheets — the rail reads the same
/// constant rather than declaring a second one that could drift.
pub const NAV_WIDTH: f32 = 232.0;

pub fn app() -> Element {
    let console = use_console_live();
    install_idea_theme_reactive(move || {
        if console.dark.get() {
            dark_theme()
        } else {
            light_theme()
        }
    });

    // The sync is keyed on the key: entering, rotating or forgetting
    // one has to re-open the event socket, and the socket's credential
    // is baked into its connect URL at open time. The switch's scope
    // teardown is what closes the old one — `raf_loop_scoped` and the
    // subscription both die with the scope that made them, so a key
    // change leaves nothing of the previous session running.
    let sync = switch(
        move || console.api_key.get(),
        move |key: &String| {
            start_sync(console, key.clone());
            ui! { view {} }
        },
    );

    // The drawer overlays the whole page. `presence` owns the
    // mount/unmount timing so the close animation can finish before the
    // subtree drops — a plain `switch` on the open flag would tear it
    // down on the same frame, leaving no window for an exit. The inner
    // `switch` is keyed on the STICKY target so the panel keeps drawing
    // itself while it slides away.
    // The module is held by id and resolved to its index in the
    // feature's graph here, on every rebuild: the graph may not have
    // loaded yet when the drawer is asked for (the home screen's
    // attention list opens into any feature), and it arrives a rev
    // later.
    let drawer_host = presence(move || {
        switch(
            move || (console.feature.get(), console.last_module.get(), console.rev.get()),
            move |(fi, sel, _rev): &(usize, Option<String>, u64)| {
                match sel.as_deref().and_then(|id| drawer_target(*fi, id)) {
                    Some(mi) => ui! { Drawer(console = console, feature = *fi, module = mi) },
                    None => ui! { view {} },
                }
            },
        )
    })
    .present(move || console.selected.get().is_some())
    .enter(PresenceAnim::fade(BACKDROP_IN_MS, Easing::EaseOut))
    .exit(PresenceAnim::fade(BACKDROP_OUT_MS, Easing::EaseIn))
    .into_element();

    // The want drawer shares the module drawer's slot; `Console` keeps
    // at most one of the two targets set. Keyed on the want **id**:
    // the pool is paged and re-sorted under the reader, and an origin
    // row can open an idea no loaded page holds — the sync loop
    // fetches it by id and the panel reads it back the same way.
    let want_host = presence(move || {
        switch(
            move || (console.last_want.get(), console.rev.get()),
            move |(id, _rev): &(Option<String>, u64)| match id {
                Some(id) if model::want_by_id(id).is_some() => ui! {
                    WantDrawer(console = console, want = id.clone())
                },
                _ => ui! { view {} },
            },
        )
    })
    .present(move || console.want.get().is_some())
    .enter(PresenceAnim::fade(BACKDROP_IN_MS, Easing::EaseOut))
    .exit(PresenceAnim::fade(BACKDROP_OUT_MS, Easing::EaseIn))
    .into_element();

    // The gate replaces the whole body, sidebar included: with the host
    // refusing us there is no data behind it to show, and a rail of
    // empty cards next to a "give me a key" card would only suggest
    // there is something to go back to.
    //
    // Refusal is the ONLY thing that reaches it. Changing a key while
    // the host is still answering is a one-field edit and happens in
    // the masthead's popover — see `Console::show_key`.
    let body = switch(
        move || (console.pane.get(), console.denied.get()),
        move |state: &(String, bool)| {
            let (_pane, denied) = state.clone();
            if denied {
                return ui! { KeyGate(console = console) };
            }
            // `AppShell` is chrome, not navigation: the console's
            // destinations still live on `Console::pane`. What it
            // supplies is the one shape this layout had hand-rolled —
            // the nav pinned in flow on a wide viewport and slid in
            // over a scrim below `pin_at`, with the panel built ONCE so
            // crossing the breakpoint never remounts the rail or
            // restarts anything it had in flight.
            ui! {
                AppShell(
                    sidebar = vec![ui! { Sidebar(console = console) }],
                    is_open = console.nav_open,
                    pin_at = Breakpoint::Lg,
                    width = NAV_WIDTH,
                ) {
                    view(style = ContentFrame()) {
                        MainPane(console = console)
                    }
                }
            }
        },
    );

    // The knowledge drawer shares the overlay slot with the other two.
    // Keyed on the memory id — the base re-ranks under a poll, so an
    // index would slide onto a different entry mid-read.
    let knowledge_host = presence(move || {
        switch(
            move || console.know_open.get(),
            move |id: &Option<String>| match id {
                Some(id) => ui! { KnowledgeDrawer(console = console, memory_id = id.clone()) },
                None => ui! { view {} },
            },
        )
    })
    .present(move || console.know_open.get().is_some())
    .enter(PresenceAnim::fade(BACKDROP_IN_MS, Easing::EaseOut))
    .exit(PresenceAnim::fade(BACKDROP_OUT_MS, Easing::EaseIn))
    .into_element();

    ui! {
        view(style = PageFrame()) {
            Header(console = console)
            view(style = ShellSlot()) { body }
            drawer_host
            want_host
            knowledge_host
            sync
        }
    }
}

fn drawer_target(fi: usize, module_id: &str) -> Option<usize> {
    model::features().get(fi)?.module_index(module_id)
}

/// Which reads the screen needs loaded right now. Computed every frame
/// from `Console`, so a fetch is exactly the consequence of something
/// being on screen — no view spawns one of its own.
#[derive(Clone, PartialEq, Eq, Default)]
struct Wanted {
    /// The selected feature's graph, when the feature pane is showing.
    feature: Option<String>,
    /// The open module's drawer contents.
    module: Option<String>,
    /// The selected feature's feed, when its activity tab is showing.
    feed: Option<String>,
    /// The pool page the wants screen is showing.
    pool: Option<PoolKey>,
    /// The idea whose drawer is open.
    want: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Default)]
struct PoolKey {
    query: String,
    status: String,
    tags: Vec<String>,
    page: usize,
}

/// One read that is on its way, and when it left. A fetch answers (or
/// fails) well within a poll window; one older than that is treated as
/// dropped so a lost callback cannot wedge its slot forever.
type InFlight = Rc<RefCell<HashMap<String, u64>>>;

/// Configure the RPC origin, open the event subscription, and drive
/// every fetch from a raf clock (scope-anchored, so both die with the
/// scope that called this).
///
/// This is the console's one scheduler. Each frame it works out what
/// the screen needs ([`Wanted`]), what the model already holds, and
/// what a tick has made stale — and issues the difference:
///
/// - the **board** is fetched on every tick, poll, or local write. It
///   is a row of counts per feature and never grows past that.
/// - the **selected feature** is fetched when the selection changes,
///   and refetched only for a tick that names it: an event on another
///   feature moves its counts on the board and leaves the open tree
///   alone. The open module's drawer and the visible feed follow the
///   same rule.
/// - a **pool page** is fetched when its filters or page change, and
///   refetched for a tick on a want or a tag.
/// - the **open want** is fetched by id when the drawer opens.
///
/// A tick's `feature_id` and `kind` are what make the split possible;
/// a tick without them (a database whose trigger predates migration
/// 0013) is treated as touching everything, which is what every tick
/// did before.
///
/// `key` is empty against an open loopback host, which sends no
/// `Authorization` at all — the gate on that host does not ask for one,
/// and sending an empty bearer would turn a fine request into a 401.
fn start_sync(console: Console, key: String) {
    let credential = key.clone();
    server::configure(
        server::ClientConfig::new(API_ORIGIN).with_credentials(server::bearer(move || {
            Some(credential.clone()).filter(|k| !k.is_empty())
        })),
    );
    // Scope-bound: the socket closes when this scope is torn down.
    // The key rides the connect URL because a browser cannot put a
    // header on a WebSocket handshake — see `api::watch_events`.
    let events = api::watch_events(key);
    // `None` means "fetch on the next frame"; `Some(t)` is when the
    // last poll started. Deliberately not `0`: on web `now_micros`
    // counts from page load, so for the first POLL_MICROS after load
    // `now - 0` is still inside the window — a zero sentinel would gate
    // off the very first fetch, and any event tick or local capture
    // that lands while the page is young, until the window elapsed.
    let mut last_poll: Option<u64> = None;
    let mut seen_nudge: u64 = 0;
    let mut seen_seq: i64 = 0;
    let mut last_wanted = Wanted::default();
    // What a tick (or the poll) has invalidated since it was last read.
    let mut stale_board = true;
    let mut stale_feature = true;
    let mut stale_pool = true;
    let mut stale_want = true;
    let in_flight: InFlight = Rc::new(RefCell::new(HashMap::new()));

    raf_loop_scoped(move || {
        let now = runtime_core::time::now_micros();
        // Report the real connection, not "a snapshot landed once".
        console.connected.set(matches!(events.status(), server::SocketStatus::Open));

        // --- What is on screen ------------------------------------
        let pane = console.pane.get();
        let fi = console.feature.get();
        let feature_id = model::feature_id_at(fi);
        let on_feature = pane == "feature" && feature_id.is_some();
        let wanted = Wanted {
            feature: on_feature.then(|| feature_id.clone()).flatten(),
            module: on_feature.then(|| console.selected.get()).flatten(),
            feed: (on_feature && console.view.get() == "feed")
                .then(|| feature_id.clone())
                .flatten(),
            pool: (pane == "wants").then(|| PoolKey {
                query: console.pool_query.get(),
                status: console.pool_status.get(),
                tags: console.pool_tags.get(),
                page: console.pool_page.get(),
            }),
            want: console.want.get(),
        };

        // --- What has changed under us ----------------------------
        // An event committed anywhere — an agent's write as much as our
        // own. Seq-keyed, so a replayed notification is not a second
        // fetch.
        if let Some(tick) = events.latest() {
            if tick.seq > seen_seq {
                seen_seq = tick.seq;
                stale_board = true;
                // No kind means an old trigger: assume the worst.
                let untyped = tick.kind.is_empty();
                if untyped || tick.feature_id == feature_id {
                    stale_feature = true;
                }
                if untyped || tick.kind.starts_with("want") || tick.kind.starts_with("tag") {
                    stale_pool = true;
                    stale_want = true;
                }
            }
        }
        // A local write (the capture composer) bumps `refresh` so its
        // result shows up without waiting for the round trip.
        let nudge = console.refresh.get();
        if nudge != seen_nudge {
            seen_nudge = nudge;
            stale_board = true;
            stale_pool = true;
        }
        // The fallback poll refreshes everything, as a tick naming all
        // of it would.
        if last_poll.is_none_or(|t| now.saturating_sub(t) >= POLL_MICROS) {
            last_poll = Some(now);
            stale_board = true;
            stale_feature = true;
            stale_pool = true;
            stale_want = true;
        }
        // A selection change is stale by definition: the new target
        // may be cached, but it has not been refreshed since it was
        // last looked at.
        if wanted.feature != last_wanted.feature {
            stale_feature = true;
        }
        if wanted.pool != last_wanted.pool {
            stale_pool = true;
        }
        if wanted.want != last_wanted.want {
            stale_want = true;
        }
        // The feed's "load older" is a one-shot request, not a state.
        let older = console.feed_older.get();
        if older.is_some() {
            console.feed_older.set(None);
        }
        last_wanted = wanted.clone();

        // --- Issue what is due ------------------------------------
        let started = |slot: &str| -> bool { claim(&in_flight, now, slot) };
        let done = |slot: String| {
            let pending = in_flight.clone();
            move || {
                pending.borrow_mut().remove(&slot);
            }
        };
        let bump = move |changed: bool| {
            if changed {
                console.rev.update(|r| r + 1);
            }
        };

        if stale_board && started("board") {
            stale_board = false;
            let release = done("board".into());
            spawn_then(api::load_board(), move |result| {
                release();
                match result {
                    Ok(board) => {
                        // A fetch that lands is proof the key (or the
                        // lack of one) is accepted, so the gate clears
                        // itself rather than waiting to be dismissed.
                        console.denied.set(false);
                        // Only the FIRST board moves the selection. A
                        // later one must not steal the feature the
                        // reader is on because a newer one arrived.
                        let first_load = !model::loaded();
                        if model::apply_board(board) {
                            if first_load {
                                if let Some(open) = model::first_open_feature() {
                                    console.feature.set(open);
                                }
                            }
                            bump(true);
                        }
                    }
                    // A refusal is a condition the reader can act on —
                    // it ends when they paste a working key — so it
                    // reaches the screen. Every other failure is
                    // transient and the next poll retries it, so it
                    // stays a log line: the screen already says
                    // "connecting", and a poll that fails forever with
                    // no trace anywhere is undiagnosable.
                    Err(err) => {
                        if matches!(err, server::ServerError::Server { status: 401, .. }) {
                            console.denied.set(true);
                        }
                        runtime_core::log_warn!("board fetch failed: {err:?}");
                    }
                }
            });
        }

        if let Some(id) = wanted.feature.clone() {
            let due = stale_feature || !model::has_detail(&id);
            if due && started("feature") {
                stale_feature = false;
                let release = done("feature".into());
                // The drawer and the feed refresh with their feature.
                let module = wanted.module.clone();
                let feed = wanted.feed.clone();
                spawn_then(api::load_feature(id.clone()), move |result| {
                    release();
                    match result {
                        Ok(detail) => bump(model::apply_feature(detail)),
                        Err(err) => runtime_core::log_warn!("feature fetch failed: {err:?}"),
                    }
                });
                if let Some(mid) = module {
                    fetch_module(console, &in_flight, now, mid);
                }
                if let Some(fid) = feed {
                    fetch_feed(console, &in_flight, now, fid, None);
                }
            }
        }
        // A drawer or feed opened onto something already fresh.
        if let Some(mid) = wanted.module.clone() {
            if model::module_detail(&mid).is_none() {
                fetch_module(console, &in_flight, now, mid);
            }
        }
        if let Some(fid) = wanted.feed.clone() {
            if model::feed(&fid).is_none() {
                fetch_feed(console, &in_flight, now, fid, None);
            }
        }
        if let Some(fid) = older {
            let cursor = model::feed(&fid).and_then(|f| f.oldest());
            if cursor.is_some() {
                fetch_feed(console, &in_flight, now, fid, cursor);
            }
        }

        if let Some(key) = wanted.pool.clone() {
            if stale_pool && started("pool") {
                stale_pool = false;
                let release = done("pool".into());
                let asked = key.page;
                spawn_then(
                    api::search_wants(key.query, key.status, key.tags, key.page as i64),
                    move |result| {
                        release();
                        match result {
                            Ok(page) => {
                                // A tick can shrink the pool under the
                                // page the reader is on. Step back to
                                // the last page that exists — a signal
                                // change, so the loop refetches it.
                                let last = (page.total.max(0) as usize).div_ceil(POOL_PAGE).max(1) - 1;
                                if asked > last {
                                    console.set_pool_page(last);
                                }
                                bump(model::set_want_page(page));
                            }
                            Err(err) => runtime_core::log_warn!("pool fetch failed: {err:?}"),
                        }
                    },
                );
            }
        }

        if let Some(id) = wanted.want.clone() {
            if stale_want && started("want") {
                stale_want = false;
                let release = done("want".into());
                spawn_then(api::load_want(id), move |result| {
                    release();
                    match result {
                        Ok(want) => bump(model::set_open_want(want)),
                        Err(err) => runtime_core::log_warn!("want fetch failed: {err:?}"),
                    }
                });
            }
        }
    });
}

/// Fetch one module's drawer contents, unless that read is already on
/// its way.
fn fetch_module(console: Console, in_flight: &InFlight, now: u64, module_id: String) {
    let slot = format!("module:{module_id}");
    if !claim(in_flight, now, &slot) {
        return;
    }
    let pending = in_flight.clone();
    spawn_then(api::load_module(module_id), move |result| {
        pending.borrow_mut().remove(&slot);
        match result {
            Ok(detail) => {
                if model::apply_module(detail) {
                    console.rev.update(|r| r + 1);
                }
            }
            Err(err) => runtime_core::log_warn!("module fetch failed: {err:?}"),
        }
    });
}

/// Fetch one page of a feature's feed: the newest page when `before`
/// is `None`, the page older than that seq otherwise.
fn fetch_feed(console: Console, in_flight: &InFlight, now: u64, feature_id: String, before: Option<i64>) {
    let slot = format!("feed:{feature_id}:{}", before.unwrap_or(0));
    if !claim(in_flight, now, &slot) {
        return;
    }
    let pending = in_flight.clone();
    spawn_then(
        api::load_events(feature_id, before.unwrap_or(0), FEED_PAGE),
        move |result| {
            pending.borrow_mut().remove(&slot);
            match result {
                Ok(page) => {
                    if model::apply_events(page) {
                        console.rev.update(|r| r + 1);
                    }
                }
                Err(err) => runtime_core::log_warn!("feed fetch failed: {err:?}"),
            }
        },
    );
}

/// Take a fetch slot, unless a fresh read already holds it.
fn claim(in_flight: &InFlight, now: u64, slot: &str) -> bool {
    let mut pending = in_flight.borrow_mut();
    match pending.get(slot) {
        Some(&t) if now.saturating_sub(t) < POLL_MICROS => false,
        _ => {
            pending.insert(slot.to_string(), now);
            true
        }
    }
}

// The two sheets below restore what `BodyRow` used to supply before
// `AppShell` took over this slot. Both exist for one reason, and it is a
// reason nothing here reports: the web backend dropped its global
// `.ui-default { display: flex }` baseline for per-node layout cost, so
// a node is a flex CONTAINER only when its own rules carry a
// flex-container property (`flex_direction`, `gap`, `justify_content`,
// `align_items`, …). `flex_grow`, `flex_basis` and `min_height` are flex
// ITEM properties and promote nothing. A column of `flex_grow: 1.0`
// views under a parent that never became `display: flex` therefore
// sizes to its content instead of to the viewport — every `scroll_view`
// below it grows to fit its rows rather than clamping, so nothing ever
// overflows, so nothing ever scrolls. It builds, it lints, and it
// renders a screen that looks right until the data is taller than the
// window.

// The shell's slot in the page column.
//
// `AppShell`'s own container is `height: 100%`, which resolves against
// the WHOLE page rather than against what is left under the masthead —
// so the shell needs a parent that is already the right size. This is
// that parent: it takes the leftover height as a flex item, and hands
// the shell a definite height to be 100% of.
stylesheet! {
    pub ShellSlot<IdeaThemeRef> {
        base(_t) {
            flex_grow: 1.0,
            // `flex_basis: 0` and not the default `auto`: with `auto`
            // this slot's base size is its CONTENT, so a pane taller
            // than the window makes the page column overflow and the
            // masthead is shrunk to pay for it. At zero the slot is
            // purely the leftover height, which is the one thing a
            // scrolling descendant can be measured against.
            flex_basis: 0,
            // Lets the scrolling descendants be shorter than their
            // content — without it nothing scrolls at all (rule 23).
            min_height: 0,
            min_width: 0,
            flex_direction: FlexDirection::Column,
        }
    }
}

// The flex context inside `AppShell`'s content wrapper.
//
// That wrapper sets `height: 100%` and `min_height: 0` and no
// flex-container property, so it is a BLOCK, and `MainPane`'s
// `flex_grow: 1.0` root is inert inside it. `AppShell` fixed exactly
// this for its own sidebar panel — see the `flex_direction` on its
// panel sheet and the regression test beside it — but not for the
// content half, so the fix has to live on our side of the boundary.
stylesheet! {
    pub ContentFrame<IdeaThemeRef> {
        base(_t) {
            height: runtime_core::Length::Percent(100.0),
            min_height: 0,
            min_width: 0,
            flex_direction: FlexDirection::Column,
        }
    }
}

stylesheet! {
    pub PageFrame<IdeaThemeRef> {
        base(t) {
            position: Position::Relative,
            width: runtime_core::Length::Percent(100.0),
            height: runtime_core::Length::Percent(100.0),
            flex_direction: FlexDirection::Column,
            background: t.color.background(),
            overflow: runtime_core::Overflow::Hidden,
        }
    }
}

#[cfg(test)]
mod tests {
    use runtime_core::{resolve_style, StyleApplication, Tokenized};

    fn shrink(t: &Option<Tokenized<f32>>) -> Option<f32> {
        match t {
            Some(Tokenized::Literal(v)) => Some(*v),
            _ => None,
        }
    }

    // Rule 27, pinned. A node is a flex CONTAINER only when its own
    // rules carry a flex-container property — the web backend dropped
    // its global `display: flex` baseline, and `flex_grow` /
    // `flex_basis` / `min_height` are flex ITEM properties that promote
    // nothing. `AppShell` sits between the viewport and every pane, and
    // NEITHER its container nor its content wrapper declares one, so
    // these two sheets are the only thing giving `MainPane`'s
    // `flex_grow: 1.0` root a context to grow in.
    //
    // Delete the `flex_direction` from either and every screen silently
    // stops scrolling: the pane sizes to its content, so nothing
    // overflows, so no `scroll_view` below it ever clamps. It still
    // builds, still lints, and still mounts — which is why this is a
    // test and not a comment.
    #[test]
    fn regression_the_scroll_chain_declares_its_flex_containers() {
        let harness = host_mock::Harness::with_registry(crate::register_scene_extensions);
        harness.world.enter(|| {
            idea_ui::install_idea_theme(idea_ui::light_theme());
            for (name, sheet) in [
                ("ShellSlot", super::shell_slot_style()),
                ("ContentFrame", super::content_frame_style()),
            ] {
                let rules = resolve_style(&StyleApplication::new(sheet));
                assert!(
                    rules.flex_direction.is_some(),
                    "{name} must declare itself a flex container, or the panes \
                     below it size to their content and nothing scrolls",
                );
                assert!(
                    rules.min_height.is_some(),
                    "{name} must let its scrolling descendants shrink below \
                     their own content (rule 23)",
                );
            }
        });
    }

    // The masthead is chrome: it sizes to its content and is never the
    // thing that gives. Without this a pane taller than the page column
    // is paid for by squeezing the header rather than by scrolling.
    #[test]
    fn regression_the_masthead_does_not_pay_for_a_taller_body() {
        let harness = host_mock::Harness::with_registry(crate::register_scene_extensions);
        harness.world.enter(|| {
            idea_ui::install_idea_theme(idea_ui::light_theme());
            let sheet = crate::components::header::header_bar_style();
            let rules = resolve_style(&StyleApplication::new(sheet));
            assert_eq!(shrink(&rules.flex_shrink), Some(0.0), "the masthead must not shrink");
        });
    }
}
