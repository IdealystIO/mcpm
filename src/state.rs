//! Console-wide UI state: which pane is showing, which feature is
//! selected, which view tab is active, which drawer is open onto what,
//! which tree rows are toggled.
//!
//! All signals — `Console` is a bag of `Copy` handles the components
//! pass through props by value, mirroring the framework's refs idiom.
//! The domain data itself lives in [`crate::model`] and is read-only.

use runtime_core::{signal, Signal};

/// Copy-able handle set for the console's interactive state.
#[derive(Clone, Copy)]
pub struct Console {
    /// Which top-level pane is showing: "feature" (the selected
    /// feature's board) or "wants" (the project-wide idea pool).
    pub pane: Signal<String>,
    /// Index into [`crate::model::features`] of the selected feature.
    pub feature: Signal<usize>,
    /// Active main-pane view tab id: "board" | "tree" | "feed" | "graph".
    pub view: Signal<String>,
    /// Module drawer target: `(stage index, module index)` in the
    /// selected feature, or `None` when the drawer is closed.
    pub selected: Signal<Option<(usize, usize)>>,
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
    pub last_module: Signal<Option<(usize, usize)>>,
    /// Last want the drawer was opened onto.
    pub last_want: Signal<Option<String>>,
    /// Tree rows whose disclosure state has been flipped from its
    /// default (stages default open, modules default closed). Keys are
    /// `"s{stage}"` / `"m{stage}.{module}"`.
    pub toggled: Signal<Vec<String>>,
    /// Dark-mode flag; drives `install_idea_theme_reactive` in `app()`.
    pub dark: Signal<bool>,
    /// Data revision: bumped whenever a changed snapshot lands, so every
    /// view keyed on it re-reads [`crate::model`].
    pub rev: Signal<u64>,
    /// Whether the event socket is open. A real connection state, not
    /// "we managed a fetch once" — the header reports it.
    pub connected: Signal<bool>,

    // --- The want pool's toolbar ------------------------------------
    /// Free-text filter over want bodies.
    pub pool_query: Signal<String>,
    /// Status filter: "all" | "open" | "promoted" | "declined".
    pub pool_status: Signal<String>,
    /// Tags a want must carry to show. Empty means no tag filter.
    pub pool_tags: Signal<Vec<String>>,
    /// Zero-based page of the filtered pool.
    pub pool_page: Signal<usize>,

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

/// Create the console state. Call once from `app()` inside the mounted
/// reactive scope.
pub fn use_console() -> Console {
    Console {
        pane: signal("feature".to_string()),
        feature: signal(0),
        view: signal("board".to_string()),
        selected: signal(None),
        want: signal(None),
        last_module: signal(None),
        last_want: signal(None),
        toggled: signal(Vec::new()),
        dark: signal(false),
        rev: signal(0),
        connected: signal(false),
        pool_query: signal(String::new()),
        pool_status: signal("all".to_string()),
        pool_tags: signal(Vec::new()),
        pool_page: signal(0),
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
    /// Select a feature in the sidebar (closes any open drawer).
    pub fn select_feature(&self, index: usize) {
        self.pane.set("feature".to_string());
        self.feature.set(index);
        self.close_drawer();
    }

    /// Show the want pool.
    pub fn show_wants(&self) {
        self.pane.set("wants".to_string());
        self.close_drawer();
    }

    /// Open the module drawer.
    pub fn open_module(&self, stage: usize, module: usize) {
        self.want.set(None);
        self.selected.set(Some((stage, module)));
        self.last_module.set(Some((stage, module)));
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

    /// Flip a tree row's disclosure state.
    pub fn toggle(&self, key: &str) {
        self.toggled.update(|keys| {
            let mut next = keys.clone();
            if let Some(pos) = next.iter().position(|k| k == key) {
                next.remove(pos);
            } else {
                next.push(key.to_string());
            }
            next
        });
    }

    /// Whether a tree row key has been flipped from its default.
    pub fn is_toggled(keys: &[String], key: &str) -> bool {
        keys.iter().any(|k| k == key)
    }
}
