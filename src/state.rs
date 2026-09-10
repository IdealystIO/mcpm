//! Console-wide UI state: which pane is showing, which feature is
//! selected, which view tab is active, which drawer is open onto what.
//!
//! All signals — `Console` is a bag of `Copy` handles the components
//! pass through props by value, mirroring the framework's refs idiom.
//! The domain data itself lives in [`crate::model`] and is read-only.

use runtime_core::{signal, Signal};

/// Copy-able handle set for the console's interactive state.
#[derive(Clone, Copy)]
pub struct Console {
    /// Which top-level pane is showing: "overview" (the project home),
    /// "feature" (the selected feature's board), "features", "wants",
    /// "knowledge", or "key".
    pub pane: Signal<String>,
    /// Whether the left nav is showing as a drawer.
    ///
    /// Only meaningful BELOW `AppShell`'s pin breakpoint: at desktop
    /// widths the nav is pinned in flow and this is ignored, which is
    /// why it defaults to closed — the first thing a phone should show
    /// is the work, not the menu.
    pub nav_open: Signal<bool>,
    /// Index into [`crate::model::features`] of the selected feature.
    pub feature: Signal<usize>,
    /// Active main-pane view tab id: "graph" | "whitepaper" | "feed" |
    /// "origin".
    pub view: Signal<String>,
    /// Module drawer target: the module's index in
    /// [`crate::model::Feature::modules`] of the selected feature, or
    /// `None` when the drawer is closed.
    pub selected: Signal<Option<usize>>,
    /// Want drawer target: the id of the want whose detail panel is
    /// open, or `None`. An **id**, not an index — the pool re-sorts
    /// under a poll, so an index would drift onto another idea.
    pub want: Signal<Option<String>>,
    // --- Sticky drawer targets --------------------------------------
    // The two above answer "is a drawer open"; these answer "open onto
    // what", and are NOT cleared on close. A closing drawer plays its
    // exit animation while `selected`/`want` are already `None`, and
    // has to keep rendering its contents for those frames — clearing
    // both together would blank the panel and then slide the blank out.
    /// Last module the drawer was opened onto.
    pub last_module: Signal<Option<usize>>,
    /// Last want the drawer was opened onto.
    pub last_want: Signal<Option<String>>,
    /// Dark-mode flag; drives `install_idea_theme_reactive` in `app()`.
    pub dark: Signal<bool>,
    /// Data revision: bumped whenever a changed snapshot lands, so every
    /// view keyed on it re-reads [`crate::model`].
    pub rev: Signal<u64>,
    /// Whether the event socket is open. A real connection state, not
    /// "we managed a fetch once" — the header reports it.
    pub connected: Signal<bool>,

    // --- The API key ------------------------------------------------
    /// The console's API key, attached to every RPC as a bearer token.
    /// Empty on a host that does not ask for one.
    ///
    /// Persisted (see [`use_console_live`]), because a key you have to
    /// re-paste on every reload is a key people will keep in a text
    /// file instead.
    pub api_key: Signal<String>,
    /// The host answered 401. Set by the snapshot poll, cleared by the
    /// first fetch that succeeds — a live condition, not the memory of
    /// one failure, so the gate screen disappears on its own the moment
    /// a good key lands.
    pub denied: Signal<bool>,
    /// Whether the masthead's key popover is open. On `Console` and
    /// not inside the component so a snapshot landing mid-paste cannot
    /// close it — the same reason its draft lives here.
    pub key_open: Signal<bool>,
    /// Whether the feature switcher's popover is open. Also here, for
    /// the same reason: the feature header rebuilds on every poll.
    pub switcher_open: Signal<bool>,
    /// The feature switcher's search buffer.
    pub switcher_query: Signal<String>,
    /// The key-entry field's buffer. Lives here rather than in the gate
    /// component so a poll landing mid-paste cannot rebuild the input
    /// under the cursor — the same reason the capture composer's
    /// buffers do.
    pub key_draft: Signal<String>,

