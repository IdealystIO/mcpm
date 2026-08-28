//! `app()` — composes the MCP Project Console: masthead, feature rail,
//! main pane, and the right-hand detail drawers — and keeps it live: the
//! console polls the mcpm-web server (which reads the same Postgres
//! store the MCP tools write) and re-renders whenever the data changes.

use idea_ui::{dark_theme, install_idea_theme_reactive, light_theme, IdeaThemeRef};
use runtime_core::{presence, raf_loop_scoped, spawn_then, stylesheet, switch, ui, Easing,
    Element, FlexDirection, IntoElement, Position, PresenceAnim};

use crate::components::drawer::{Drawer, WantDrawer};
use crate::components::header::Header;
use crate::components::main_pane::MainPane;
use crate::components::sidebar::Sidebar;
use crate::model;
use crate::state::{use_console, Console};

/// Where the mcpm-web API listens
/// (`cargo run -p api --bin mcpm-web --features server`).
const API_ORIGIN: &str = "http://127.0.0.1:3210";
/// Snapshot poll cadence — the FALLBACK, not the primary path. Every
/// committed event arrives over `watch_events` within a frame or two;
/// this is what keeps the console correct if that socket is down (the
/// host restarted, a proxy dropped the upgrade, LISTEN failed).
const POLL_MICROS: u64 = 30_000_000;

// Drawer motion. The backdrop fades over the whole overlay; the panel
// itself slides (see `drawer::panel_motion`). Out is quicker than in —
// a dismissal should feel like it obeyed you, not like it is being
// reconsidered.
/// Backdrop fade-in.
pub const BACKDROP_IN_MS: u32 = 180;
/// Backdrop fade-out.
pub const BACKDROP_OUT_MS: u32 = 140;

pub fn app() -> Element {
    let console = use_console();
    install_idea_theme_reactive(move || {
        if console.dark.get() {
            dark_theme()
        } else {
            light_theme()
        }
    });

    start_sync(console);

    // The drawer overlays the whole page. `presence` owns the
    // mount/unmount timing so the close animation can finish before the
    // subtree drops — a plain `switch` on the open flag would tear it
    // down on the same frame, leaving no window for an exit. The inner
    // `switch` is keyed on the STICKY target so the panel keeps drawing
    // itself while it slides away.
    let drawer_host = presence(move || {
        switch(
            move || (console.feature.get(), console.last_module.get(), console.rev.get()),
            move |&(fi, sel, _rev): &(usize, Option<(usize, usize)>, u64)| match sel {
                Some((si, mi)) if drawer_target_exists(fi, si, mi) => ui! {
                    Drawer(console = console, feature = fi, stage = si, module = mi)
                },
                _ => ui! { view {} },
            },
        )
    })
    .present(move || console.selected.get().is_some())
    .enter(PresenceAnim::fade(BACKDROP_IN_MS, Easing::EaseOut))
    .exit(PresenceAnim::fade(BACKDROP_OUT_MS, Easing::EaseIn))
    .into_element();

    // The want drawer shares the module drawer's slot; `Console` keeps
    // at most one of the two targets set. Keyed on the want **id**,
    // resolved to an index here, because a poll can re-sort the pool
    // under an index.
    let want_host = presence(move || {
        switch(
            move || (console.last_want.get(), console.rev.get()),
            move |(id, _rev): &(Option<String>, u64)| {
                match id.as_deref().and_then(model::want_index) {
                    Some(wi) => ui! { WantDrawer(console = console, want = wi) },
                    None => ui! { view {} },
                }
            },
        )
    })
    .present(move || console.want.get().is_some())
    .enter(PresenceAnim::fade(BACKDROP_IN_MS, Easing::EaseOut))
    .exit(PresenceAnim::fade(BACKDROP_OUT_MS, Easing::EaseIn))
    .into_element();

    ui! {
        view(style = PageFrame()) {
            Header(console = console)
            view(style = BodyRow()) {
                Sidebar(console = console)
                MainPane(console = console)
            }
            drawer_host
            want_host
        }
    }
}

fn drawer_target_exists(fi: usize, si: usize, mi: usize) -> bool {
    model::features()
        .get(fi)
        .and_then(|f| f.stages.get(si))
        .map(|s| mi < s.modules.len())
        .unwrap_or(false)
}

/// Configure the RPC origin, open the event subscription, and drive the
/// snapshot fetch from a raf clock (scope-anchored, so both die with
/// the app).
///
/// The socket carries only "an event committed, at seq N" — the console
/// then refetches the snapshot it already knows how to apply, rather
/// than folding individual events into the tree. That keeps
/// `apply_snapshot` the single place the model is written, and costs
/// one ~16KB fetch per change instead of one every three seconds
/// forever.
fn start_sync(console: Console) {
    server::configure(server::ClientConfig::new(API_ORIGIN));
    // Scope-bound: the socket closes when the app unmounts.
    let events = api::watch_events();
    // `None` means "fetch on the next frame"; `Some(t)` is when the
    // last attempt started. Deliberately not `0`: on web `now_micros`
    // counts from page load, so for the first POLL_MICROS after load
    // `now - 0` is still inside the window — a zero sentinel would gate
    // off the very first snapshot, and any event tick or local capture
    // that lands while the page is young, until the window elapsed.
    let mut last_fetch: Option<u64> = None;
    let mut in_flight = false;
    let mut seen_nudge: u64 = 0;
    let mut seen_seq: i64 = 0;
    raf_loop_scoped(move || {
        let now = runtime_core::time::now_micros();
        // Report the real connection, not "a snapshot landed once".
        console.connected.set(matches!(events.status(), server::SocketStatus::Open));
        // An event committed anywhere — an agent's write as much as our
        // own. Seq-keyed, so a replayed notification is not a second
        // fetch.
        if let Some(tick) = events.latest() {
            if tick.seq > seen_seq {
                seen_seq = tick.seq;
                last_fetch = None;
                in_flight = false;
            }
        }
        // A local write (the capture composer) bumps `refresh` so its
        // result shows up without waiting for the round trip.
        let nudge = console.refresh.get();
        if nudge != seen_nudge {
            seen_nudge = nudge;
            last_fetch = None;
            in_flight = false;
        }
        let due = last_fetch.is_none_or(|t| now.saturating_sub(t) >= POLL_MICROS);
        if in_flight || !due {
            // A fetch answers (or fails) well within a poll window;
            // reset the in-flight latch once the window passes so a
            // dropped callback can't wedge the loop.
            if due {
                in_flight = false;
            }
            return;
        }
        last_fetch = Some(now);
        in_flight = true;
        spawn_then(api::load_snapshot(), move |result| {
            match result {
                Ok(snapshot) => {
                    if model::apply_snapshot(snapshot) {
                        console.rev.update(|r| r + 1);
                    }
                }
                // The screen already says "connecting" and the next poll
                // retries, so this stays out of the UI — but it goes to
                // the log, because a poll that fails forever with no
                // trace anywhere is undiagnosable.
                Err(err) => runtime_core::log_warn!("snapshot poll failed: {err:?}"),
            }
        });
        in_flight = false;
    });
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

stylesheet! {
    pub BodyRow<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            min_height: 0,
            flex_direction: FlexDirection::Row,
        }
    }
}
