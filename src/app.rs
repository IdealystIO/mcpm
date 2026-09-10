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
    let drawer_host = presence(move || {
        switch(
            move || (console.feature.get(), console.last_module.get(), console.rev.get()),
            move |&(fi, sel, _rev): &(usize, Option<usize>, u64)| match sel {
                Some(mi) if drawer_target_exists(fi, mi) => ui! {
                    Drawer(console = console, feature = fi, module = mi)
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

fn drawer_target_exists(fi: usize, mi: usize) -> bool {
    model::features()
        .get(fi)
        .map(|f| mi < f.modules.len())
        .unwrap_or(false)
}

/// Configure the RPC origin, open the event subscription, and drive the
/// snapshot fetch from a raf clock (scope-anchored, so both die with
/// the scope that called this).
///
/// The socket carries only "an event committed, at seq N" — the console
/// then refetches the snapshot it already knows how to apply, rather
/// than folding individual events into the tree. That keeps
/// `apply_snapshot` the single place the model is written, and costs
/// one ~16KB fetch per change instead of one every three seconds
/// forever.
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
                    // A fetch that lands is proof the key (or the lack
                    // of one) is accepted, so the gate clears itself
                    // rather than waiting to be dismissed.
                    console.denied.set(false);
                    // Only the FIRST snapshot moves the selection. A
                    // later poll must not steal the feature the reader
                    // is on because a newer one arrived.
                    let first_load = !model::loaded();
                    if model::apply_snapshot(snapshot) {
                        if first_load {
                            if let Some(open) = model::first_open_feature() {
                                console.feature.set(open);
                            }
                        }
                        console.rev.update(|r| r + 1);
                    }
                }
                // A refusal is a condition the reader can act on — it
                // ends when they paste a working key — so it reaches the
                // screen. Every other failure is transient and the next
                // poll retries it, so it stays a log line: the screen
                // already says "connecting", and a poll that fails
                // forever with no trace anywhere is undiagnosable.
                Err(err) => {
                    if matches!(err, server::ServerError::Server { status: 401, .. }) {
                        console.denied.set(true);
                    }
                    runtime_core::log_warn!("snapshot poll failed: {err:?}");
                }
            }
        });
        in_flight = false;
    });
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
