//! `control-center` — the MCP Project Console.
//!
//! A read-only window onto a single project's agent-driven work:
//! Features plan into a GRAPH of Modules (each names the modules it
//! depends on; the gate opens when every one is done), Modules carry
//! Task checklists the agents check off over MCP, and both levels
//! carry a document — the feature's whitepaper, the module's handoff.
//! The console shows the graph, the whitepaper, the event ledger, and
//! each module's drawer — including gate rejections when a subagent
//! starts before its prerequisites are done.
//!
//! Built on idea-ui components over an installed light/dark IdeaTheme.
//! The entry point is `src/main.rs`, one `idealyst::entry!` line whose
//! shell the target triple selects.

mod app;
mod components;
mod model;
mod state;
mod styles;

pub use app::app;

// Recorder-side registration seam for the runtime-server sidecar
// (`dev_server::sidecar::run_newcore`) — the recorder's scene-registry
// twin of `register_scene_extensions`. Gated by `sidecar` (set only by
// the generated sidecar wrapper) so device/web builds never pull
// `dev-server`.
#[cfg(feature = "sidecar")]
pub fn register_scene_extensions_recorder(registry: &mut dev_server::newcore::SceneRegistry) {
    register_scene_extensions(registry);
}

// SDK-handler registration seam, invoked by the CLI-generated wrappers
// (web `start_in`/`hydrate_in`, macOS `run_with`, iOS `run_in_view`,
// terminal `run`) after `runtime_vocabulary::register_builtins`.
// Registry-generic over the scene `Host` so ONE seam serves every
// backend; each wrapper's call site pins `H` to its concrete backend.
//
// Registration is MANDATORY — the scene registry has no fallback
// handler, so a `code_editor` with no handler panics at realize rather
// than rendering a placeholder. The bounds are `codeblock::register`'s
// own; they are caps, not a backend dependency.
pub fn register_scene_extensions<H>(registry: &mut runtime_scene::Registry<H>)
where
    H: runtime_vocabulary::style_attach::StyleServices
        + runtime_vocabulary::caps::TextOps
        + runtime_vocabulary::caps::InputOps
        + 'static,
{
    // The want composer's `code_editor`.
    codeblock::register(registry);
    // `idea_ui::Table` — the want pool's rows. It is not a plain
    // component over builtins: it renders the `table` SDK's payloads, so
    // it needs a handler like any other extension. The bounds above are
    // the UNION of what these two ask for (`TextOps` for codeblock,
    // `InputOps` for table); every backend satisfies all of them, so a
    // new SDK here widens the bound rather than forcing a second seam.
    table::register(registry);
    // The whitepaper and handoff documents. Its bounds (`StyleServices +
    // TextOps`) are already inside the union above.
    markdown::register(registry);
}

// Android entry: the generated Android wrapper's `attach` mounts
// `scene_app()` through `backend_android::newcore::start`. `app()`
// already returns the scene `Element`, so this is a plain re-export
// shim with the conventional name.
pub fn scene_app() -> runtime_core::Element {
    app()
}
