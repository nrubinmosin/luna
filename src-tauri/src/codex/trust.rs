//! Project trust Codex reads from config and the trust Luna passes for one run.
//! Helpers, chats trusted in Luna, and sessions under a trusted root use
//! `-c projects=…`. Codex's own saved decisions stay in `config.toml`.

use super::config;
use luna_window::dto::{CodexFolderTrust, ProjectTrust};
use std::path::Path;
use toml_edit::Item;

/// Codex compares project keys case-insensitively on Windows and tries both
/// spellings of the separator; the same leniency here, and only there:
/// elsewhere `/projects/App` and `/projects/app` are two folders.
fn same_project(a: &str, b: &str) -> bool {
    crate::paths::path_key(a) == crate::paths::path_key(b)
}

fn native(folder: &str) -> String {
    #[cfg(windows)]
    {
        Path::new(&folder.replace('/', "\\")).components().as_path().to_string_lossy().into_owned()
    }
    #[cfg(not(windows))]
    {
        Path::new(folder).components().as_path().to_string_lossy().into_owned()
    }
}

/// Whether Codex trusts `folder` for a session of the account: the folder's own
/// saved decision, else its project root's, else its repository's, as Codex
/// decides a folder (`Decisions::layer_trusted`).
pub fn codex_folder_trusted(account_path: String, folder: String) -> Result<CodexFolderTrust, String> {
    config::account_file(&account_path)?;
    let trust = Decisions::of(&account_path, &folder, None);
    let root = config::project_root_and_layers(&account_path, &folder).map(|(r, _)| r.to_string_lossy().into_owned());
    let trusted = trust.layer_trusted(&folder, &root.unwrap_or_else(|_| folder.clone()), &folder);
    Ok(CodexFolderTrust { trusted, project: (!trusted).then(|| project_trust(&folder)).flatten() })
}

/// Codex's saved trust decisions for one run of an account (0.171.0): the
/// `[projects]` of its system and account layers, a later layer's key over an
/// earlier one's, and over them the run's own `-c projects=…` where Luna gives
/// one (a helper's, a trusted root's, or the person's choice for a chat),
/// merged key by key as Codex merges the
/// command line's table before it decides trust (config/src/loader/mod.rs,
/// `load_config_layers_state`, the `cli_overrides_layer` merged into
/// `merged_so_far`); and over all of them the managed layer's
/// (`config::managed_layers`), which Codex merges above the command line
/// before it decides (rust-v0.171.0): what the managed config trusts or not,
/// neither the account nor Luna's `-c` changes.
pub(crate) struct Decisions(Vec<(String, bool)>);

impl Decisions {
    pub(crate) fn of(account_path: &str, folder: &str, trust_key: Option<&str>) -> Decisions {
        let mut saved = Vec::new();
        let files = [Some(config::system_layer()), config::account_file(account_path).ok()];
        for file in files.into_iter().flatten() {
            saved.extend(projects_in(&file));
        }
        if let Some(key) = trust_key.map(str::to_owned).or_else(|| trusted_root_key(folder)) {
            saved.push((key, true));
        }
        for file in config::managed_layers() {
            saved.extend(projects_in(&file));
        }
        Decisions(saved)
    }

    /// The decision saved under `dir`'s own key, or under another spelling of
    /// the same folder (Codex looks a folder up canonical as well as written,
    /// config/src/loader/mod.rs `normalized_project_trust_keys`): trusted or
    /// not; None for none.
    fn at(&self, dir: &str) -> Option<bool> {
        self.0.iter().rev().find(|(key, _)| same_project(key, dir) || crate::checkout::same_place(key, dir)).map(|&(_, trusted)| trusted)
    }

    /// Whether a folder from `root` down to `dir`, both included, is saved as
    /// other than trusted.
    pub(crate) fn untrusted_between(&self, root: &str, dir: &str) -> bool {
        let root = Path::new(root);
        Path::new(dir).ancestors().take_while(|d| d.starts_with(root)).any(|d| self.at(&d.to_string_lossy()) == Some(false))
    }

