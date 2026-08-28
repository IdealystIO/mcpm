//! The app's entry point — every platform, one line.
//!
//! `entry!` reads `[package.metadata.idealyst.app]` from this crate's
//! Cargo.toml, lifts `control_center::register_scene_extensions` into the
//! `SceneExtensions` impl the boot seam needs, and emits a `main` that
//! hands both to `idealyst::boot::run`.
//!
//! Which shell that resolves to is CONFIG, not code: the target triple
//! settles web/iOS/Android, and a feature on the `idealyst` dep picks
//! between the native shells that share a triple. Nothing here names a
//! platform, so this one file serves every target.
idealyst::entry!(control_center);
