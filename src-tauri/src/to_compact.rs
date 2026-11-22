//! How many tokens a session can still take in before its CLI compacts its
//! context on its own (`Context::to_compact`): the threshold its CLI's
//! settings make of its window (claude/compact.rs, codex/compact.rs), less
//! what its last answer used. The settings are read with Luna's own readers,
//! never by asking the CLI, once per start of a session and model: what the
//! CLI read as it started is what it compacts by, and a model switched to
//! has settings and a window of its own. Each usage sample is then a
//! subtraction.
//!
//! Always an estimate: Luna counts a context by its last answer's usage,
//! and each CLI by its own count, which takes in more than that.

use crate::background::{Context, ToCompact};
use crate::provider::Provider;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

/// The start of a session a figure is reckoned for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Launch {
    pub provider: Provider,
    pub account_path: String,
    /// Where it works, whose project settings it reads.
    pub folder: String,
    /// A helper's run, its folder trusted for it (codex/config.rs
    /// `loaded_layers`).
    pub helper: bool,
    /// Which start of it: a chat's spawn, a helper's turn.
    pub start: u64,
}

impl Launch {
    /// The launch of session `id` while it is one: a helper's latest turn, a
    /// chat's running CLI. None for a chat whose CLI is not running: what a
    /// later start reads may differ.
    pub fn of(pty: &crate::pty::PtyManager, id: &str) -> Option<Launch> {
        if crate::background::has(id) {
            return crate::background::launch(id);
        }
        let p = pty.probe(id)?;
        let folder = p.working_dir().unwrap_or(p.cwd);
        Some(Launch { provider: p.provider, account_path: p.account_path, folder, helper: false, start: p.spawned_at_ms as u64 })
    }
}

#[derive(Clone)]
enum Inputs {
    Claude(crate::claude::compact::Inputs),
    Codex(crate::codex::compact::Inputs),
}

/// Sessions' inputs, by session, the start they were read for and model.
type Seen = HashMap<(String, Launch, String), Option<Inputs>>;

fn seen() -> &'static Mutex<Seen> {
    static S: OnceLock<Mutex<Seen>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// Past this many starts and models the cache starts over: it is only ever a
/// read saved.
const SEEN_CAP: usize = 512;

/// `Context::of` with its tokens to auto-compaction, where `launch` says
/// whose they are.
pub fn context_of(id: &str, launch: Option<&Launch>, path: &Path, provider: Provider) -> Option<Context> {
    let mut context = Context::of(path, provider)?;
    if let Some(launch) = launch.filter(|l| l.provider == provider) {
        // The model whose usage the context is: a switch mid-session reads
        // the new one's settings.
        let model = match provider {
            Provider::Claude => crate::claude::session::last_model(path),
            Provider::Codex => crate::codex::session::read_tail(path).model,
        };
        context.to_compact = model.and_then(|m| left(id, launch, &m, context.tokens, context.window));
    }
    Some(context)
}

/// What session `id` on `model` can take in past `tokens`, its rollout or
/// transcript stating `window`; None where auto-compaction is off or its
/// threshold cannot be told.
pub fn left(id: &str, launch: &Launch, model: &str, tokens: u64, window: u64) -> Option<ToCompact> {
    let key = (id.to_string(), launch.clone(), model.to_string());
    let known = seen().lock().unwrap_or_else(|e| e.into_inner()).get(&key).cloned();
    let inputs = match known {
        Some(inputs) => inputs,
        None => {
            // Read with the cache let go: every row's look waits on it.
            let inputs = inputs_of(launch, model);
            let mut s = seen().lock().unwrap_or_else(|e| e.into_inner());
            if s.len() >= SEEN_CAP {
                s.clear();
            }
            s.insert(key, inputs.clone());
            inputs
        }
    }?;
    let limit = match &inputs {
        Inputs::Claude(i) => crate::claude::compact::threshold(i, window),
        Inputs::Codex(i) => crate::codex::compact::limit(i, window),
    };
    Some(ToCompact { tokens: limit.saturating_sub(tokens), estimated: true })
}

fn inputs_of(launch: &Launch, model: &str) -> Option<Inputs> {
    let Launch { provider, account_path, folder, helper, .. } = launch;
    match provider {
        // Its CLI's environment is the core's, which starts it with none of
        // the variables read here.
        Provider::Claude => {
            crate::claude::compact::inputs(account_path, folder, model, &|name| std::env::var(name).ok()).map(Inputs::Claude)
        }
        Provider::Codex => crate::codex::compact::inputs(account_path, folder, *helper, model).map(Inputs::Codex),
    }
}

/// "~Nk": a figure to auto-compaction as a notice or a tooltip says it, in
/// thousands rounded down, so it never says more room than there is.
pub fn thousands(tokens: u64) -> String {
    format!("~{}k", tokens / 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch(account: &Path, start: u64) -> Launch {
        let path = account.to_string_lossy().into_owned();
        Launch { provider: Provider::Claude, account_path: path.clone(), folder: path, helper: false, start }
    }

    /// Inputs read once per start and model: a settings change shows at the
    /// next start, or on another model.
    #[test]
    fn the_settings_are_read_once_per_start_and_model() {
        let account = std::env::temp_dir().join(format!("luna-to-compact-{}", std::process::id()));
        std::fs::create_dir_all(&account).unwrap();
        let settings = account.join("settings.json");
        std::fs::write(&settings, r#"{ "autoCompactWindow": 400000 }"#).unwrap();
        let model = "claude-opus-5-5";
        let left_at = |start: u64, model: &str| left("tc-chat", &launch(&account, start), model, 100_000, 1_000_000);
        assert_eq!(left_at(1, model), Some(ToCompact { tokens: 400_000 - 33_000 - 100_000, estimated: true }));
        std::fs::write(&settings, r#"{ "autoCompactEnabled": false }"#).unwrap();
        assert_eq!(left_at(1, model).map(|t| t.tokens), Some(267_000), "the same start: what it read then");
        assert_eq!(left_at(1, "claude-sonnet-5"), None, "another model: read again");
        assert_eq!(left_at(2, model), None, "a new start: read again");
        std::fs::write(&settings, "{}").unwrap();
        assert_eq!(left("tc-chat", &launch(&account, 3), model, 990_000, 1_000_000).map(|t| t.tokens), Some(0), "past it: none left");
        let _ = std::fs::remove_dir_all(&account);
    }

    #[test]
    fn a_figure_is_said_in_thousands_rounded_down() {
        assert_eq!(thousands(267_999), "~267k");
        assert_eq!(thousands(999), "~0k");
    }
}