    // --- The want pool's toolbar ------------------------------------
    /// Free-text filter over want bodies.
    pub pool_query: Signal<String>,
    /// Status filter: "all" | "open" | "promoted" | "declined".
    pub pool_status: Signal<String>,
    /// Tags a want must carry to show. Empty means no tag filter.
    pub pool_tags: Signal<Vec<String>>,
    /// Zero-based page of the filtered pool.
    pub pool_page: Signal<usize>,

    // --- The all-features screen's toolbar --------------------------
    /// Free-text filter over feature names.
    pub feature_query: Signal<String>,
    /// Status filter: "all" | "open" | "done".
    pub feature_status: Signal<String>,
    /// Zero-based page of the filtered list.
    pub feature_page: Signal<usize>,

    // --- The knowledge screen's toolbar -----------------------------
    /// Free text over the knowledge base. Ranked server-side, so this
    /// is a query rather than a client-side filter.
    pub know_query: Signal<String>,
    /// Kinds to include. Empty means every kind.
    pub know_kinds: Signal<Vec<String>>,
    /// Tags an entry must carry, all of them.
    pub know_tags: Signal<Vec<String>>,
    /// Scope filter: "" (all) | "project" | "feature" | "module" | …
    pub know_level: Signal<String>,
    /// Zero-based page.
    pub know_page: Signal<usize>,
    /// The entry whose drawer is open, by id. An id and not an index:
    /// a re-ranked result set slides a held index onto a different
    /// entry, the same reason the want drawer addresses by id.
    pub know_open: Signal<Option<String>>,
    /// Show entries that have been superseded or disputed. Off by
    /// default — the current answer is what a reader wants — but never
    /// unavailable, because nothing here is ever deleted.
    pub know_history: Signal<bool>,

    // --- The capture composer ---------------------------------------
    // These live here, not inside the composer component, so a data
    // poll that rebuilds the pool can never discard half-typed text.
    /// The capture buffer: one want per line.
    pub draft: Signal<String>,
    /// Result of the last capture, shown next to the button.
    pub status: Signal<String>,
    /// A capture is in flight.
    pub busy: Signal<bool>,
    /// Bumped to make the poller fetch NOW rather than waiting out its
    /// window — so your own write appears immediately.
    pub refresh: Signal<u64>,
}

/// The namespace the console's persisted state is filed under.
const STORE: &str = "control-center";

/// The console state `app()` actually runs on: [`use_console`] with the
/// API key restored from local storage and written back as it changes.
///
/// Kept separate from [`use_console`] because `Console::default` calls
/// that one to satisfy the props machinery, and a `Default` impl is not
/// the place to touch the browser's storage — it runs for prop structs
/// nobody reads.
pub fn use_console_live() -> Console {
    Console {
        api_key: storage::persisted_signal(STORE, "api_key", String::new()),
        ..use_console()
    }
}

/// Create the console state. Call once from `app()` inside the mounted
/// reactive scope.
pub fn use_console() -> Console {
    Console {
        pane: signal("overview".to_string()),
        nav_open: signal(false),
        feature: signal(0),
        view: signal("graph".to_string()),
        selected: signal(None),
        want: signal(None),
        last_module: signal(None),
        last_want: signal(None),
        dark: signal(false),
        rev: signal(0),
        connected: signal(false),
        pool_query: signal(String::new()),
        pool_status: signal("all".to_string()),
        pool_tags: signal(Vec::new()),
        pool_page: signal(0),
        api_key: signal(String::new()),
        denied: signal(false),
        key_open: signal(false),
        switcher_open: signal(false),
        switcher_query: signal(String::new()),
        key_draft: signal(String::new()),
        feature_query: signal(String::new()),
        feature_status: signal("all".to_string()),
        feature_page: signal(0),
        know_query: signal(String::new()),
        know_kinds: signal(Vec::new()),
        know_tags: signal(Vec::new()),
        know_level: signal(String::new()),
        know_page: signal(0),
        know_history: signal(false),
        know_open: signal(None),
        draft: signal(String::new()),
        status: signal(String::new()),
        busy: signal(false),
        refresh: signal(0),
    }
}

