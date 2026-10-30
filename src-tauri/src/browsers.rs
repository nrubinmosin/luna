//! Which browser a link from a terminal opens in. A browser here is a name
//! and a command line with `%1` where the link goes, so a profile, a private
//! window or another program entirely is one flag away: Brave's accounts are
//! `--profile-directory="Profile 1"`. The ones on this machine are found
//! (Chromium browsers and Firefox, each profile its own entry), the user adds
//! their own, and the one used is the most particular there is: the chat's,
//! the account's, the folder's, then the default — the system's handler,
//! flags and all, unless set otherwise.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What the system's handler is called in a choice: it is the one link
/// opening would take without Luna.
pub const SYSTEM: &str = "system";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Browser {
    pub id: String,
    pub name: String,
    /// The program and its arguments; `%1` is the link, put after the rest
    /// when the command has none.
    pub command: String,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct BrowserSettings {
    /// The user's own.
    pub custom: Vec<Browser>,
    /// The browser for every link nothing more particular claims; None is the
    /// system's handler.
    pub default: Option<String>,
    /// By account, `provider/name`.
    pub accounts: BTreeMap<String, String>,
    /// By project folder.
    pub folders: BTreeMap<String, String>,
}

/// What the settings show: the system's handler as found, the browsers on
/// this machine, and the choices made.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Browsers {
    /// The handler's command line, when the system says what it is.
    pub system: Option<String>,
    pub found: Vec<Browser>,
    pub settings: BrowserSettings,
}

#[tauri::command]
pub fn browsers() -> Browsers {
    Browsers { system: imp::system_command(), found: imp::found(), settings: crate::settings::get().browsers }
}

#[tauri::command]
pub fn set_browsers(settings: BrowserSettings) -> Result<Browsers, String> {
    for b in &settings.custom {
        if b.id == SYSTEM || b.id.trim().is_empty() {
            return Err("a browser needs an id of its own".into());
        }
        if split(&b.command).is_empty() {
            return Err(format!("\"{}\" has no command", b.name));
        }
    }
    crate::settings::update(|s| s.browsers = settings)?;
    Ok(browsers())
}

/// Where a link was clicked: the chat's own choice, its account and its
/// project folder.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Context {
    /// A browser picked for this link or remembered for the chat.
    pub browser: Option<String>,
    pub account: Option<String>,
    pub folder: Option<String>,
}

/// The browser's id for a link clicked in `ctx`, most particular first.
fn choose(s: &BrowserSettings, ctx: &Context) -> String {
    let folder = ctx
        .folder
        .as_deref()
        .and_then(|f| s.folders.iter().find(|(k, _)| crate::pty::same_dir(k, f)).map(|(_, v)| v.clone()));
    ctx.browser
        .clone()
        .or_else(|| ctx.account.as_ref().and_then(|a| s.accounts.get(a).cloned()))
        .or(folder)
        .or_else(|| s.default.clone())
        .unwrap_or_else(|| SYSTEM.to_string())
}

/// Opens `url` in the browser `ctx` comes to. One that is no longer there
/// (a custom one deleted, a profile gone) falls back to the system's.
pub fn open(url: &str, ctx: &Context) -> Result<(), String> {
    let s = crate::settings::get().browsers;
    let id = choose(&s, ctx);
    let browser = (id != SYSTEM)
        .then(|| s.custom.iter().find(|b| b.id == id).cloned().or_else(|| imp::found().into_iter().find(|b| b.id == id)))
        .flatten();
    match browser {
        Some(b) => launch(&b.command, url),
        None => tauri_plugin_opener::open_url(url, None::<&str>).map_err(|e| e.to_string()),
    }
}

/// Runs a browser's command with the link in it, as one argument of its own
/// and with no shell between: a link cannot add flags or commands to it.
fn launch(command: &str, url: &str) -> Result<(), String> {
    let mut args = split(command);
    if args.is_empty() {
        return Err("the browser has no command".into());
    }
    if args.iter().any(|a| a.contains("%1")) {
        for a in args.iter_mut() {
            *a = a.replace("%1", url);
        }
    } else {
        args.push(url.to_string());
    }
    let mut cmd = std::process::Command::new(&args[0]);
    cmd.args(&args[1..]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0000_0008 | 0x0000_0200); // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    }
    // Not waited for: the browser outlives the click, and Luna.
    cmd.spawn().map(|_| ()).map_err(|e| format!("could not start {}: {e}", args[0]))
}

/// A command line as its words: whitespace apart, double quotes keeping a
/// word together (the way Windows writes `"C:\Program Files\…" --flag="a b"`).
pub fn split(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let (mut quoted, mut any) = (false, false);
    for c in command.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut word));
                    any = false;
                }
            }
            c => {
                word.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(word);
    }
    out
}

