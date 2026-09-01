//! Short, prefixed, stable ids: `feat_…`, `stg_…`, `mod_…`, `tsk_…`,
//! `mem_…`, `want_…`. Never positional, so reordering a stage or moving a module
//! breaks no reference; the prefix lets tools dispatch on id shape.

use serde::{Deserialize, Serialize};

/// Which level of the tree an id (or a memory scope) names.
///
/// `Project` is not part of the work tree — nothing is ever planned or
/// claimed at project level. It exists so knowledge can be filed ABOVE
/// the tree: a convention, a decision that binds every feature, a
/// pointer to something external. It is the top of the `direction =
/// up` chain, so a worker reading upward from its module reaches it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Project,
    Feature,
    Stage,
    Module,
    Task,
}

/// The single subject id every project-scoped memory pins to.
///
/// One deployment is one project (see the README), so this is a
/// singleton rather than a row id — but it is spelled like an id so
/// `id_level` recognizes it and the `subject_id` column stays uniform.
pub const PROJECT_SUBJECT: &str = "proj_main";

impl Level {
    pub fn prefix(self) -> &'static str {
        match self {
            Level::Project => "proj",
            Level::Feature => "feat",
            Level::Stage => "stg",
            Level::Module => "mod",
            Level::Task => "tsk",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Level::Project => "project",
            Level::Feature => "feature",
            Level::Stage => "stage",
            Level::Module => "module",
            Level::Task => "task",
        }
    }

    pub fn from_str(s: &str) -> Option<Level> {
        match s {
            "project" => Some(Level::Project),
            "feature" => Some(Level::Feature),
            "stage" => Some(Level::Stage),
            "module" => Some(Level::Module),
            "task" => Some(Level::Task),
            _ => None,
        }
    }
}

/// Mint a fresh id for a level: prefix + 8 hex chars of a UUIDv4.
pub fn new_id(level: Level) -> String {
    let uuid = uuid::Uuid::new_v4().simple().to_string();
    format!("{}_{}", level.prefix(), &uuid[..8])
}

/// Mint a memory id.
pub fn new_memory_id() -> String {
    let uuid = uuid::Uuid::new_v4().simple().to_string();
    format!("mem_{}", &uuid[..8])
}

/// Mint a want id. Wants live beside the tree, not in it — they are raw
/// ideas, not work — so they carry their own prefix and never appear as
/// a [`Level`].
pub fn new_want_id() -> String {
    let uuid = uuid::Uuid::new_v4().simple().to_string();
    format!("want_{}", &uuid[..8])
}

/// Recover the level from an id's prefix (for tools like `revise_plan`
/// whose ops take any tree id).
pub fn id_level(id: &str) -> Option<Level> {
    match id.split('_').next() {
        Some("proj") => Some(Level::Project),
        Some("feat") => Some(Level::Feature),
        Some("stg") => Some(Level::Stage),
        Some("mod") => Some(Level::Module),
        Some("tsk") => Some(Level::Task),
        _ => None,
    }
}

/// Normalize a tag to its registry slug: lowercase, `-`-joined, and
/// limited to `[a-z0-9_-]`. A leading `#` is accepted and dropped, so
/// the same function serves typed `#field-reports` and an agent's
/// `"Field Reports"`. Returns `None` for anything that normalizes away
/// to nothing.
pub fn normalize_tag(raw: &str) -> Option<String> {
    let mut out = String::new();
    let mut last_dash = true; // leading dashes are dropped
    for ch in raw.trim().trim_start_matches('#').chars() {
        let ch = ch.to_ascii_lowercase();
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
            last_dash = false;
        } else if !last_dash && out.len() < TAG_MAX {
            out.push('-');
            last_dash = true;
        }
        if out.len() >= TAG_MAX {
            break;
        }
    }
    let trimmed = out.trim_end_matches('-');
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Slug length cap. Long enough for a real phrase, short enough that a
/// tag stays a label rather than becoming the note itself.
const TAG_MAX: usize = 32;
