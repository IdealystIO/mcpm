//! `control-center` — the MCP Project Console.
//!
//! A read-only window onto a single project's agent-driven work:
//! Features plan into Stages (sequential, gate-enforced), Stages hold
//! Modules (one worker subagent each, concurrent within a stage), and
//! Modules carry Task checklists the agents check off over MCP. The
//! console shows the board, the hierarchy, the event ledger, the
//! dependency graph, and each module's drawer — including gate
//! rejections when a subagent starts too early.
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
        + 'static,
{
    codeblock::register(registry);
}

// Android entry: the generated Android wrapper's `attach` mounts
// `scene_app()` through `backend_android::newcore::start`. `app()`
// already returns the scene `Element`, so this is a plain re-export
// shim with the conventional name.
pub fn scene_app() -> runtime_core::Element {
    app()
}