/// A Chromium browser's profiles, `(directory, name)`, off its `Local State`.
fn chromium_profiles(local_state: &str) -> Vec<(String, String)> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(local_state) else { return Vec::new() };
    let Some(cache) = v["profile"]["info_cache"].as_object() else { return Vec::new() };
    let mut out: Vec<(String, String)> =
        cache.iter().map(|(dir, p)| (dir.clone(), p["name"].as_str().unwrap_or(dir).to_string())).collect();
    // Default first, then Profile 1, 2, … in order.
    out.sort_by_key(|(dir, _)| (dir != "Default", dir.len(), dir.clone()));
    out
}

/// Firefox's profiles by name, off its `profiles.ini`.
fn firefox_profiles(ini: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_profile = false;
    for line in ini.lines().map(str::trim) {
        if line.starts_with('[') {
            in_profile = line.starts_with("[Profile");
        } else if let Some(name) = line.strip_prefix("Name=").filter(|_| in_profile) {
            out.push(name.to_string());
        }
    }
    out
}

/// Where each Chromium browser keeps its profiles, by its exe, under
/// `%LOCALAPPDATA%`.
const CHROMIUM: &[(&str, &str)] = &[
    ("chrome.exe", "Google\\Chrome\\User Data"),
    ("msedge.exe", "Microsoft\\Edge\\User Data"),
    ("brave.exe", "BraveSoftware\\Brave-Browser\\User Data"),
    ("vivaldi.exe", "Vivaldi\\User Data"),
    ("browser.exe", "Yandex\\YandexBrowser\\User Data"),
];

/// The entries for one browser installed at `exe`, called `label`: one per
/// profile where it has them.
fn entries_for(exe: &str, label: &str, read: &dyn Fn(&std::path::Path) -> Option<String>) -> Vec<Browser> {
    let file = std::path::Path::new(exe).file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let stem = file.trim_end_matches(".exe").to_string();
    if let Some((_, data)) = CHROMIUM.iter().find(|(f, _)| *f == file) {
        let local = dirs::data_local_dir().map(|d| d.join(data).join("Local State"));
        let profiles = local.as_deref().and_then(read).map(|s| chromium_profiles(&s)).unwrap_or_default();
        if !profiles.is_empty() {
            return profiles
                .into_iter()
                .map(|(dir, name)| Browser {
                    id: format!("found:{stem}:{dir}"),
                    name: format!("{label} — {name}"),
                    command: format!("\"{exe}\" --profile-directory=\"{dir}\" --single-argument %1"),
                })
                .collect();
        }
    }
    if file == "firefox.exe" {
        let ini = dirs::config_dir().map(|d| d.join("Mozilla").join("Firefox").join("profiles.ini"));
        let profiles = ini.as_deref().and_then(read).map(|s| firefox_profiles(&s)).unwrap_or_default();
        if !profiles.is_empty() {
            return profiles
                .into_iter()
                .map(|name| Browser {
                    id: format!("found:{stem}:{name}"),
                    name: format!("{label} — {name}"),
                    command: format!("\"{exe}\" -P \"{name}\" -new-tab %1"),
                })
                .collect();
        }
    }
    vec![Browser { id: format!("found:{stem}"), name: label.to_string(), command: format!("\"{exe}\" %1") }]
}

