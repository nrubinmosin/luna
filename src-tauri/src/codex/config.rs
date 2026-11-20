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
/// document loses nothing of it. Read as a settings file is
/// (`boundary::read_settings_file`): a plain file of at most
/// `boundary::SETTINGS_MAX` bytes, a link to one followed, so a FIFO there
/// holds no sync, trust mark or start that reads it.
pub fn read(path: &Path) -> Result<DocumentMut, String> {
    let mut looks = 0;
    loop {
        let read = crate::boundary::read_settings_file(path, crate::boundary::SETTINGS_MAX)
            .and_then(|bytes| bytes.map(|b| String::from_utf8(b).map_err(std::io::Error::other)).transpose());
        let got = match read {
            Ok(Some(text)) => match text.parse::<DocumentMut>() {
                Ok(doc) if !text.trim().is_empty() => return Ok(doc),
                Ok(doc) => Ok(doc),
                Err(e) => Err(format!("{}: {e}", path.display())),
            },
            Ok(None) => return Ok(DocumentMut::new()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        };
        looks += 1;
        if looks == 3 {
            return got;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// What a helper started without connectors turns off for its run
/// (codex/launch.rs `connectors_off`): every MCP server the layers Codex
/// merges for it define, by name with the keys their tables set, and every
/// plugin on in them but the library's (`plugins_on`), whose servers Luna
/// cannot name. The layers: Codex's system one (`system_layer`), the
/// account's, and the project's (`project_layers`), as a helper's folder is
/// trusted for its run.
#[derive(Debug, Default, PartialEq)]
pub struct HelperMcp {
    pub servers: Vec<(String, Vec<String>)>,
    pub plugins: Vec<String>,
}

/// `HelperMcp` for a helper's start in `folder`, which goes no further
/// without it: a layer that will not read or parse, after `read`'s looks
/// again, is an error and not a short list, as a server left out of it would
/// stay on. So is a managed layer setting servers, plugins or apps: it comes
/// above the command line's `-c`, which cannot turn them off.
pub fn helper_mcp(account_path: &str, folder: &str) -> Result<HelperMcp, String> {
    let refused =
        |e: String| format!("the helper is not started: Luna could not read which MCP servers its run would have, to turn them off ({e})");
    let mut files = vec![system_layer(), account_file(account_path)?];
    files.extend(project_layers(account_path, folder).map_err(refused)?);
    let mut docs = Vec::new();
    let mut servers: Vec<(String, Vec<String>)> = Vec::new();
    for file in &files {
        let doc = read(file).map_err(refused)?;
        for (name, keys) in servers_in(&doc) {
            match servers.iter_mut().find(|(n, _)| *n == name) {
                Some((_, all)) => all.extend(keys.into_iter().filter(|k| !all.contains(k)).collect::<Vec<_>>()),
                None => servers.push((name, keys)),
            }
        }
        docs.push(doc);
    }
    for file in managed_layers(account_path) {
        let doc = read(&file).map_err(refused)?;
        let apps = doc.get("features").and_then(|f| f.get("apps")).is_some();
        if doc.get("mcp_servers").is_some() || doc.get("plugins").is_some() || apps {
            return Err(format!(
                "the helper is not started: {} sets MCP servers, plugins or apps above the command line, where no -c can \
                 turn them off for its run; spawn with connectors: true",
                file.display()
            ));
        }
    }
    let luna = format!("@{}", crate::library::CODEX_MARKET);
    let plugins = plugins_on(&docs).into_iter().filter(|name| !name.ends_with(&luna)).collect();
    Ok(HelperMcp { servers, plugins })
}

/// The MCP servers one layer defines, each by its name as written with the
/// keys its table sets.
fn servers_in(doc: &DocumentMut) -> Vec<(String, Vec<String>)> {
    let keys = |v: &toml_edit::Item| v.as_table_like().map(|t| t.iter().map(|(k, _)| k.to_string()).collect()).unwrap_or_default();
    let servers = doc.get("mcp_servers").and_then(|t| t.as_table_like());
    servers.map(|t| t.iter().map(|(name, v)| (name.to_string(), keys(v))).collect()).unwrap_or_default()
}

/// Codex's system layer, under the account's in its stack (0.169.0,
/// codex-rs/config loader `system_config_toml_file`):
/// `%ProgramData%\OpenAI\Codex\config.toml` on Windows, `/etc/codex/config.toml`
/// elsewhere.
pub(crate) fn system_layer() -> PathBuf {
    if cfg!(windows) {
        let data = std::env::var_os("ProgramData").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
        data.join("OpenAI").join("Codex").join("config.toml")
    } else {
        PathBuf::from("/etc/codex/config.toml")
    }
}

/// The `.codex/config.toml` of each folder Codex reads a project layer from
/// for a session in `folder`, lowest first, as 0.169.0 finds them
/// (codex-rs/config loader `discover_project_root`, `discover_project_layers`):
/// from the project root down to the folder, or the folder's alone without
/// one. The root is the nearest folder holding one of the account's
/// `project_root_markers` (system and account layers, the later one's; `.git`
/// without one; none for an empty list), where a `.git` folder counts only
/// with a `HEAD` in it. Not the account's own folder, should it be one of
/// them. Err where the markers cannot be read, as Codex would not start.
pub(crate) fn project_layers(account_path: &str, folder: &str) -> Result<Vec<PathBuf>, String> {
    project_root_and_layers(account_path, folder).map(|(_, layers)| layers)
}

/// `project_layers` and the project root they start from: the folder itself
/// where no marker is found, as Codex has it.
pub(crate) fn project_root_and_layers(account_path: &str, folder: &str) -> Result<(PathBuf, Vec<PathBuf>), String> {
    let mut markers = vec![".git".to_string()];
    for file in [system_layer(), account_file(account_path)?] {
        let doc = read(&file)?;
        if let Some(item) = doc.get("project_root_markers") {
            let list = item.as_array().ok_or_else(|| format!("{}: project_root_markers is not a list", file.display()))?;
            markers = list
                .iter()
                .map(|m| {
                    m.as_str().map(str::to_owned).ok_or_else(|| format!("{}: project_root_markers holds a non-string", file.display()))
                })
                .collect::<Result<_, _>>()?;
        }
    }
    let folder = Path::new(folder);
    let marks = |d: &Path| {
        markers.iter().any(|m| {
            let p = d.join(m);
            p.exists() && (m != ".git" || !p.is_dir() || p.join("HEAD").exists())
        })
    };
    let root = folder.ancestors().find(|d| marks(d));
    let dirs: Vec<&Path> = match root {
        Some(root) => folder.ancestors().take_while(|d| *d != root).chain([root]).collect(),
        None => vec![folder],
    };
    let layers = dirs
        .into_iter()
        .rev()
        .map(|d| d.join(".codex"))
        .filter(|codex| codex.is_dir() && !crate::pty::same_dir(&codex.to_string_lossy(), account_path))
        .map(|codex| codex.join("config.toml"))
        .collect();
    Ok((root.unwrap_or(folder).to_path_buf(), layers))
}

/// The folder a project layer's `.codex/config.toml` is in.
pub(crate) fn layer_dir(file: &Path) -> String {
    file.parent().and_then(Path::parent).map(|d| d.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The project layers whose `developer_instructions` a run of `account_path`
/// in `folder` reads: those Codex trusts (`super::trust::Decisions`), and
/// none at or below a folder saved as untrusted, which Luna leaves out where
/// unsure: a layer left out loses its text from Luna's `-c`, one put in would
/// bring back text Codex skipped.
fn instruction_layers(account_path: &str, folder: &str, helper: bool) -> Result<Vec<PathBuf>, String> {
    let (root, layers) = project_root_and_layers(account_path, folder)?;
    let trust = super::trust::Decisions::of(account_path, folder, helper);
    let root = root.to_string_lossy().into_owned();
    Ok(layers
        .into_iter()
        .filter(|file| {
            let dir = layer_dir(file);
            trust.layer_trusted(&dir, &root, folder) && !trust.untrusted_between(&root, &dir)
        })
        .collect())
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
/// codex-rs/config): the system layer (`system_layer`), the
/// account's `config.toml`, the project's layers the run trusts
/// (`instruction_layers`; `helper`: a helper's, its folder trusted for its
/// run), each later one's value over the earlier. No
/// profile: 0.167.2 refuses to start on a `profile = "<name>"` in a config
/// (`[profiles.<name>]` is legacy), and a `--profile`, which Luna never
/// passes, is a user layer (`<name>.config.toml`) below the `-c`
/// (codex-rs/config `ConfigLayerSource::precedence`; docs/cli-compatibility.md).
/// A managed layer comes above the `-c` itself:
/// one that sets the key, or one Luna cannot read, is said in `unsure`, and
/// the session is started all the same (ADR-0026).
pub fn developer_instructions_in(account_path: &str, folder: &str, helper: bool) -> StackInstructions {
    let mut out = StackInstructions::default();
    let mut files = vec![system_layer(), Path::new(account_path).join("config.toml")];
    match instruction_layers(account_path, folder, helper) {
        Ok(layers) => files.extend(layers),
        Err(e) => out.unsure.push(format!("{e}: the project's developer_instructions not read")),
    }
    for file in files {
        match std::fs::read_to_string(&file) {
            Ok(text) => match text.parse::<DocumentMut>() {
                Ok(doc) => {
                    if let Some(v) = string_at(&doc, "developer_instructions") {
                        out.value = Some(v);
                    }
                }
                Err(e) => out.unsure.push(format!("{} does not parse ({e}): its developer_instructions not read", file.display())),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => out.unsure.push(format!("{}: {e}: its developer_instructions not read", file.display())),
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

    /// A repository's root as Codex 0.169.0 knows one: a `.git` folder with
    /// a `HEAD` in it.
    fn repo_at(dir: &Path) {
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
    }

    /// An account's `config.toml` past the cap does not read, and on Unix
    /// neither does a FIFO in its place, at once rather than waited on: the
    /// settings sync, a trust mark and a start that read it go no further.
    #[test]
    fn a_config_too_large_or_no_file_does_not_read_and_holds_nothing_up() {
        let root = std::env::temp_dir().join(format!("luna-codex-config-bounded-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(read(&root.join("missing.toml")).unwrap().to_string(), "", "none is an empty document");
        let big = root.join("big.toml");
        std::fs::write(&big, format!("model = \"x\"\n# {}\n", " ".repeat(crate::boundary::SETTINGS_MAX as usize))).unwrap();
        assert!(read(&big).unwrap_err().contains("over"));
        #[cfg(unix)]
        {
            let fifo = root.join("config.toml");
            crate::claude::defaults::tests::mkfifo(&fifo);
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(read(&fifo).err());
            });
            let err = rx.recv_timeout(std::time::Duration::from_secs(5)).expect("answered without waiting on a FIFO");
            assert!(err.is_some_and(|e| e.contains("not a plain file")));
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_helpers_servers_are_read_or_the_reading_fails() {
        let root = std::env::temp_dir().join(format!("luna-codex-servers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let folder = root.join("work");
        std::fs::create_dir_all(&folder).unwrap();
        let (account, f) = (root.to_string_lossy().into_owned(), folder.to_string_lossy().into_owned());
        assert_eq!(helper_mcp(&account, &f), Ok(HelperMcp::default()), "no file: nothing to turn off");
        std::fs::write(root.join("config.toml"), "model = \"m\"\n[mcp_servers.gmail]\ncommand = \"node\"\n").unwrap();
        assert_eq!(helper_mcp(&account, &f).unwrap().servers, vec![("gmail".to_string(), vec!["command".to_string()])]);
        // Caught mid-write, or broken: an error, never a short list.
        std::fs::write(root.join("config.toml"), "[mcp_servers.gmail\ncommand = ").unwrap();
        assert!(helper_mcp(&account, &f).unwrap_err().contains("not started"));
        // So is a project layer, which could define servers of its own.
        std::fs::write(root.join("config.toml"), "").unwrap();
        std::fs::create_dir_all(folder.join(".codex")).unwrap();
        std::fs::write(folder.join(".codex").join("config.toml"), "[mcp_servers.docs\n").unwrap();
        assert!(helper_mcp(&account, &f).unwrap_err().contains("not started"));
        std::fs::write(folder.join(".codex").join("config.toml"), "").unwrap();
        // A managed layer comes over the command line: no -c turns its servers off.
        std::fs::write(root.join("managed_config.toml"), "[mcp_servers.it]\nurl = \"u\"\n").unwrap();
        let e = helper_mcp(&account, &f).unwrap_err();
        assert!(e.contains("not started") && e.contains("managed_config.toml") && e.contains("connectors: true"), "{e}");
        std::fs::write(root.join("managed_config.toml"), "[plugins.\"crm@m\"]\nenabled = true\n").unwrap();
        assert!(helper_mcp(&account, &f).is_err(), "nor its plugins");
        std::fs::write(root.join("managed_config.toml"), "[features]\napps = true\n").unwrap();
        assert!(helper_mcp(&account, &f).is_err(), "nor its apps");
        std::fs::write(root.join("managed_config.toml"), "model = \"m\"\n").unwrap();
        assert_eq!(helper_mcp(&account, &f), Ok(HelperMcp::default()), "a managed layer setting none of them");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_helper_turns_off_its_accounts_and_projects_servers_and_their_plugins() {
        let root = std::env::temp_dir().join(format!("luna-codex-helper-mcp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (account, repo) = (root.join("acc"), root.join("repo"));
        let sub = repo.join("crates").join("app");
        for d in [&account, &sub.join(".codex"), &repo.join(".codex"), &root.join(".codex")] {
            std::fs::create_dir_all(d).unwrap();
        }
        repo_at(&repo);
        let (a, s) = (account.to_string_lossy().into_owned(), sub.to_string_lossy().into_owned());
        assert_eq!(helper_mcp(&a, &s), Ok(HelperMcp::default()), "no config anywhere");
        let write = |dir: &Path, text: &str| std::fs::write(dir.join("config.toml"), text).unwrap();
        write(&repo.join(".codex"), "[mcp_servers.docs]\ncommand = \"x\"\n[mcp_servers.gmail]\nenv = { A = \"1\" }\n");
        write(&sub.join(".codex"), "[mcp_servers.\"team.db\"]\nurl = \"u\"\n");
        // Above the repository's root: not a layer of this project.
        write(&root.join(".codex"), "[mcp_servers.above]\ncommand = \"x\"\n");
        write(
            &account,
            "[mcp_servers.gmail]\ncommand = \"x\"\n[plugins.\"crm@market\"]\nenabled = true\n\
             [plugins.\"off@market\"]\nenabled = false\n[plugins.\"repo@luna\"]\nenabled = true\n",
        );
        let names = |got: HelperMcp| got.servers.into_iter().map(|(n, _)| n).collect::<Vec<_>>();
        let got = helper_mcp(&a, &s).unwrap();
        assert_eq!(got.servers[0].1, ["command", "env"], "a server's keys from every layer that sets it");
        assert_eq!(got.plugins, ["crm@market"], "on, and not the library's");
        assert_eq!(names(got), ["gmail", "docs", "team.db"], "the account's, then the project's from its root down");
        // Without a root marker, the folder's own layer alone.
        let _ = std::fs::remove_dir_all(repo.join(".git"));
        assert_eq!(names(helper_mcp(&a, &s).unwrap()), ["gmail", "team.db"]);
        // A `.git` folder without a HEAD is no root to Codex either.
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        assert_eq!(names(helper_mcp(&a, &s).unwrap()), ["gmail", "team.db"]);
        // The account's own root markers, as Codex reads them before the project's.
        write(&account, "project_root_markers = [\"Cargo.lock\"]\n");
        std::fs::write(repo.join("Cargo.lock"), "").unwrap();
        assert_eq!(names(helper_mcp(&a, &s).unwrap()), ["docs", "gmail", "team.db"]);
        write(&account, "project_root_markers = []\n");
        assert_eq!(names(helper_mcp(&a, &s).unwrap()), ["team.db"], "no root at all: the folder's own");
        write(&account, "project_root_markers = \".git\"\n");
        assert!(helper_mcp(&a, &s).is_err(), "markers Codex would refuse");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The plugins a helper turns off are those on in the config Codex merges
    /// for its run, the account's and then each project layer, a later one's
    /// `enabled` over an earlier one's (codex-rs/config merge.rs; core-plugins
    /// loader.rs and marketplace_policy.rs at rust-v0.167.0).
    #[test]
    fn plugins_on_follow_the_layers_as_codex_merges_them() {
        let root = std::env::temp_dir().join(format!("luna-codex-plugins-on-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (account, repo) = (root.join("acc"), root.join("repo"));
        for d in [&account, &repo.join(".codex")] {
            std::fs::create_dir_all(d).unwrap();
        }
        repo_at(&repo);
        let (a, r) = (account.to_string_lossy().into_owned(), repo.to_string_lossy().into_owned());
        let write = |dir: &Path, text: &str| std::fs::write(dir.join("config.toml"), text).unwrap();
        let plugins = || helper_mcp(&a, &r).unwrap().plugins;

        // On in the project alone.
        write(&account, "");
        write(&repo.join(".codex"), "[plugins.\"crm@market\"]\nenabled = true\n");
        assert_eq!(plugins(), ["crm@market"], "the project's own");
        // On in the account, off in the project.
        write(&account, "[plugins.\"crm@market\"]\nenabled = true\n");
        write(&repo.join(".codex"), "[plugins.\"crm@market\"]\nenabled = false\n");
        assert!(plugins().is_empty(), "the project turns it off");
        // Off in the account, on in the project.
        write(&account, "[plugins.\"crm@market\"]\nenabled = false\n");
        write(&repo.join(".codex"), "[plugins.\"crm@market\"]\nenabled = true\n");
        assert_eq!(plugins(), ["crm@market"], "the project turns it on");
        // A later table that does not say `enabled` keeps the earlier one's.
        write(&repo.join(".codex"), "[plugins.\"crm@market\"]\nrelease_id = \"r\"\n");
        assert!(plugins().is_empty(), "still off");
        // `_default` stands for one that says nothing, and is no plugin itself.
        write(&account, "[plugins._default]\nenabled = false\n[plugins.\"crm@market\"]\n[plugins.\"web@market\"]\nenabled = true\n");
        write(&repo.join(".codex"), "");
        assert_eq!(plugins(), ["web@market"], "the default off, one turned on");
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
        for d in [&account, &repo.join(".codex"), &sub.join(".codex")] {
            std::fs::create_dir_all(d).unwrap();
        }
        repo_at(&repo);
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
        // A legacy profile is no layer of 0.167.2's (it refuses to start on
        // the selector), and a `--profile` file sits below the `-c`: the
        // account's own value stands, and Luna's goes over both.
        std::fs::write(
            account.join("config.toml"),
            "profile = \"work\"\ndeveloper_instructions = \"The account's.\"\n[profiles.work]\ndeveloper_instructions = \"Work's.\"\n",
        )
        .unwrap();
        std::fs::write(account.join("work.config.toml"), "developer_instructions = \"Work's.\"\n").unwrap();
        assert_eq!(value(false), StackInstructions { value: Some("The account's.".into()), unsure: Vec::new() });
        // One that will not parse, and a managed layer over the `-c`: said, not refused.
        std::fs::write(sub.join(".codex").join("config.toml"), "developer_instructions = [broken\n").unwrap();
        let v = value(true);
        assert!(v.unsure.len() == 1 && v.unsure[0].contains("does not parse"), "{:?}", v.unsure);
        std::fs::write(account.join("managed_config.toml"), "developer_instructions = \"IT's.\"\n").unwrap();
        assert!(value(false).unsure.iter().any(|u| u.contains("above the command line's")));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A folder saved as untrusted keeps its own layer, and those below it,
    /// out of what Luna's `-c developer_instructions` carries, though the
    /// repository above it is trusted; a linked worktree goes by its main
    /// repository's trust, as Codex resolves it.
    #[test]
    fn instructions_skip_layers_at_or_below_an_untrusted_folder_and_follow_the_main_repository() {
        let root = std::env::temp_dir().join(format!("luna-codex-devinstr-untrusted-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (account, repo) = (root.join("account"), root.join("repo"));
        let (mid, deep) = (repo.join("mid"), repo.join("mid").join("deep"));
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
        for (dir, text) in [(&repo, "The repository's."), (&mid, "The middle's."), (&deep, "The deep one's.")] {
            std::fs::create_dir_all(dir.join(".codex")).unwrap();
            std::fs::write(dir.join(".codex").join("config.toml"), format!("developer_instructions = \"{text}\"\n")).unwrap();
        }
        std::fs::create_dir_all(&account).unwrap();
        let key = |p: &Path| toml_edit::Value::from(p.to_string_lossy().into_owned()).to_string();
        std::fs::write(
            account.join("config.toml"),
            format!("[projects.{}]\ntrust_level = \"trusted\"\n[projects.{}]\ntrust_level = \"untrusted\"\n", key(&repo), key(&mid)),
        )
        .unwrap();
        let a = account.to_string_lossy().into_owned();
        let value = |folder: &Path| developer_instructions_in(&a, &folder.to_string_lossy(), false).value;
        assert_eq!(value(&deep).as_deref(), Some("The repository's."), "the middle and below it left out");
        assert_eq!(value(&repo).as_deref(), Some("The repository's."));

        // A linked worktree of the trusted repository: trusted by it.
        let wt = root.join("wt");
        let gitdir = repo.join(".git").join("worktrees").join("wt");
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::create_dir_all(wt.join(".codex")).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", gitdir.to_string_lossy())).unwrap();
        std::fs::write(wt.join(".codex").join("config.toml"), "developer_instructions = \"The worktree's.\"\n").unwrap();
        assert_eq!(value(&wt).as_deref(), Some("The worktree's."));
        let _ = std::fs::remove_dir_all(&root);
    }
}