    /// Whether Codex trusts a project layer in `dir` (a folder from the
    /// project `root` down to the session's `folder`): the first decision saved
    /// for `dir`, for the root, for the repository the folder is in
    /// (`repo_root_of`), that one deciding (config/src/loader/mod.rs
    /// `ProjectTrustContext::decision_for_dir`, used per layer by
    /// `discover_project_layers`).
    pub(crate) fn layer_trusted(&self, dir: &str, root: &str, folder: &str) -> bool {
        self.layer_decision(dir, root, folder) == Some(true)
    }

    /// `layer_trusted`'s deciding decision itself: None where none is saved
    /// for any of them, a folder Codex's trust screen asks about.
    pub(crate) fn layer_decision(&self, dir: &str, root: &str, folder: &str) -> Option<bool> {
        self.at(dir).or_else(|| self.at(root)).or_else(|| repo_root_of(folder).and_then(|repo| self.at(&repo)))
    }
}

/// The trust decisions one layer's `[projects]` saves, each by its key as
/// written: trusted or not. A file that does not read or parse saves none.
fn projects_in(file: &Path) -> Vec<(String, bool)> {
    let doc = config::read_or_empty(file);
    let Some(projects) = doc.get("projects").and_then(Item::as_table_like) else { return Vec::new() };
    projects
        .iter()
        .filter_map(|(key, entry)| {
            let level = entry.as_table_like().and_then(|t| t.get("trust_level")).and_then(Item::as_str)?;
            Some((key.to_string(), level == "trusted"))
        })
        .collect()
}

/// The repository root Codex keys trust by (git-utils
/// `resolve_root_git_project_for_trust`): a linked worktree's main
/// repository, else the checkout `folder` is in.
fn repo_root_of(folder: &str) -> Option<String> {
    let checkout = checkout_of(folder)?;
    if Path::new(&checkout).join(".git").is_dir() {
        Some(checkout)
    } else {
        crate::checkout::registered_repo_of_linked(&checkout)
    }
}

/// The nearest Git checkout, if there is one.
fn checkout_of(folder: &str) -> Option<String> {
    Path::new(folder)
        .ancestors()
        .find(|dir| {
            let marker = dir.join(".git");
            marker.is_file() || marker.join("HEAD").exists()
        })
        .map(|dir| dir.to_string_lossy().into_owned())
}

/// Trust the Git checkout so its ancestor layers load when starting below
/// it. Use the main repo for a registered linked worktree. Outside Git trust
/// the chosen folder, even if Codex found project layers further up.
pub(crate) fn override_key(folder: &str) -> String {
    let Some(checkout) = checkout_of(folder) else { return native(folder) };
    let root = crate::checkout::registered_repo_of_linked(&checkout).unwrap_or(checkout);
    native(&root)
}

pub(crate) fn project_trust(folder: &str) -> Option<ProjectTrust> {
    let key = override_key(folder);
    if !crate::boundary::derived_ok(&key, folder) {
        return None;
    }
    let (canonical_path, file_id) = crate::checkout::directory_stamp(Path::new(&key))?;
    Some(ProjectTrust { key, canonical_path, file_id })
}

/// Re-resolve from the launch folder and compare with the approved place.
/// A changed marker or a replaced directory requires fresh consent.
pub(crate) fn granted_key(grant: &ProjectTrust, folder: &str) -> Option<String> {
    let current = project_trust(folder)?;
    (current.file_id == grant.file_id && current.canonical_path == grant.canonical_path).then_some(current.key)
}

pub(crate) fn trusted_root_key(folder: &str) -> Option<String> {
    (!folder.is_empty() && under_trusted_root(folder)).then(|| override_key(folder))
}

/// `-c` trusting the resolved project key for this one run.
pub fn trust_override(key: &str) -> [String; 2] {
    let key = toml_edit::Key::new(key);
    ["-c".into(), format!("projects={{{key}={{trust_level=\"trusted\"}}}}")]
}