#[cfg(windows)]
mod imp {
    use super::Browser;
    use windows::core::{HSTRING, PCWSTR, PWSTR};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ,
        RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
    };

    /// A string value, `None` for the key's default one.
    fn string(root: HKEY, path: &str, value: Option<&str>) -> Option<String> {
        let path = HSTRING::from(path);
        let value = value.map(HSTRING::from);
        let name = value.as_ref().map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr()));
        let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ;
        let mut len = 0u32;
        // SAFETY: sized first, then read into a buffer of that size that
        // outlives the call.
        unsafe {
            if RegGetValueW(root, &path, name, flags, None, None, Some(&mut len)).is_err() || len < 2 {
                return None;
            }
            let mut buf = vec![0u16; len as usize / 2];
            if RegGetValueW(root, &path, name, flags, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut len)).is_err() {
                return None;
            }
            let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Some(String::from_utf16_lossy(&buf[..end]))
        }
    }

    fn subkeys(root: HKEY, path: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut key = HKEY::default();
        // SAFETY: a key opened and closed here; each name read into a buffer
        // that outlives the call.
        unsafe {
            if RegOpenKeyExW(root, &HSTRING::from(path), None, KEY_READ, &mut key).is_err() {
                return out;
            }
            for i in 0.. {
                let mut buf = [0u16; 256];
                let mut len = buf.len() as u32;
                if RegEnumKeyExW(key, i, Some(PWSTR(buf.as_mut_ptr())), &mut len, None, None, None, None).is_err() {
                    break;
                }
                out.push(String::from_utf16_lossy(&buf[..len as usize]));
            }
            let _ = RegCloseKey(key);
        }
        out
    }

    /// What `https` links open with: the user's choice's command line.
    pub fn system_command() -> Option<String> {
        let prog = string(
            HKEY_CURRENT_USER,
            r"Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https\UserChoice",
            Some("ProgId"),
        )?;
        let path = format!(r"Software\Classes\{prog}\shell\open\command");
        string(HKEY_CURRENT_USER, &path, None).or_else(|| string(HKEY_LOCAL_MACHINE, &path, None))
    }

    /// Every browser Windows knows of, the user's and the machine's, by its
    /// exe; Edge, which an LTSC Windows does not list, where it is installed.
    pub fn found() -> Vec<Browser> {
        const CLIENTS: &str = r"SOFTWARE\Clients\StartMenuInternet";
        let mut seen: Vec<(String, String)> = Vec::new();
        for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
            for name in subkeys(root, CLIENTS) {
                let Some(cmd) = string(root, &format!(r"{CLIENTS}\{name}\shell\open\command"), None) else { continue };
                let Some(exe) = super::split(&cmd).into_iter().next() else { continue };
                if exe.to_lowercase().ends_with("iexplore.exe") || seen.iter().any(|(e, _)| e.eq_ignore_ascii_case(&exe)) {
                    continue;
                }
                let label = string(root, &format!(r"{CLIENTS}\{name}"), None).filter(|l| !l.is_empty()).unwrap_or(name);
                seen.push((exe, label));
            }
        }
        let edge = r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe";
        if std::path::Path::new(edge).exists() && !seen.iter().any(|(e, _)| e.to_lowercase().ends_with("msedge.exe")) {
            seen.push((edge.to_string(), "Microsoft Edge".to_string()));
        }
        let read = |p: &std::path::Path| std::fs::read_to_string(p).ok();
        seen.iter().flat_map(|(exe, label)| super::entries_for(exe, label, &read)).collect()
    }
}

/// Elsewhere only the system's handler and the user's own: the browsers and
/// their profiles are found on Windows alone.
#[cfg(not(windows))]
mod imp {
    pub fn system_command() -> Option<String> {
        None
    }
    pub fn found() -> Vec<super::Browser> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_line_splits_the_way_windows_writes_one() {
        assert_eq!(
            split(r#""C:\Program Files\Brave\brave.exe" --profile-directory="Profile 1" --single-argument %1"#),
            [r"C:\Program Files\Brave\brave.exe", "--profile-directory=Profile 1", "--single-argument", "%1"]
        );
        assert_eq!(split("  firefox  -new-tab "), ["firefox", "-new-tab"]);
        assert!(split("   ").is_empty());
    }

    #[test]
    fn the_most_particular_choice_wins() {
        let s = BrowserSettings {
            custom: Vec::new(),
            default: Some("g".into()),
            accounts: [("claude/work".to_string(), "a".to_string())].into(),
            folders: [(r"C:\p".to_string(), "f".to_string())].into(),
        };
        let ctx = |b: Option<&str>, a: Option<&str>, f: Option<&str>| Context {
            browser: b.map(Into::into),
            account: a.map(Into::into),
            folder: f.map(Into::into),
        };
        assert_eq!(choose(&s, &ctx(Some("c"), Some("claude/work"), Some(r"C:\p"))), "c");
        assert_eq!(choose(&s, &ctx(None, Some("claude/work"), Some(r"C:\p"))), "a");
        assert_eq!(choose(&s, &ctx(None, Some("claude/home"), Some(r"C:\p\"))), "f");
        assert_eq!(choose(&s, &ctx(None, None, Some(r"D:\q"))), "g");
        assert_eq!(choose(&BrowserSettings::default(), &ctx(None, None, None)), SYSTEM);
    }

    #[test]
    fn profiles_are_read_off_the_browsers_own_files() {
        let local = r#"{"profile":{"info_cache":{"Profile 1":{"name":"Work"},"Default":{"name":"Personal"}}}}"#;
        assert_eq!(chromium_profiles(local), [("Default".into(), "Personal".into()), ("Profile 1".into(), "Work".into())]);
        let ini = "[General]\nName=no\n[Profile0]\nName=default-release\n[Profile1]\nName=work\n";
        assert_eq!(firefox_profiles(ini), ["default-release", "work"]);
    }

    #[test]
    fn a_chromium_browser_gets_an_entry_per_profile() {
        let read = |_: &std::path::Path| Some(r#"{"profile":{"info_cache":{"Profile 1":{"name":"Work"}}}}"#.to_string());
        let e = entries_for(r"C:\B\brave.exe", "Brave", &read);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].name, "Brave — Work");
        assert_eq!(split(&e[0].command)[1], "--profile-directory=Profile 1");
        let plain = entries_for(r"C:\O\opera.exe", "Opera", &|_: &std::path::Path| None);
        assert_eq!(plain[0].command, r#""C:\O\opera.exe" %1"#);
    }
}
