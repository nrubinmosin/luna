//! What Luna lets an agent put into a session, as Luna's own admission and
//! not a judgment of the CLI's modes: no helper sits in a folder that holds
//! Luna (`reaches_luna`), text is plain text (`plain_text`), and text a CLI
//! would read as a command goes only into the caller's own helpers
//! (`command_reach`). Whose a session is, and so what reaches it at all, is
//! the registry's (agents/registry.rs `is_descendant`, `authorize_send`).

use crate::agents::registry::{is_descendant, is_dev, reg};

/// A folder that is Luna's data folder, inside it or above it: a helper there
/// could edit Luna's settings, its library and what it keeps of its sessions,
/// whatever mode it runs in. Compared resolved (links followed) as far as the
/// path exists, the rest put back on, as `library::resolved` has it.
pub(super) fn reaches_luna(folder: &str) -> bool {
    let resolve = |p: &std::path::Path| crate::paths::path_key(&crate::library::resolved(p).to_string_lossy());
    within_or_above(&resolve(std::path::Path::new(folder)), &resolve(&crate::paths::data_dir()))
}

/// Two `path_key`s: one is the other or inside it.
fn within_or_above(a: &str, b: &str) -> bool {
    let sep = std::path::MAIN_SEPARATOR;
    // A Unix root's key is empty, and holds everything.
    let under =
        |inner: &str, outer: &str| outer.is_empty() || inner == outer || inner.strip_prefix(outer).is_some_and(|r| r.starts_with(sep));
    under(a, b) || under(b, a)
}

/// Whether `id` is one of the caller's helpers, directly or through one.
pub(super) fn owns(caller: &str, id: &str) -> bool {
    is_descendant(&reg().lock().unwrap_or_else(|e| e.into_inner()), caller, id)
}

/// The first non-blank character a CLI's composer reads as a command rather
/// than a prompt: `/` (slash commands; Codex's `/permissions` switches its
/// sandbox with no dialog) and `!` (shell mode: the command runs as the user,
/// outside the model and its sandbox), in both CLIs. Claude Code 2.1.318 has
/// no `#` shortcut (its hints: `!`, `/`, `@` for file paths, `?` for help).
pub(crate) fn command_prefix(text: &str) -> Option<char> {
    text.trim_start().chars().next().filter(|c| matches!(c, '/' | '!'))
}

/// Text sent to a session is a prompt for its model. One the CLI would take as
/// a command goes only into the caller's own helpers, or from the dev caller;
/// it is refused, not escaped, as no escape is known to hold in both CLIs.
/// Into a chat's terminal, or another's helper, it never goes: the chats there
/// are the person's, and what runs in another's helper is its owner's.
pub(super) fn command_reach(text: &str, dev: bool, own: bool) -> Result<(), String> {
    let Some(c) = command_prefix(text) else { return Ok(()) };
    if dev || own {
        return Ok(());
    }
    let what = if c == '/' { "a slash command" } else { "shell mode, a command run as the user" };
    Err(format!(
        "text starting with \"{c}\" is {what} to the CLI, not a prompt: it goes only into your own helpers. Start the \
         text with something else"
    ))
}

/// `command_reach` for a caller and a target as they stand now: checked when
/// text is taken and again when it is typed, held text and resumes included.
pub(crate) fn command_reach_now(caller: &str, id: &str, text: &str) -> Result<(), String> {
    if command_prefix(text).is_none() {
        return Ok(());
    }
    command_reach(text, is_dev(caller), owns(caller, id))
}

/// Text an agent sends, refused if it holds what a terminal takes as keys:
/// Esc (and so any escape sequence, the end of a bracketed paste among them),
/// other C0 and C1 controls, DEL. Line breaks and tabs are text.
pub(super) fn plain_text(text: &str) -> Result<(), String> {
    match text.chars().find(|&c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')) {
        Some(c) => Err(format!("the text holds control character U+{:04X}, which a terminal takes as a key; send plain text", c as u32)),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_helper_works_in_lunas_own_folder_or_above_it() {
        let sep = std::path::MAIN_SEPARATOR;
        let k = |p: &str| crate::paths::path_key(&p.replace('/', &sep.to_string()));
        let data = k("/apps/luna");
        assert!(within_or_above(&data, &data));
        assert!(within_or_above(&k("/apps/luna/accounts/x"), &data), "inside");
        assert!(within_or_above(&k("/apps"), &data), "above: it holds Luna's folder");
        assert!(within_or_above(&k("/"), &data), "the root holds everything");
        assert!(!within_or_above(&k("/apps/luna-test"), &data), "a sibling whose name starts the same");
        assert!(!within_or_above(&k("/projects/x"), &data));
        assert!(reaches_luna(&crate::paths::data_dir().join("sub").to_string_lossy()));
        assert!(!reaches_luna(&std::env::temp_dir().join("luna-some-project").to_string_lossy()));
    }

    #[test]
    fn sent_text_holds_no_keys() {
        assert_eq!(plain_text("two lines\r\nand\ta tab, \u{fc}nicode, \u{65e5}\u{672c}"), Ok(()));
        for bad in ["\x1b[201~y\r", "\x1b", "a\x03", "\x7f", "\u{9b}2~", "\x00"] {
            assert!(plain_text(bad).unwrap_err().contains("control character"), "{bad:?}");
        }
    }

    /// Whatever the modes either side runs in: into one's own helpers, yes;
    /// into anything else, a person's chats included, never.
    #[test]
    fn text_a_cli_reads_as_a_command_goes_only_into_ones_own_helpers() {
        for text in ["/permissions :workspace", "! rm -rf build", "!cmd", "  /approve-for-me", "\n\t! whoami", "\u{3000}/x"] {
            assert!(command_prefix(text).is_some(), "{text:?}");
        }
        for text in ["fix /src/main.rs", "why ! there", "# heading", "@file.rs look", "?", "", "   "] {
            assert!(command_prefix(text).is_none(), "{text:?}");
        }
        for text in ["/permissions :workspace", "  ! cmd"] {
            assert_eq!(command_reach(text, false, true), Ok(()), "{text:?}: its own helper");
            assert!(command_reach(text, false, false).unwrap_err().contains("only into your own helpers"), "{text:?}");
            assert_eq!(command_reach(text, true, false), Ok(()), "the dev caller");
        }
        assert_eq!(command_reach("plain words", false, false), Ok(()));
    }
}
