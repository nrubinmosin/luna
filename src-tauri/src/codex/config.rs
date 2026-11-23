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
/// holds no sync, trust mark or start that reads it. A parse error says
/// where (`not_toml`), never what is there.
pub fn read(path: &Path) -> Result<DocumentMut, String> {
    #[cfg(test)]
    if EMPTY_MACHINE_LAYERS.get() && (path == system_layer() || (cfg!(unix) && path == Path::new(SYSTEM_MANAGED_LAYER))) {
        return Ok(DocumentMut::new());
    }
    let mut looks = 0;
    loop {
        let read = crate::boundary::read_settings_file(path, crate::boundary::SETTINGS_MAX)
            .and_then(|bytes| bytes.map(|b| String::from_utf8(b).map_err(std::io::Error::other)).transpose());
        let got = match read {
            Ok(Some(text)) => match text.parse::<DocumentMut>() {
                Ok(doc) if !text.trim().is_empty() => return Ok(doc),
                Ok(doc) => Ok(doc),
                Err(e) => Err(not_toml(path, &text, &e)),
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

/// A file that does not parse as TOML, by its path and the line and column
/// where it stops parsing: not toml_edit's own message, which quotes the line
/// and can name a key of it, and goes where the start's error goes (a
/// helper's failure record, a chat's error). The file may be a link to one
/// that is not a config at all.
fn not_toml(path: &Path, text: &str, e: &toml_edit::TomlError) -> String {
    format!("{} {}", path.display(), toml_unparsed(text, e))
}

/// `not_toml` without the path, for a text whose file the caller names:
/// "does not parse as TOML (line L, column C)", nothing of the text.
pub(crate) fn toml_unparsed(text: &str, e: &toml_edit::TomlError) -> String {
    let Some(at) = e.span().map(|s| s.start.min(text.len())) else {
        return "does not parse as TOML".into();
    };
    let before = text.get(..at).unwrap_or(text);
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    format!("does not parse as TOML (line {line}, column {column})")
}

#[cfg(test)]
thread_local! {
    static EMPTY_MACHINE_LAYERS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Isolate a test's launch from machine policy while keeping its account
/// and project fixtures. Each test runs on its own thread.
#[cfg(test)]
pub(crate) fn use_empty_machine_layers_for_test() {
    EMPTY_MACHINE_LAYERS.set(true);
}

/// What a helper started without connectors turns off for its run
/// (codex/launch.rs `connectors_off`): every MCP server the layers Codex
/// merges for it define, by name with the keys their tables set, and every
/// plugin on in them but the library's (`plugins_on`), whose servers Luna
/// cannot name. The layers: those Codex loads for the helper's run
/// (`loaded_layers`, its folder trusted for it).
#[derive(Debug, Default, PartialEq)]
pub struct HelperMcp {
    pub servers: Vec<(String, Vec<String>)>,
    pub plugins: Vec<String>,
}

/// `HelperMcp` for a helper's start in `folder`, which goes no further
/// without it: a layer Codex loads that will not read or parse, after
/// `read`'s looks again, is an error and not a short list, as a server left
/// out of it would stay on. So is a managed layer setting servers, plugins or
/// apps: it comes above the command line's `-c`, which cannot turn them off.
/// A project layer Codex leaves out names nothing to turn off: a server only
/// it defines, named in the `-c`, would fail the run ("invalid transport").
pub fn helper_mcp(account_path: &str, folder: &str) -> Result<HelperMcp, String> {
    let refused =
        |e: String| format!("the helper is not started: Luna could not read which MCP servers its run would have, to turn them off ({e})");
    let files = loaded_layers(account_path, folder, true).map_err(refused)?;
    let mut docs = Vec::new();
    let mut servers: Vec<(String, Vec<String>)> = Vec::new();
    for (file, sure) in &files {
        let doc = match read(file) {
            Ok(doc) => doc,
            Err(_) if !sure => continue,
            Err(e) => return Err(refused(e)),
        };
        for (name, keys) in servers_in(&doc) {
            match servers.iter_mut().find(|(n, _)| *n == name) {
                Some((_, all)) => all.extend(keys.into_iter().filter(|k| !all.contains(k)).collect::<Vec<_>>()),
                None => servers.push((name, keys)),
            }
        }
        docs.push(doc);
    }
    for file in managed_layers() {
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

/// The layers below the command line that Codex loads for a session in
/// `folder`, lowest first, each with whether it surely does: its system one
/// (`system_layer`) and the account's, then the project's (`project_root_and_layers`)
/// as Codex decides each (`trust::Decisions`; `helper`: a helper's run, its
/// folder trusted for it). One saved as untrusted Codex leaves out, a parse
/// error in it and all (config/src/loader/mod.rs at rust-v0.171.0), and so
/// does Luna. One with no decision saved is in, but not surely: the person
/// may trust it at Codex's trust screen, after Luna has read it, or never,
/// so a reader takes what it holds but no parse error of it for an error.
fn loaded_layers(account_path: &str, folder: &str, helper: bool) -> Result<Vec<(PathBuf, bool)>, String> {
    let (root, project) = project_root_and_layers(account_path, folder)?;
    let trust = super::trust::Decisions::of(account_path, folder, helper);
    let root = root.to_string_lossy().into_owned();
    let mut files = vec![(system_layer(), true), (account_file(account_path)?, true)];
    for file in project {
        match trust.layer_decision(&layer_dir(&file), &root, folder) {
            Some(false) => {}
            decided => files.push((file, decided.is_some())),
        }
    }
    Ok(files)
}

/// What Codex's config stack sets of auto-compaction for a session
/// (codex/compact.rs), each key the highest layer's that sets it.
#[derive(Debug, Default, PartialEq)]
pub struct Compaction {
    pub context_window: Option<i64>,
    pub token_limit: Option<i64>,
    pub model_catalog: Option<PathBuf>,
    /// `model_auto_compact_token_limit_scope`: `total` unset.
    pub scope: Option<String>,
    pub post_turn_pct: Option<f64>,
}

/// `Compaction` for a chat in `folder`: the
/// layers Codex is known to load below the command line, then the
/// managed one above it; Luna's `-c` names none of these keys. A layer Codex
/// surely loads that will not read is an error: what it says is not known.
pub fn compaction(account_path: &str, folder: &str) -> Result<Compaction, String> {
    let below = loaded_layers(account_path, folder, false)?.into_iter().filter_map(|(file, sure)| sure.then_some(file));
    let mut out = Compaction::default();
    for file in below.chain(managed_layers()) {
        let doc = read(&file)?;
        let int = |key: &str| doc.get(key).and_then(|v| v.as_integer());
        out.context_window = int("model_context_window").or(out.context_window);
        out.token_limit = int("model_auto_compact_token_limit").or(out.token_limit);
        // Keep the spelling from the highest layer so the reader can refuse
        // a relative, share or device path before touching it.
        if let Some(path) = doc.get("model_catalog_json") {
            out.model_catalog = Some(PathBuf::from(path.as_str().unwrap_or_default()));
        }
        out.scope = string_at(&doc, "model_auto_compact_token_limit_scope").or(out.scope);
        let pct = doc.get("model_post_turn_compact_threshold_percent").and_then(|v| v.as_float().or(v.as_integer().map(|n| n as f64)));
        out.post_turn_pct = pct.or(out.post_turn_pct);
    }
    Ok(out)
}

/// A layer that defines an MCP server of a given name (`servers_named`).
#[derive(Debug, PartialEq)]
pub struct NamedServer {
    pub file: PathBuf,
    /// The keys its table sets.
    pub keys: Vec<String>,
    /// A managed layer, above the command line's `-c`.
    pub managed: bool,
}

/// Each layer Codex merges for a session in `folder` that defines an MCP
/// server called `name`, lowest first: those it loads below the command line
/// (`loaded_layers`; `helper` as there), then the managed one above it
/// (`managed_layers`). For a name Luna gives its own server by
/// (codex/launch.rs `luna_server_free`): Codex 0.171.0 merges a table key by
/// key, a `-c`'s included, so what a layer sets there runs as part of
/// Luna's. A layer Codex surely loads that will not read or parse is an
/// error, as it may define one.
pub fn servers_named(account_path: &str, folder: &str, name: &str, helper: bool) -> Result<Vec<NamedServer>, String> {
    // (file, a managed layer, a parse error there is an error)
    let below = loaded_layers(account_path, folder, helper)?.into_iter().map(|(f, sure)| (f, false, sure));
    let layers = below.chain(managed_layers().into_iter().map(|f| (f, true, true)));
    let mut found = Vec::new();
    for (file, managed, sure) in layers {
        let doc = match read(&file) {
            Ok(doc) => doc,
            Err(_) if !sure => continue,
            Err(e) => return Err(e),
        };
        if let Some((_, keys)) = servers_in(&doc).into_iter().find(|(n, _)| n == name) {
            found.push(NamedServer { file, keys, managed });
        }
    }
    Ok(found)
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
/// `<ProgramData>\OpenAI\Codex\config.toml` on Windows, `/etc/codex/config.toml`
/// elsewhere.
pub(crate) fn system_layer() -> PathBuf {
    #[cfg(windows)]
    return program_data().join("OpenAI").join("Codex").join("config.toml");
    #[cfg(not(windows))]
    PathBuf::from("/etc/codex/config.toml")
}

/// The ProgramData folder as Codex finds it, the known folder
/// (`FOLDERID_ProgramData`, config/src/loader/mod.rs at rust-v0.171.0), not
/// the `ProgramData` variable Luna inherits, which whoever started it may
/// have set to anything; `C:\ProgramData` where the shell has none.
#[cfg(windows)]
fn program_data() -> PathBuf {
    known_program_data().unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
}

#[cfg(windows)]
fn known_program_data() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows::core::{GUID, HRESULT, PWSTR};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_ProgramData, KF_FLAG_DEFAULT, KNOWN_FOLDER_FLAG};
    // The call itself, not the windows crate's wrapper, which drops the
    // path it was handed back on a failure: the caller frees it whether the
    // call succeeds or not (SHGetKnownFolderPath's documentation).
    #[link(name = "shell32")]
    extern "system" {
        fn SHGetKnownFolderPath(rfid: *const GUID, flags: KNOWN_FOLDER_FLAG, token: HANDLE, path: *mut PWSTR) -> HRESULT;
    }
    let mut path = PWSTR::null();
    // SAFETY: `path` is the shell's to fill, with a NUL-ended string it
    // allocated with CoTaskMemAlloc, or with null; it is read only on
    // success and when not null, and freed once whatever the call returned.
    let known = unsafe {
        let got = SHGetKnownFolderPath(&FOLDERID_ProgramData, KF_FLAG_DEFAULT, HANDLE::default(), &mut path);
        // Wide as it is: a folder name need not be valid UTF-16.
        let wide = (got.is_ok() && !path.is_null()).then(|| std::ffi::OsString::from_wide(path.as_wide()));
        if !path.is_null() {
            CoTaskMemFree(Some(path.0.cast_const().cast()));
        }
        wide
    };
    known.filter(|p| !p.is_empty()).map(PathBuf::from)
}

/// The `.codex/config.toml` of each folder Codex reads a project layer from
/// for a session in `folder`, lowest first, as 0.169.0 finds them
/// (codex-rs/config loader `discover_project_root`, `discover_project_layers`):
/// from the project root down to the folder, or the folder's alone without
/// one. The root is the nearest folder holding one of the account's
/// `project_root_markers` (system, account and managed layers, the later
/// one's, the managed one's above all as Codex applies it first; `.git`
/// without one; none for an empty list), where a `.git` folder counts only
/// with a `HEAD` in it. Not the account's own folder, should it be one of
/// them. Err where the markers cannot be read, as Codex would not start.
/// With them the project root they start from: the folder itself where no
/// marker is found, as Codex has it.
pub(crate) fn project_root_and_layers(account_path: &str, folder: &str) -> Result<(PathBuf, Vec<PathBuf>), String> {
    // The highest layer's value replaces the others' whole, and only what is
    // left is checked, as Codex checks the merged value: a layer below with
    // one it would refuse refuses nothing.
    let mut said = None;
    for file in [system_layer(), account_file(account_path)?].into_iter().chain(managed_layers()) {
        if let Some(item) = read(&file)?.get("project_root_markers") {
            said = Some((file, item.clone()));
        }
    }
    let mut markers = vec![".git".to_string()];
    if let Some((file, item)) = said {
        let list = item.as_array().ok_or_else(|| format!("{}: project_root_markers is not a list", file.display()))?;
        markers = list
            .iter()
            .map(|m| m.as_str().map(str::to_owned).ok_or_else(|| format!("{}: project_root_markers holds a non-string", file.display())))
            .collect::<Result<_, _>>()?;
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

const SYSTEM_MANAGED_LAYER: &str = "/etc/codex/managed_config.toml";

/// The managed layer Codex reads from a file above the command line's `-c`,
/// for every reader of Luna's: `/etc/codex/managed_config.toml` on Unix, and
/// none on Windows, where `CODEX_HOME/managed_config.toml` is skipped by
/// default (config/src/loader/layer_io.rs at rust-v0.167.0 and
/// rust-v0.171.0; 0.171.0 says it "is no longer supported on Windows"). Its
/// macOS MDM layer is no file Luna reads.
pub(crate) fn managed_layers() -> Vec<PathBuf> {
    #[cfg(test)]
    if let Some(file) = MANAGED_FOR_TEST.with(|m| m.borrow().clone()) {
        return vec![file];
    }
    if cfg!(unix) {
        vec![PathBuf::from(SYSTEM_MANAGED_LAYER)]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
thread_local! {
    /// A file a test puts in the place of the machine's managed layer
    /// (`managed_layers`), for its own thread alone.
    pub(crate) static MANAGED_FOR_TEST: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Codex 0.171 can override or constrain approval above the launch flags.
/// A customer config server and a managed ChatGPT workspace may deliver
/// requirements at startup, so absence of the local cache is insufficient.
pub(super) fn approval_is_unmanaged(account: &str, folder: &str) -> bool {
    if !requirements_clear() || !approval_layers_clear(managed_layers()) {
        return false;
    }
    let Ok(account_file) = account_file(account) else { return false };
    let Ok((_, projects)) = project_root_and_layers(account, folder) else { return false };
    let config_files = [system_layer(), account_file].into_iter().chain(projects);
    let uses_file_credentials = config_files
        .map(|f| read(&f))
        .all(|doc| doc.is_ok_and(|doc| doc.get("cli_auth_credentials_store").is_none_or(|v| v.as_str() == Some("file"))));
    uses_file_credentials && cloud_approval_is_absent(Path::new(account))
}

fn requirements_clear() -> bool {
    #[cfg(test)]
    if !cfg!(target_os = "macos") && EMPTY_MACHINE_LAYERS.get() {
        return true;
    }
    system_requirements().is_some_and(|file| approval_layers_clear([file]))
}

fn approval_layers_clear(files: impl IntoIterator<Item = PathBuf>) -> bool {
    files.into_iter().all(|file| {
        read(&file).is_ok_and(|doc| {
            ["approval_policy", "approvals_reviewer", "allowed_approval_policies", "allowed_approvals_reviewers", "config_server"]
                .iter()
                .all(|key| doc.get(key).is_none())
                && doc.get("cli_auth_credentials_store").is_none_or(|v| v.as_str() == Some("file"))
        })
    })
}

fn cloud_approval_is_absent(account: &Path) -> bool {
    let read = |name| crate::boundary::read_settings_file(&account.join(name), crate::boundary::SETTINGS_MAX);
    if !matches!(read("cloud-config-bundle-cache.json"), Ok(None)) {
        return false;
    }
    match read("auth.json") {
        Ok(None) => true,
        Ok(Some(bytes)) => serde_json::from_slice::<serde_json::Value>(&bytes).is_ok_and(|auth| {
            if auth["tokens"].is_null() {
                return auth["auth_mode"].as_str().is_none_or(|mode| mode == "apikey");
            }
            // Codex's 0.171 loader fetches cloud requirements for managed
            // plans. An unrecognized or unreadable plan stays unknown.
            auth["tokens"]["id_token"].as_str().and_then(super::limits::jwt_claims).is_some_and(|claims| {
                matches!(
                    claims["https://api.openai.com/auth"]["chatgpt_plan_type"].as_str(),
                    Some("free" | "go" | "plus" | "pro" | "prolite")
                )
            })
        }),
        Err(_) => false,
    }
}

#[cfg(target_os = "macos")]
fn system_requirements() -> Option<PathBuf> {
    // Codex reads forced config and requirements preferences through
    // CoreFoundation. Luna cannot establish their absence on this platform.
    None
}

#[cfg(all(unix, not(target_os = "macos")))]
fn system_requirements() -> Option<PathBuf> {
    Some(PathBuf::from("/etc/codex/requirements.toml"))
}

#[cfg(windows)]
fn system_requirements() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows::core::w;
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY};

    let mut path = [0u16; 260];
    let mut bytes = (path.len() * 2) as u32;
    // SAFETY: The registry writes at most the stated buffer size.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"SOFTWARE\Policies\OpenAI\Codex"),
            w!("RequirementsPath"),
            RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
            None,
            Some(path.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    if status == ERROR_SUCCESS {
        let end = path.iter().position(|&c| c == 0)?;
        let path = PathBuf::from(std::ffi::OsString::from_wide(&path[..end]));
        return path.is_absolute().then_some(path);
    }
    if status != ERROR_FILE_NOT_FOUND && status != ERROR_PATH_NOT_FOUND {
        return None;
    }
    known_program_data().map(|data| data.join("OpenAI").join("Codex").join("requirements.toml"))
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
                Err(e) => out.unsure.push(format!("{}: its developer_instructions not read", not_toml(&file, &text, &e))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => out.unsure.push(format!("{}: {e}: its developer_instructions not read", file.display())),
        }
    }
    for file in managed_layers() {
        match std::fs::read_to_string(&file) {
            Ok(text) => match text.parse::<DocumentMut>() {
                Ok(doc) if doc.get("developer_instructions").is_some() => out.unsure.push(format!(
                    "{} sets developer_instructions, above the command line's: what Luna gives may not reach the session",
                    file.display()
                )),
                Ok(_) => {}
                Err(e) => out.unsure.push(format!("{}: it may set developer_instructions over Luna's", not_toml(&file, &text, &e))),
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
    fn managed_approval_or_unreadable_requirements_keep_spawn_prompted() {
        let root = std::env::temp_dir().join(format!("luna-codex-approval-layers-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let requirements = root.join("requirements.toml");
        assert!(approval_layers_clear([requirements.clone()]));
        for (body, clear) in [
            ("allowed_sandbox_modes = [\"read-only\"]", true),
            ("allowed_approval_policies = [\"on-request\"]", false),
            ("allowed_approvals_reviewers = [\"auto_review\"]", false),
            ("approval_policy = \"never\"", false),
            ("approvals_reviewer = \"auto_review\"", false),
            ("[config_server]\nurl = \"https://example.invalid\"", false),
            ("[broken", false),
        ] {
            std::fs::write(&requirements, body).unwrap();
            assert_eq!(approval_layers_clear([requirements.clone()]), clear, "{body}");
        }
        assert!(cloud_approval_is_absent(&root));
        std::fs::write(root.join("cloud-config-bundle-cache.json"), "{}").unwrap();
        assert!(!cloud_approval_is_absent(&root));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A repository's root as Codex 0.169.0 knows one: a `.git` folder with
    /// a `HEAD` in it.
    fn repo_at(dir: &Path) {
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
    }

    /// The machine's managed layer as `file` for this test's thread, or as it
    /// is (`MANAGED_FOR_TEST`).
    fn managed_at(file: Option<PathBuf>) {
        MANAGED_FOR_TEST.with(|m| *m.borrow_mut() = file);
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
        // The account's own managed_config.toml is no layer of Codex's, on
        // Windows or elsewhere.
        std::fs::write(root.join("managed_config.toml"), "[mcp_servers.it]\nurl = \"u\"\n").unwrap();
        assert_eq!(helper_mcp(&account, &f), Ok(HelperMcp::default()), "CODEX_HOME's managed_config.toml");
        // The machine's managed layer comes over the command line: no -c turns
        // its servers off.
        let managed = root.join("etc-managed_config.toml");
        managed_at(Some(managed.clone()));
        std::fs::write(&managed, "[mcp_servers.it]\nurl = \"u\"\n").unwrap();
        let e = helper_mcp(&account, &f).unwrap_err();
        assert!(e.contains("not started") && e.contains("etc-managed_config.toml") && e.contains("connectors: true"), "{e}");
        std::fs::write(&managed, "[plugins.\"crm@m\"]\nenabled = true\n").unwrap();
        assert!(helper_mcp(&account, &f).is_err(), "nor its plugins");
        std::fs::write(&managed, "[features]\napps = true\n").unwrap();
        assert!(helper_mcp(&account, &f).is_err(), "nor its apps");
        std::fs::write(&managed, "model = \"m\"\n").unwrap();
        assert_eq!(helper_mcp(&account, &f), Ok(HelperMcp::default()), "a managed layer setting none of them");
        managed_at(None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A project layer saved as untrusted is one Codex leaves out of a
    /// helper's run though the helper's folder is trusted for it: none of its
    /// servers is named to turn off (a server no loaded layer defines would
    /// fail the run), and one that will not parse stops no start.
    #[test]
    fn a_helper_leaves_a_project_layer_codex_skips_alone() {
        let root = std::env::temp_dir().join(format!("luna-codex-helper-untrusted-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (account, repo) = (root.join("acc"), root.join("repo"));
        let sub = repo.join("crates").join("app");
        for d in [&account, &repo.join(".codex"), &sub.join(".codex")] {
            std::fs::create_dir_all(d).unwrap();
        }
        repo_at(&repo);
        let (a, s) = (account.to_string_lossy().into_owned(), sub.to_string_lossy().into_owned());
        std::fs::write(repo.join(".codex").join("config.toml"), "[mcp_servers.docs]\ncommand = \"x\"\n").unwrap();
        std::fs::write(sub.join(".codex").join("config.toml"), "[mcp_servers.app]\ncommand = \"x\"\n").unwrap();
        let names = || helper_mcp(&a, &s).map(|m| m.servers.into_iter().map(|(n, _)| n).collect::<Vec<_>>());
        assert_eq!(names().unwrap(), ["docs", "app"], "both trusted for the run");
        std::fs::write(account.join("config.toml"), format!("[projects.'{s}']\ntrust_level = \"untrusted\"\n")).unwrap();
        assert_eq!(names().unwrap(), ["docs"], "the folder's own layer, saved as untrusted, left out");
        std::fs::write(sub.join(".codex").join("config.toml"), "[mcp_servers.app\n").unwrap();
        assert_eq!(names().unwrap(), ["docs"], "and its parse error with it");
        std::fs::write(repo.join(".codex").join("config.toml"), "[mcp_servers.docs\n").unwrap();
        assert!(names().unwrap_err().contains("not started"), "a layer Codex loads still fails closed");
        // Trusted by the managed config over the account's word, the folder's
        // layer is loaded, and its server named to turn off.
        std::fs::write(repo.join(".codex").join("config.toml"), "[mcp_servers.docs]\ncommand = \"x\"\n").unwrap();
        std::fs::write(sub.join(".codex").join("config.toml"), "[mcp_servers.app]\ncommand = \"x\"\n").unwrap();
        let managed = root.join("etc-managed_config.toml");
        managed_at(Some(managed.clone()));
        std::fs::write(&managed, format!("[projects.'{s}']\ntrust_level = \"trusted\"\n")).unwrap();
        assert_eq!(names().unwrap(), ["docs", "app"], "the managed config's trust over the account's");
        managed_at(None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The layers that define a server named `luna`, each with its keys:
    /// what Luna's own would merge with. A project layer counts as Codex
    /// would load it: trusted, or undecided, its parse error then no error;
    /// one saved as untrusted not at all. The account's `managed_config.toml`
    /// is no layer of 0.171.0's.
    #[test]
    fn the_layers_naming_a_server_are_found_or_the_reading_fails() {
        let root = std::env::temp_dir().join(format!("luna-codex-named-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (account, repo) = (root.join("acc"), root.join("repo"));
        for d in [&account, &repo.join(".codex")] {
            std::fs::create_dir_all(d).unwrap();
        }
        repo_at(&repo);
        let (a, r) = (account.to_string_lossy().into_owned(), repo.to_string_lossy().into_owned());
        let project = repo.join(".codex").join("config.toml");
        assert_eq!(servers_named(&a, &r, "luna", false), Ok(Vec::new()), "no config anywhere");
        let theirs = "[mcp_servers.luna]\ncommand = \"node\"\n[mcp_servers.gmail]\nurl = \"u\"\n";
        std::fs::write(account.join("config.toml"), theirs).unwrap();
        std::fs::write(&project, "mcp_servers.luna.enabled = false\n").unwrap();
        std::fs::write(account.join("managed_config.toml"), "[mcp_servers.luna]\nurl = \"m\"\n").unwrap();
        let named =
            |file: PathBuf, keys: &[&str], managed: bool| NamedServer { file, keys: keys.iter().map(|k| k.to_string()).collect(), managed };
        let both = [named(account.join("config.toml"), &["command"], false), named(project.clone(), &["enabled"], false)];
        for helper in [false, true] {
            assert_eq!(servers_named(&a, &r, "luna", helper).unwrap(), both, "undecided for a chat, trusted for a helper");
        }
        assert_eq!(servers_named(&a, &r, "gmail", false).unwrap().len(), 1);
        // One that will not parse: an error where Codex loads it, not where
        // it may never.
        std::fs::write(&project, "[mcp_servers.luna\n").unwrap();
        assert_eq!(servers_named(&a, &r, "luna", false).unwrap(), both[..1], "undecided");
        assert!(servers_named(&a, &r, "luna", true).is_err(), "trusted for the helper's run");
        // Saved as untrusted, Codex leaves it out of a chat, a `luna` or a
        // parse error in it; a helper's run trusts its folder over that.
        let untrusted = format!("{theirs}[projects.'{r}']\ntrust_level = \"untrusted\"\n");
        std::fs::write(account.join("config.toml"), &untrusted).unwrap();
        assert_eq!(servers_named(&a, &r, "luna", false).unwrap(), both[..1], "a parse error left out");
        assert!(servers_named(&a, &r, "luna", true).is_err(), "the run's own -c projects over the saved decision");
        std::fs::write(&project, "mcp_servers.luna.enabled = false\n").unwrap();
        assert_eq!(servers_named(&a, &r, "luna", false).unwrap(), both[..1], "a `luna` left out");
        // The managed layer's trust is above the account's: trusted there,
        // Codex loads the layer, and its `luna` would turn Luna's tools off.
        let managed = root.join("etc-managed_config.toml");
        managed_at(Some(managed.clone()));
        std::fs::write(&managed, format!("[projects.'{r}']\ntrust_level = \"trusted\"\n")).unwrap();
        assert_eq!(servers_named(&a, &r, "luna", false).unwrap(), both, "trusted by the managed config");
        // And above a helper's own `-c projects`: untrusted there, left out.
        std::fs::write(account.join("config.toml"), theirs).unwrap();
        std::fs::write(&managed, format!("[projects.'{r}']\ntrust_level = \"untrusted\"\n")).unwrap();
        assert_eq!(servers_named(&a, &r, "luna", true).unwrap(), both[..1], "untrusted by the managed config");
        managed_at(None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The system layer is in the folder the shell knows as ProgramData, a
    /// folder that is there, whatever the inherited variable says.
    #[cfg(windows)]
    #[test]
    fn the_system_layer_is_in_the_program_data_known_folder() {
        let data = program_data();
        assert!(data.is_absolute() && data.is_dir(), "{}", data.display());
        assert_eq!(system_layer(), data.join("OpenAI").join("Codex").join("config.toml"));
    }

    /// A parse error says which file and where, never what is in it: the
    /// file may be a link to one that is no config.
    #[test]
    fn a_parse_error_names_the_place_and_none_of_the_text() {
        let root = std::env::temp_dir().join(format!("luna-codex-parse-error-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("config.toml");
        std::fs::write(&file, "model = \"m\"\nsecret-token = ghp_s3cr3t value\n").unwrap();
        let e = read(&file).unwrap_err();
        assert_eq!(e, format!("{} does not parse as TOML (line 2, column 16)", file.display()));
        assert!(!e.contains("ghp_") && !e.contains("secret-token"), "{e}");
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
             [plugins.\"off@market\"]\nenabled = false\n[plugins.\"repo@rm-luna\"]\nenabled = true\n",
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
        // Replaced by a higher layer's, they refuse nothing: Codex checks the
        // merged value.
        let managed = root.join("etc-managed_config.toml");
        std::fs::write(&managed, "project_root_markers = [\"Cargo.lock\"]\n").unwrap();
        managed_at(Some(managed));
        assert_eq!(names(helper_mcp(&a, &s).unwrap()), ["docs", "gmail", "team.db"], "the managed layer's markers");
        managed_at(None);
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
        // Where, and nothing of what is there.
        let at_place = |said: &str, file: PathBuf, then: &str| {
            let place = format!("{} does not parse as TOML (line 1, column ", file.display());
            said.starts_with(&place) && said.ends_with(then) && !said.contains("[broken")
        };
        let v = value(true);
        let project = sub.join(".codex").join("config.toml");
        assert!(v.unsure.len() == 1 && at_place(&v.unsure[0], project, "): its developer_instructions not read"), "{:?}", v.unsure);
        let managed_file = account.join("etc-managed_config.toml");
        managed_at(Some(managed_file.clone()));
        std::fs::write(&managed_file, "developer_instructions = [broken\n").unwrap();
        let managed = |u: &String| at_place(u, managed_file.clone(), "): it may set developer_instructions over Luna's");
        assert!(value(false).unsure.iter().any(managed), "{:?}", value(false).unsure);
        std::fs::write(&managed_file, "developer_instructions = \"IT's.\"\n").unwrap();
        assert!(value(false).unsure.iter().any(|u| u.contains("above the command line's")));
        managed_at(None);
        std::fs::write(account.join("managed_config.toml"), "developer_instructions = \"IT's.\"\n").unwrap();
        assert!(value(false).unsure.is_empty(), "CODEX_HOME's managed_config.toml is no layer");
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