impl Default for Console {
    /// Struct-literal component dispatch requires a `Default` for props
    /// structs carrying a `Console`; call sites always pass the real
    /// one, so these fresh (scope-owned) signals are never observed.
    fn default() -> Self {
        use_console()
    }
}

impl Console {
    /// Show the project home.
    pub fn show_overview(&self) {
        self.pane.set("overview".to_string());
        self.close_drawer();
    }

    /// Open or close the nav drawer.
    pub fn toggle_nav(&self) {
        self.nav_open.update(|open| !open);
    }

    /// Open one module's drawer from anywhere in the console, including
    /// a feature other than the selected one.
    ///
    /// The attention list on the home screen points across the whole
    /// project, so following a row has to move the selection as well as
    /// the drawer — `open_module` alone would open the drawer onto a
    /// coordinate in a feature the reader is not looking at.
    pub fn open_module_in(&self, feature: usize, module: usize) {
        self.pane.set("feature".to_string());
        self.feature.set(feature);
        self.view.set("graph".to_string());
        self.open_module(module);
    }

    /// Select a feature in the sidebar (closes any open drawer).
    pub fn select_feature(&self, index: usize) {
        self.pane.set("feature".to_string());
        self.feature.set(index);
        self.switcher_open.set(false);
        self.close_drawer();
    }

    /// Open or close the feature switcher, clearing whatever was typed
    /// into it last time — a stale query would hide the list the reader
    /// just asked to see.
    pub fn toggle_switcher(&self) {
        self.switcher_query.set(String::new());
        self.switcher_open.update(|open| !open);
    }

    /// Close the key popover without changing anything.
    pub fn dismiss_key(&self) {
        self.key_open.set(false);
    }

    /// Open the key popover, seeding the field with the key in use so
    /// rotating one is an edit rather than a retype.
    ///
    /// A popover and not a screen: changing a key is a one-field edit,
    /// and replacing the whole body for it threw away whatever the
    /// reader was looking at. The full-screen [`crate::components::gate::KeyGate`]
    /// is still what a REFUSED console shows — there the body behind it
    /// genuinely has nothing in it.
    pub fn show_key(&self) {
        self.key_draft.set(self.api_key.get());
        self.key_open.update(|open| !open);
    }

    /// Adopt the typed key and leave the gate. The poll picks it up on
    /// its next tick, and `denied` clears when a fetch succeeds — this
    /// does not clear it optimistically, because a second wrong key
    /// would then look accepted until the next failure.
    pub fn save_key(&self, key: String) {
        self.api_key.set(key.trim().to_string());
        self.key_open.set(false);
    }

    /// Show the want pool.
    pub fn show_wants(&self) {
        self.pane.set("wants".to_string());
        self.close_drawer();
    }

    /// Show the knowledge base.
    pub fn show_knowledge(&self) {
        self.pane.set("knowledge".to_string());
        self.close_drawer();
    }

    /// Narrow the knowledge base by free text.
    pub fn set_know_query(&self, text: String) {
        self.know_query.set(text);
        self.know_page.set(0);
    }

    /// Add or remove one kind from the filter.
    pub fn toggle_know_kind(&self, kind: &str) {
        self.know_kinds.update(|kinds| {
            let mut next = kinds.clone();
            match next.iter().position(|k| k == kind) {
                Some(at) => {
                    next.remove(at);
                }
                None => next.push(kind.to_string()),
            }
            next
        });
        self.know_page.set(0);
    }

    /// Add or remove one tag from the filter.
    pub fn toggle_know_tag(&self, tag: &str) {
        self.know_tags.update(|tags| {
            let mut next = tags.clone();
            match next.iter().position(|t| t == tag) {
                Some(at) => {
                    next.remove(at);
                }
                None => next.push(tag.to_string()),
            }
            next
        });
        self.know_page.set(0);
    }

