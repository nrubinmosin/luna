//! `config.toml` under a Codex account (`CODEX_HOME`), read and written with
//! the user's own formatting and comments intact.

use std::path::{Path, PathBuf};
use toml_edit::DocumentMut;

pub fn account_file(account_path: &str) -> Result<PathBuf, String> {
    if account_path.is_empty() {
        return Err("No account selected".into());
    }
    Ok(Path::new(account_path).join("config.toml"))
}

/// The document to change and write back, or an empty one when the file is
/// missing. A file that will not read or parse is an error rather than an
/// empty document: writing back over it would throw the user's config away.
/// Codex or an editor rewriting the file in place can be caught midway —
/// blank, cut short, or on Windows held open — so such a read looks again a
/// moment later. Blank on every look, the file is blank, and an empty
/// document loses nothing of it.
pub fn read(path: &Path) -> Result<DocumentMut, String> {
    let mut looks = 0;
    loop {
        let got = match std::fs::read_to_string(path) {
            Ok(text) => match text.parse::<DocumentMut>() {
                Ok(doc) if !text.trim().is_empty() => return Ok(doc),
                Ok(doc) => Ok(doc),
                Err(e) => Err(format!("{}: {e}", path.display())),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(DocumentMut::new()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        };
        looks += 1;
        if looks == 3 {
            return got;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// The MCP servers an account's `config.toml` defines, each by its name as
/// written with the keys its table sets; none without the file. One that will not read or parse, after
/// `read`'s looks again, is an error and not an empty list: a helper's start
/// turns those servers off by name, and a list read short would leave them on.
pub fn mcp_servers(account_path: &str) -> Result<Vec<(String, Vec<String>)>, String> {
    let doc = read(&account_file(account_path)?)?;
    let servers = doc.get("mcp_servers").and_then(|t| t.as_table_like());
    let keys = |v: &toml_edit::Item| v.as_table_like().map(|t| t.iter().map(|(k, _)| k.to_string()).collect()).unwrap_or_default();
    Ok(servers.map(|t| t.iter().map(|(name, v)| (name.to_string(), keys(v))).collect()).unwrap_or_default())
}

/// `mcp_servers` for a helper's start, which goes no further without it.
pub fn helper_mcp_servers(account_path: &str) -> Result<Vec<(String, Vec<String>)>, String> {
    mcp_servers(account_path).map_err(|e| {
        format!(
            "the helper is not started: Luna could not read which MCP servers its account's config.toml defines, to turn them off ({e})"
        )
    })
}

/// What a helper started without connectors still has (codex/launch.rs
/// `connectors_off` turns off only the account's own servers), one line each,
/// for the spawn's result and "What Luna applied": the servers of each
/// `.codex/config.toml` Codex reads as a project layer — the folder's and its
/// parents' up to the project root, the one holding `.git`, or the folder's
/// alone outside a repository; a helper's folder is trusted for its run — and
/// the plugins on in the config Codex merges for the run, which may bring
/// servers of their own (`plugins_on`). Not the plugins of Luna's library:
/// they hold skills only (library/folders.rs).
pub fn servers_left_on(account_path: &str, folder: &str) -> Vec<String> {
    let mut left = Vec::new();
    let account = Path::new(account_path);
    // Lowest first, as Codex merges them: the account, then the project's
    // layers from its root down to the folder.
    let mut docs = vec![read_or_empty(&account.join("config.toml"))];
    for file in project_layers(account_path, folder) {
        let doc = read_or_empty(&file);
        if let Some(servers) = doc.get("mcp_servers").and_then(|t| t.as_table_like()) {
            left.extend(servers.iter().map(|(name, _)| format!("server {name:?} of {}", file.display())));
        }
        docs.push(doc);
    }
    let luna = format!("@{}", crate::library::CODEX_MARKET);
    left.extend(
        plugins_on(&docs)
            .into_iter()
            .filter(|name| !name.ends_with(&luna))
            .map(|name| format!("plugin {name:?}: the servers it brings, if any")),
    );
    left
}

/// The `.codex/config.toml` of each folder Codex reads a project layer from
/// for a session in `folder`, lowest first: the project root's (the folder
/// holding `.git`) down to the folder's own, or the folder's alone outside a
/// repository. Not the account's own folder, should it be one of them.
fn project_layers(account_path: &str, folder: &str) -> Vec<PathBuf> {
    let folder = Path::new(folder);
    let root = folder.ancestors().find(|d| d.join(".git").exists());
    let dirs: Vec<&Path> = match root {
        Some(root) => folder.ancestors().take_while(|d| *d != root).chain([root]).collect(),
        None => vec![folder],
    };
    dirs.into_iter()
        .rev()
        .map(|d| d.join(".codex"))
        .filter(|codex| !crate::pty::same_dir(&codex.to_string_lossy(), account_path))
        .map(|codex| codex.join("config.toml"))
        .collect()
}

/// What Codex's own config stack sets `developer_instructions` to for a
/// session on `account_path` in `folder`, and what Luna could not be sure of.
#[derive(Debug, Default, PartialEq)]
pub struct StackInstructions {
    pub value: Option<String>,
    /// Layers Luna could not read, or that come above its `-c`: what it
    /// gives may not be what the session gets, said in what Luna applied.
    pub unsure: Vec<String>,
}

/// The managed layers Codex reads above the command line's `-c` (0.167.2:
/// `managed_config.toml` in `CODEX_HOME`, and in `/etc/codex` on Unix).
fn managed_layers(account_path: &str) -> Vec<PathBuf> {
    let mut out = vec![Path::new(account_path).join("managed_config.toml")];
    if cfg!(unix) {
        out.push(PathBuf::from("/etc/codex/managed_config.toml"));
    }
    out
}

/// `developer_instructions` as Codex's config stack sets it, which a `-c` of
/// Luna's replaces whole (a higher layer's string takes a lower one's place,
/// codex-rs/config): the system layer (`/etc/codex/config.toml`, Unix), the
/// account's `config.toml`, the project's layers where the run trusts the
/// project (`project_trusted`), each later one's value over the earlier; then
/// the profile the stack selects (`profile`, `[profiles.<name>]`), whose
/// value goes over them all. A managed layer comes above the `-c` itself:
/// one that sets the key, or one Luna cannot read, is said in `unsure`, and
/// the session is started all the same (ADR-0026).
pub fn developer_instructions_in(account_path: &str, folder: &str, project_trusted: bool) -> StackInstructions {
    let mut out = StackInstructions::default();
    let mut files: Vec<PathBuf> = Vec::new();
    if cfg!(unix) {
        files.push(PathBuf::from("/etc/codex/config.toml"));
    }
    files.push(Path::new(account_path).join("config.toml"));
    if project_trusted {
        files.extend(project_layers(account_path, folder));
    }
    let mut profile: Option<String> = None;
    let mut docs = Vec::new();
    for file in files {
        match std::fs::read_to_string(&file) {
            Ok(text) => match text.parse::<DocumentMut>() {
                Ok(doc) => {
                    if let Some(v) = string_at(&doc, "developer_instructions") {
                        out.value = Some(v);
                    }
                    if let Some(p) = string_at(&doc, "profile") {
                        profile = Some(p);
                    }
                    docs.push(doc);
                }
                Err(e) => out.unsure.push(format!("{} does not parse ({e}): its developer_instructions not read", file.display())),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => out.unsure.push(format!("{}: {e}: its developer_instructions not read", file.display())),
        }
    }
    if let Some(profile) = profile {
        for doc in &docs {
            if let Some(v) = doc.get("profiles").and_then(|p| p.get(&profile)).and_then(|p| p.get("developer_instructions")) {
                if let Some(s) = v.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                    out.value = Some(s.to_string());
                }
            }
        }
    }
    for file in managed_layers(account_path) {
        match std::fs::read_to_string(&file) {
            Ok(text) => match text.parse::<DocumentMut>() {
                Ok(doc) if doc.get("developer_instructions").is_some() => out.unsure.push(format!(
                    "{} sets developer_instructions, above the command line's: what Luna gives may not reach the session",
                    file.display()
                )),
                Ok(_) => {}
                Err(e) => {
                    out.unsure.push(format!("{} does not parse ({e}): it may set developer_instructions over Luna's", file.display()))
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => out.unsure.push(format!("{}: {e}: it may set developer_instructions over Luna's", file.display())),
        }
    }
    out
}

/// The plugins on in `layers` merged lowest first, as Codex 0.167.0 merges
/// its config stack: table by table, so a later layer's `enabled` replaces
/// an earlier one's and a table that does not say keeps it
/// (codex-rs/config/src/merge.rs). A plugin named under `[plugins]` is on
/// unless its merged `enabled` is false; one that says nothing follows
/// `[plugins._default] enabled`, true without it (core-plugins/src/loader.rs
/// `load_plugins_from_layer_stack_with_scope`, config/src/types.rs
/// `allows_plugin`). Sorted, as Codex loads them.
fn plugins_on(layers: &[DocumentMut]) -> Vec<String> {
    const DEFAULT: &str = "_default";
    let mut said: std::collections::BTreeMap<String, Option<bool>> = Default::default();
    let mut default = None;
    for doc in layers {
        let Some(plugins) = doc.get("plugins").and_then(|t| t.as_table_like()) else { continue };
        for (name, v) in plugins.iter() {
            let enabled = v.get("enabled").and_then(|e| e.as_bool());
            if name == DEFAULT {
                default = enabled.or(default);
                continue;
            }
            let entry = said.entry(name.to_string()).or_default();
            *entry = enabled.or(*entry);
        }
    }
    said.into_iter().filter(|(_, enabled)| enabled.unwrap_or(default.unwrap_or(true))).map(|(name, _)| name).collect()
}

/// For lookups only: a config that will not parse simply has nothing to say.
/// One look, as nothing is written back.
pub fn read_or_empty(path: &Path) -> DocumentMut {
    std::fs::read_to_string(path).ok().and_then(|t| t.parse().ok()).unwrap_or_default()
}

/// Writes via a sibling temp file so a crash mid-write cannot truncate the
/// account's whole config. The temp name is this write's own (process and
/// count), so two writes at once never share one.
pub fn write(path: &Path, doc: &DocumentMut) -> Result<(), String> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("toml.{}-{n}.tmp", std::process::id()));
    std::fs::write(&tmp, doc.to_string()).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
    // A rename refused (Codex holding the file) leaves no temp file behind.
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}

/// Whether the account's config.toml names a `windows.sandbox`: the user's
/// choice, which Luna does not override with its own (codex/launch.rs
/// `codex_windows_sandbox`). Not a project's `.codex/config.toml`: that is
/// the repository's say, and Luna's sandbox stands over it as ever.
pub fn windows_sandbox_set(account_path: &str) -> bool {
    account_file(account_path).is_ok_and(|f| read_or_empty(&f).get("windows").and_then(|w| w.get("sandbox")).is_some())
}

/// A string-valued top-level key, trimmed; None for anything else.
pub fn string_at(doc: &DocumentMut, key: &str) -> Option<String> {
    let s = doc.get(key)?.as_str()?.trim();
    (!s.is_empty()).then(|| s.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_accounts_servers_are_read_or_the_reading_fails() {
        let root = std::env::temp_dir().join(format!("luna-codex-servers-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let account = root.to_string_lossy().into_owned();
        assert_eq!(mcp_servers(&account), Ok(Vec::new()), "no file: no servers");
        std::fs::write(root.join("config.toml"), "model = \"m\"\n[mcp_servers.gmail]\ncommand = \"node\"\n").unwrap();
        assert_eq!(mcp_servers(&account), Ok(vec![("gmail".to_string(), vec!["command".to_string()])]));
        // Caught mid-write, or broken: an error, never a short list.
        std::fs::write(root.join("config.toml"), "[mcp_servers.gmail\ncommand = ").unwrap();
        assert!(mcp_servers(&account).is_err());
        assert!(helper_mcp_servers(&account).unwrap_err().contains("not started"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn what_a_helper_keeps_is_its_projects_servers_and_its_accounts_plugins() {
        let root = std::env::temp_dir().join(format!("luna-codex-left-on-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (account, repo) = (root.join("acc"), root.join("repo"));
        let sub = repo.join("crates").join("app");
        for d in [&account, &repo.join(".git"), &sub.join(".codex"), &repo.join(".codex"), &root.join(".codex")] {
            std::fs::create_dir_all(d).unwrap();
        }
        let (a, s) = (account.to_string_lossy().into_owned(), sub.to_string_lossy().into_owned());
        assert!(servers_left_on(&a, &s).is_empty(), "no config anywhere");
        let write = |dir: &Path, text: &str| std::fs::write(dir.join("config.toml"), text).unwrap();
        write(&repo.join(".codex"), "[mcp_servers.docs]\ncommand = \"x\"\n");
        write(&sub.join(".codex"), "[mcp_servers.\"team.db\"]\nurl = \"u\"\n");
        // Above the repository's root: not a layer of this project.
        write(&root.join(".codex"), "[mcp_servers.above]\ncommand = \"x\"\n");
        write(
            &account,
            "[mcp_servers.gmail]\ncommand = \"x\"\n[plugins.\"crm@market\"]\nenabled = true\n\
             [plugins.\"off@market\"]\nenabled = false\n[plugins.\"repo@luna\"]\nenabled = true\n",
        );
        let left = servers_left_on(&a, &s);
        assert_eq!(left.len(), 3, "{left:?}");
        assert!(left[0].starts_with("server \"docs\" of ") && left[0].contains("repo"), "root first: {left:?}");
        assert!(left[1].starts_with("server \"team.db\" of "), "{left:?}");
        assert!(left[2].starts_with("plugin \"crm@market\""), "{left:?}");
        assert!(!left.iter().any(|l| l.contains("above") || l.contains("gmail") || l.contains("off@") || l.contains("@luna")));
        // Outside a repository, the folder's own layer alone.
        let _ = std::fs::remove_dir_all(repo.join(".git"));
        let left = servers_left_on(&a, &s);
        assert!(left[0].starts_with("server \"team.db\"") && left.len() == 2, "{left:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The plugins a helper keeps are those on in the config Codex merges for
    /// its run, the account's and then each project layer, a later one's
    /// `enabled` over an earlier one's (codex-rs/config merge.rs; core-plugins
    /// loader.rs and marketplace_policy.rs at rust-v0.167.0).
    #[test]
    fn plugins_left_on_follow_the_layers_as_codex_merges_them() {
        let root = std::env::temp_dir().join(format!("luna-codex-plugins-on-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (account, repo) = (root.join("acc"), root.join("repo"));
        for d in [&account, &repo.join(".git"), &repo.join(".codex")] {
            std::fs::create_dir_all(d).unwrap();
        }
        let (a, r) = (account.to_string_lossy().into_owned(), repo.to_string_lossy().into_owned());
        let write = |dir: &Path, text: &str| std::fs::write(dir.join("config.toml"), text).unwrap();
        let plugins = |left: Vec<String>| left.into_iter().filter(|l| l.starts_with("plugin ")).collect::<Vec<_>>();
        let named = |id: &str| format!("plugin {id:?}: the servers it brings, if any");

        // On in the project alone.
        write(&account, "");
        write(&repo.join(".codex"), "[plugins.\"crm@market\"]\nenabled = true\n");
        assert_eq!(plugins(servers_left_on(&a, &r)), [named("crm@market")], "the project's own");
        // On in the account, off in the project.
        write(&account, "[plugins.\"crm@market\"]\nenabled = true\n");
        write(&repo.join(".codex"), "[plugins.\"crm@market\"]\nenabled = false\n");
        assert!(plugins(servers_left_on(&a, &r)).is_empty(), "the project turns it off");
        // Off in the account, on in the project.
        write(&account, "[plugins.\"crm@market\"]\nenabled = false\n");
        write(&repo.join(".codex"), "[plugins.\"crm@market\"]\nenabled = true\n");
        assert_eq!(plugins(servers_left_on(&a, &r)), [named("crm@market")], "the project turns it on");
        // A later table that does not say `enabled` keeps the earlier one's.
        write(&repo.join(".codex"), "[plugins.\"crm@market\"]\nrelease_id = \"r\"\n");
        assert!(plugins(servers_left_on(&a, &r)).is_empty(), "still off");
        // `_default` stands for one that says nothing, and is no plugin itself.
        write(&account, "[plugins._default]\nenabled = false\n[plugins.\"crm@market\"]\n[plugins.\"web@market\"]\nenabled = true\n");
        write(&repo.join(".codex"), "");
        assert_eq!(plugins(servers_left_on(&a, &r)), [named("web@market")], "the default off, one turned on");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_windows_sandbox_of_the_accounts_is_the_users_choice_and_a_projects_is_not() {
        let root = std::env::temp_dir().join(format!("luna-codex-winsandbox-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (account, project) = (root.join("acc"), root.join("proj"));
        std::fs::create_dir_all(&account).unwrap();
        std::fs::create_dir_all(project.join(".codex")).unwrap();
        let a = account.to_string_lossy().into_owned();
        assert!(!windows_sandbox_set(&a), "no config at all");
        std::fs::write(account.join("config.toml"), "model = \"x\"\n[windows]\nother = 1\n").unwrap();
        assert!(!windows_sandbox_set(&a), "a [windows] table without a sandbox");
        std::fs::write(account.join("config.toml"), "[windows]\nsandbox = \"elevated\"\n").unwrap();
        assert!(windows_sandbox_set(&a), "the account's");
        std::fs::write(account.join("config.toml"), "model = [broken\n").unwrap();
        assert!(!windows_sandbox_set(&a), "a config that will not parse says nothing");
        std::fs::write(project.join(".codex").join("config.toml"), "windows.sandbox = \"elevated\"\n").unwrap();
        assert!(!windows_sandbox_set(&a), "the project's is the repository's say, not the user's");
        std::fs::write(account.join("config.toml"), "windows.sandbox = \"elevated\"\n").unwrap();
        assert!(windows_sandbox_set(&a), "the account's, dotted");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_stack_s_developer_instructions_are_the_highest_layer_s_and_a_trusted_project_s() {
        let root = std::env::temp_dir().join(format!("luna-codex-devinstr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (account, repo) = (root.join("acc"), root.join("repo"));
        let sub = repo.join("app");
        for d in [&account, &repo.join(".git"), &repo.join(".codex"), &sub.join(".codex")] {
            std::fs::create_dir_all(d).unwrap();
        }
        let (a, s) = (account.to_string_lossy().into_owned(), sub.to_string_lossy().into_owned());
        let value = |trusted| developer_instructions_in(&a, &s, trusted);
        assert_eq!(value(true), StackInstructions::default(), "nothing set anywhere");
        std::fs::write(account.join("config.toml"), "developer_instructions = \"The account's.\"\n").unwrap();
        assert_eq!(value(true).value.as_deref(), Some("The account's."));
        std::fs::write(repo.join(".codex").join("config.toml"), "developer_instructions = \"The repository's.\"\n").unwrap();
        assert_eq!(value(true).value.as_deref(), Some("The repository's."));
        std::fs::write(sub.join(".codex").join("config.toml"), "developer_instructions = '''The folder's.'''\n").unwrap();
        assert_eq!(value(true).value.as_deref(), Some("The folder's."), "the deepest layer last");
        assert_eq!(value(false).value.as_deref(), Some("The account's."), "an untrusted project's layers are not read");
        // A profile the stack selects goes over every layer.
        std::fs::write(
            account.join("config.toml"),
            "profile = \"work\"\ndeveloper_instructions = \"The account's.\"\n[profiles.work]\ndeveloper_instructions = \"Work's.\"\n",
        )
        .unwrap();
        assert_eq!(value(true).value.as_deref(), Some("Work's."));
        // One that will not parse, and a managed layer over the `-c`: said, not refused.
        std::fs::write(sub.join(".codex").join("config.toml"), "developer_instructions = [broken\n").unwrap();
        let v = value(true);
        assert!(v.unsure.len() == 1 && v.unsure[0].contains("does not parse"), "{:?}", v.unsure);
        std::fs::write(account.join("managed_config.toml"), "developer_instructions = \"IT's.\"\n").unwrap();
        assert!(value(false).unsure.iter().any(|u| u.contains("above the command line's")));
        let _ = std::fs::remove_dir_all(&root);
    }
}