/// Whether `folder` is under a folder trusted on this machine.
pub fn under_trusted_root(folder: &str) -> bool {
    folder_under_root(folder, &crate::settings::get().codex_trusted_roots)
}

fn folder_under_root(folder: &str, roots: &[String]) -> bool {
    if roots.is_empty() {
        return false;
    }
    // A path shaped like a CLI-made checkout can point at a trusted repo.
    // Use that repo only if Codex would recognize the link.
    let project = checkout_of(folder)
        .and_then(|checkout| crate::checkout::registered_repo_of_linked(&checkout))
        .filter(|_| crate::checkout::project_of(folder).is_some());
    under_root(project.as_deref().unwrap_or(folder), roots)
}

/// Use Codex's project key spelling rules and recognize other spellings
/// of checked, existing directories by identity.
fn under_root(folder: &str, roots: &[String]) -> bool {
    let sep = if cfg!(windows) { '\\' } else { '/' };
    let key = crate::paths::path_key(folder);
    roots.iter().map(|root| (root, crate::paths::path_key(root))).filter(|(_, root_key)| !root_key.is_empty()).any(|(root, root_key)| {
        key == root_key
            || key.starts_with(&format!("{root_key}{sep}"))
            || crate::checkout::within_plain_root(Path::new(folder), Path::new(root))
    })
}

pub fn codex_trusted_roots() -> Vec<String> {
    crate::settings::get().codex_trusted_roots
}

