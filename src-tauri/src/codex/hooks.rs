//! Whether Codex runs a folder's hooks without asking first.
//!
//! Codex runs the hooks a project brings (`.codex/hooks.json`) only once they
//! are trusted, and trust is kept per account, per path and per hook content:
//! a new worktree, another account or an edited hook asks again. The TUI asks
//! with its "Hooks need review" screen; `codex exec`, which background helpers
//! are, cannot ask and leaves untrusted hooks out without a word.
//! `--dangerously-bypass-hook-trust` runs them unasked. Whether a session gets
//! it is the new-chat dialog's "Hooks without asking" box, which starts the
//! way the default in Settings or the folder's own kept choice says; a helper
//! gets whatever its project folder would. Kept in Luna's settings file, not
//! the window's storage, because the core starts background helpers without
//! asking the window.

use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookStart {
    /// The default in Settings.
    pub default: bool,
    /// What was kept for this folder, if anything.
    pub kept: Option<bool>,
}

/// What a Codex session in `folder` (a project folder) starts with.
pub fn unasked_in(folder: &str) -> bool {
    let s = crate::settings::get();
    kept(&s, folder).unwrap_or(s.codex_hooks_unasked)
}

fn kept(s: &crate::settings::Settings, folder: &str) -> Option<bool> {
    s.codex_hooks_unasked_in.iter().find(|(path, _)| crate::pty::same_dir(path, folder)).map(|(_, on)| *on)
}

#[tauri::command]
pub fn codex_hooks_start(folder: String) -> HookStart {
    let s = crate::settings::get();
    HookStart { default: s.codex_hooks_unasked, kept: kept(&s, &folder) }
}

#[tauri::command]
pub fn set_codex_hooks_default(on: bool) -> Result<bool, String> {
    crate::settings::update(|s| s.codex_hooks_unasked = on)?;
    Ok(on)
}

/// Keeps a folder's own choice; `None` hands the folder back to the default.
#[tauri::command]
pub fn keep_codex_hooks(folder: String, on: Option<bool>) -> Result<(), String> {
    crate::settings::update(|s| {
        s.codex_hooks_unasked_in.retain(|path, _| !crate::pty::same_dir(path, &folder));
        if let Some(on) = on {
            s.codex_hooks_unasked_in.insert(folder, on);
        }
    })
}