    /// Narrow to one scope level (or "" for all).
    pub fn set_know_level(&self, level: String) {
        self.know_level.set(level);
        self.know_page.set(0);
    }

    /// Open one entry's detail drawer, or close it with `None`.
    pub fn open_knowledge(&self, id: Option<String>) {
        self.know_open.set(id);
    }

    /// Show or hide superseded and disputed entries.
    pub fn set_know_history(&self, on: bool) {
        self.know_history.set(on);
        self.know_page.set(0);
    }

    /// Drop every knowledge filter at once.
    pub fn clear_know_filters(&self) {
        self.know_query.set(String::new());
        self.know_kinds.set(Vec::new());
        self.know_tags.set(Vec::new());
        self.know_level.set(String::new());
        self.know_history.set(false);
        self.know_page.set(0);
    }

    /// Step the knowledge base's page.
    pub fn set_know_page(&self, page: usize) {
        self.know_page.set(page);
    }

    /// Show every feature, including the completed ones the rail hides.
    pub fn show_features(&self) {
        self.pane.set("features".to_string());
        self.close_drawer();
    }

    /// Narrow the feature list by name. Any filter change returns to the
    /// first page — page 4 of the old result set means nothing in the
    /// new one.
    pub fn set_feature_query(&self, text: String) {
        self.feature_query.set(text);
        self.feature_page.set(0);
    }

    /// Narrow the feature list to one status (or "all").
    pub fn set_feature_status(&self, status: String) {
        self.feature_status.set(status);
        self.feature_page.set(0);
    }

    /// Drop every feature filter at once.
    pub fn clear_feature_filters(&self) {
        self.feature_query.set(String::new());
        self.feature_status.set("all".to_string());
        self.feature_page.set(0);
    }

    /// Step the feature list's page. Callers clamp to the page count.
    pub fn set_feature_page(&self, page: usize) {
        self.feature_page.set(page);
    }

    /// Open the module drawer on one module of the selected feature.
    /// Also how the drawer moves to a prerequisite: with `selected`
    /// already `Some`, the panel stays present and re-targets.
    pub fn open_module(&self, module: usize) {
        self.want.set(None);
        self.selected.set(Some(module));
        self.last_module.set(Some(module));
    }

    /// Open the want drawer on one idea. Both drawers occupy the same
    /// right-hand slot, so opening either closes the other.
    pub fn open_want(&self, id: &str) {
        self.selected.set(None);
        self.want.set(Some(id.to_string()));
        self.last_want.set(Some(id.to_string()));
    }

    /// Close whichever drawer is open. The sticky targets stay put so
    /// the exit animation has something to draw on its way out.
    pub fn close_drawer(&self) {
        self.selected.set(None);
        self.want.set(None);
    }

    /// Narrow the pool by free text. Any filter change returns to the
    /// first page — page 4 of the old result set means nothing in the
    /// new one.
    pub fn set_pool_query(&self, text: String) {
        self.pool_query.set(text);
        self.pool_page.set(0);
    }

    /// Narrow the pool to one status (or "all").
    pub fn set_pool_status(&self, status: String) {
        self.pool_status.set(status);
        self.pool_page.set(0);
    }

    /// Add or remove one tag from the pool filter.
    pub fn toggle_pool_tag(&self, tag: &str) {
        self.pool_tags.update(|tags| {
            let mut next = tags.clone();
            match next.iter().position(|t| t == tag) {
                Some(at) => {
                    next.remove(at);
                }
                None => next.push(tag.to_string()),
            }
            next
        });
        self.pool_page.set(0);
    }

    /// Drop every pool filter at once.
    pub fn clear_pool_filters(&self) {
        self.pool_query.set(String::new());
        self.pool_status.set("all".to_string());
        self.pool_tags.set(Vec::new());
        self.pool_page.set(0);
    }

    /// Step the pool's page. Callers clamp to the page count.
    pub fn set_pool_page(&self, page: usize) {
        self.pool_page.set(page);
    }
}