pub fn set_codex_trusted_roots(roots: Vec<String>) -> Result<Vec<String>, String> {
    let roots: Vec<String> = roots.into_iter().map(|r| r.trim().to_string()).filter(|r| !r.is_empty()).collect();
    // Each one trusts everything under it: a plain absolute folder only, as
    // the folder picker gives (boundary.rs).
    for r in &roots {
        crate::boundary::absolute(r).map_err(|e| format!("{r}: {e}"))?;
    }
    crate::settings::update(|s| s.codex_trusted_roots = roots.clone())?;
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn a_root_covers_the_folders_under_it_and_nothing_beside_it() {
        let roots = vec!["E:\\Projects\\".to_string()];
        assert!(under_root("E:\\Projects", &roots));
        assert!(under_root("E:/Projects/Sprouter/app", &roots), "either separator");
        assert!(under_root("e:\\projects\\app", &roots), "any case");
        assert!(!under_root("E:\\Projects2\\app", &roots), "a sibling that shares the prefix");
        assert!(!under_root("E:\\Projects\\app", &[String::new()]));
    }

    #[cfg(not(windows))]
    #[test]
    fn a_root_covers_the_folders_under_it_and_nothing_beside_it() {
        let roots = vec!["/home/u/projects/".to_string()];
        assert!(under_root("/home/u/projects", &roots));
        assert!(under_root("/home/u/projects/app", &roots));
        assert!(!under_root("/home/u/Projects/app", &roots), "another folder");
        assert!(!under_root("/home/u/projects\\app", &roots), "a backslash is part of a name");
        assert!(!under_root("/home/u/projects2/app", &roots), "a sibling that shares the prefix");
        assert!(!under_root("/home/u/projects/app", &[String::new()]));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn a_directory_replaced_by_a_link_during_the_trust_check_is_refused() {
        use crate::checkout::{before_comparison_open, link_dir};

        for (name, swap_root, use_child) in [("folder", false, false), ("ancestor", false, true), ("root", true, false)] {
            let base = temp(name);
            let trusted = base.join("trusted");
            let outside = base.join("outside");
            std::fs::create_dir_all(trusted.join("child")).unwrap();
            std::fs::create_dir_all(outside.join("child")).unwrap();
            let candidate = if use_child { outside.join("child") } else { outside.clone() };
            let roots = [trusted.to_str().unwrap().to_owned()];
            let (entry, target) = if swap_root { (trusted, outside) } else { (outside, trusted) };
            let swapped = entry.clone();
            before_comparison_open(entry, move || {
                std::fs::rename(&swapped, swapped.with_extension("before")).unwrap();
                link_dir(&swapped, &target);
            });

            assert!(!under_root(candidate.to_str().unwrap(), &roots), "{name}");
            let link = base.join(if swap_root { "trusted" } else { "outside" });
            assert!(crate::boundary::is_link(&std::fs::symlink_metadata(&link).unwrap()), "{name}: the swap ran");
            #[cfg(windows)]
            std::fs::remove_dir(link).unwrap();
            #[cfg(unix)]
            std::fs::remove_file(link).unwrap();
            std::fs::remove_dir_all(base).unwrap();
        }
    }

    #[test]
    fn the_override_names_the_checkout_as_toml() {
        config::use_empty_machine_layers_for_test();
        let root = temp("ov");
        let repo = root.join("repo");
        let sub = repo.join("sub");
        let acc = root.join("account");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(&acc).unwrap();
        let config = acc.join("config.toml");
        let repo = repo.to_string_lossy();
        let sub = sub.to_string_lossy();
        let acc = acc.to_string_lossy();
        let old = format!("[projects.{}]\ntrust_level = \"trusted\"\n", toml_edit::Key::new(sub.as_ref()));
        std::fs::write(&config, &old).unwrap();
        let key = override_key(&sub);
        let [flag, value] = trust_override(&key);
        assert_eq!(flag, "-c");
        let parsed: toml_edit::DocumentMut = value.parse().unwrap();
        let (key, entry) = parsed["projects"].as_inline_table().unwrap().iter().next().unwrap();
        assert!(same_project(key, &repo), "{key}");
        assert_eq!(entry.as_inline_table().unwrap().get("trust_level").and_then(|v| v.as_str()), Some("trusted"));
        assert!(!Decisions::of(&acc, &sub, None).layer_trusted(&repo, &repo, &sub));
        assert!(Decisions::of(&acc, &sub, Some(&override_key(&sub))).layer_trusted(&repo, &repo, &sub));
        assert_eq!(std::fs::read_to_string(config).unwrap(), old);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_linked_worktree_uses_the_main_repos_key_only_with_its_registration() {
        config::use_empty_machine_layers_for_test();
        let root = temp("linked");
        let repo = root.join("repo");
        let gitdir = repo.join(".git/worktrees/linked");
        let worktree = root.join("elsewhere/worktrees/1234/linked");
        let sub = worktree.join("sub");
        let acc = root.join("account");
        for dir in [&gitdir, &sub, &acc] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(worktree.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
        std::fs::write(gitdir.join("commondir"), "../..\n").unwrap();
        let (repo, worktree, sub, acc) = (repo.to_string_lossy(), worktree.to_string_lossy(), sub.to_string_lossy(), acc.to_string_lossy());
        assert_eq!(override_key(&sub), native(&worktree), "a one-way pointer cannot change the key");
        let grant = project_trust(&repo).unwrap();
        assert_eq!(granted_key(&grant, &sub), None);
        let trusted_roots = [repo.to_string()];
        assert!(!folder_under_root(&worktree, &trusted_roots), "a one-way pointer cannot borrow the Settings grant");
        std::fs::write(gitdir.join("gitdir"), format!("{worktree}/.git\n")).unwrap();
        assert!(folder_under_root(&worktree, &trusted_roots));
        let granted = granted_key(&grant, &sub).expect("the worktree uses the approved repository");
        assert!(crate::checkout::same_place(&granted, &repo));
        assert!(crate::checkout::same_place(&override_key(&sub), &override_key(&repo)));
        #[cfg(windows)]
        if let Some(short_root) = crate::checkout::short_alias(&root) {
            let short_repo = short_root.join("repo");
            assert!(folder_under_root(&worktree, &[short_repo.to_string_lossy().into_owned()]));
        }
        assert!(Decisions::of(&acc, &sub, Some(&override_key(&sub))).layer_trusted(&worktree, &worktree, &sub));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_marker_outside_git_does_not_grant_ancestor_layers() {
        config::use_empty_machine_layers_for_test();
        let root = temp("marker");
        let repo = root.join("repo");
        let sub = repo.join("sub");
        let acc = root.join("account");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir(sub.join(".git")).unwrap();
        std::fs::create_dir_all(&acc).unwrap();
        std::fs::write(repo.join(".project"), "").unwrap();
        std::fs::write(acc.join("config.toml"), "project_root_markers = [\".project\"]\n").unwrap();
        let (acc, repo, sub) = (acc.to_string_lossy(), repo.to_string_lossy(), sub.to_string_lossy());
        let scope = codex_folder_trusted(acc.to_string(), sub.to_string()).unwrap().project.unwrap();
        assert_eq!(scope.key, native(&sub));
        let decisions = Decisions::of(&acc, &sub, Some(&scope.key));
        assert!(decisions.layer_trusted(&sub, &repo, &sub));
        assert!(!decisions.layer_trusted(&repo, &repo, &sub));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Outside git Codex keys a project's trust by the folder it runs in (no
    /// repository root to go up to), so the override names that folder, a
    /// folder under a trusted one is not the trusted one's, and nothing needs
    /// a repository to be read.
    #[test]
    fn outside_git_the_folder_is_its_own_checkout() {
        let root = temp("nogit");
        let (acc, deep) = (root.join("account"), root.join("work").join("deep"));
        std::fs::create_dir_all(&acc).unwrap();
        std::fs::create_dir_all(&deep).unwrap();
        if crate::pty::folder::in_git_repository(&deep) {
            eprintln!("skipped: the temp folder is inside a git repository");
            return;
        }
        let (a, d) = (acc.to_string_lossy().into_owned(), deep.to_string_lossy().into_owned());
        assert_eq!(checkout_of(&d), None);
        let [flag, value] = trust_override(&override_key(&d));
        let parsed: toml_edit::DocumentMut = value.parse().unwrap();
        let (key, _) = parsed["projects"].as_inline_table().unwrap().iter().next().unwrap();
        assert_eq!(flag, "-c");
        assert!(same_project(key, &d), "{key}");

        let key = toml_edit::Key::new(root.join("work").to_string_lossy().as_ref());
        std::fs::write(acc.join("config.toml"), format!("[projects.{key}]\ntrust_level = \"trusted\"\n")).unwrap();
        assert!(!codex_folder_trusted(a, d).unwrap().trusted, "a parent outside Git does not confer trust");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn projects_toml_round_trips_punctuation_and_separators_in_the_key() {
        let path = if cfg!(windows) { r"C:\work\it's a #1.repo" } else { "/work/it's a #1.repo\\name\nnext line" };
        let [flag, value] = trust_override(path);
        assert_eq!(flag, "-c");
        let parsed: toml_edit::DocumentMut = value.parse().unwrap();
        let projects = parsed["projects"].as_inline_table().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects.get(path).unwrap().as_inline_table().unwrap().get("trust_level").and_then(|v| v.as_str()), Some("trusted"));
    }

    #[test]
    fn an_approved_project_cannot_widen_or_be_replaced() {
        let root = temp("bound");
        let project = root.join("repo");
        let chosen = project.join("sub");
        for dir in [&root, &project] {
            std::fs::create_dir_all(dir.join(".git")).unwrap();
            std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        }
        std::fs::create_dir_all(&chosen).unwrap();
        let folder = chosen.to_str().unwrap();
        let grant = project_trust(folder).unwrap();
        assert_eq!(granted_key(&grant, folder).as_deref(), Some(grant.key.as_str()));
        std::fs::remove_file(project.join(".git/HEAD")).unwrap();
        assert_eq!(granted_key(&grant, folder), None, "the parent now supplies the project root");

        std::fs::rename(&project, root.join("old")).unwrap();
        std::fs::create_dir_all(project.join(".git")).unwrap();
        std::fs::write(project.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::create_dir_all(&chosen).unwrap();
        if grant.file_id.is_some() {
            assert_eq!(granted_key(&grant, folder), None, "another directory now occupies the same path");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("luna-codex-trust-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }
}
