//! Which account folders have a CLI process of Luna's on them, from just before
//! the process starts until it has exited, and the short holds that keep new
//! ones off an account while one of its files is rewritten.
//!
//! The session map cannot say this: a chat being deleted leaves the map first
//! and exits after (pty/control.rs `take_and_shut_down`), and a CLI on its way out still
//! writes its files. A CLI rewrites them from what it read, so a change made
//! under it is lost whichever of the two writes last; an atomic rename only
//! keeps the file whole. Hence a hold is taken only with no process on the
//! account, and a process that wants to start waits the hold out. A CLI started
//! outside Luna is in no table here, and nothing keeps it out.
//!
//! All in the core, where every session and helper starts.

use std::collections::{HashMap, HashSet};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};

#[derive(Default)]
struct Table {
    running: HashMap<String, usize>,
    held: HashSet<String>,
}

fn table() -> &'static (Mutex<Table>, Condvar) {
    static T: OnceLock<(Mutex<Table>, Condvar)> = OnceLock::new();
    T.get_or_init(Default::default)
}

fn locked() -> MutexGuard<'static, Table> {
    table().0.lock().unwrap_or_else(|e| e.into_inner())
}

fn key(account_path: &str) -> String {
    crate::paths::path_key_loose(account_path)
}

/// The folder's name, for a message.
fn name(account_path: &str) -> &str {
    account_path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(account_path)
}

/// One process on an account; dropped once it has exited.
#[must_use]
pub struct Running(String);

impl Drop for Running {
    fn drop(&mut self) {
        let mut t = locked();
        if let Some(n) = t.running.get_mut(&self.0) {
            *n -= 1;
            if *n == 0 {
                t.running.remove(&self.0);
            }
        }
        drop(t);
        table().1.notify_all();
    }
}

/// Counts a process about to start on the account, waiting out a hold on it:
/// a hold lasts as long as a few file writes.
pub fn running_on(account_path: &str) -> Running {
    let k = key(account_path);
    let mut t = locked();
    while t.held.contains(&k) {
        t = table().1.wait(t).unwrap_or_else(|e| e.into_inner());
    }
    *t.running.entry(k.clone()).or_default() += 1;
    Running(k)
}

/// Whether a process of Luna's is on the account, one on its way out included.
pub fn running(account_path: &str) -> bool {
    locked().running.contains_key(&key(account_path))
}

/// Accounts no process of Luna's may start on until this is dropped.
#[must_use]
#[derive(Debug)]
pub struct Held(Vec<String>);

impl Drop for Held {
    fn drop(&mut self) {
        let mut t = locked();
        for k in &self.0 {
            t.held.remove(k);
        }
        drop(t);
        table().1.notify_all();
    }
}

/// Holds all the accounts or none: an error naming the one a process is on.
/// Taken all at once, so two holds cannot wait on each other.
pub fn hold(account_paths: &[String]) -> Result<Held, String> {
    let keys: Vec<String> = account_paths.iter().map(|p| key(p)).collect();
    let mut t = locked();
    while keys.iter().any(|k| t.held.contains(k)) {
        t = table().1.wait(t).unwrap_or_else(|e| e.into_inner());
    }
    if let Some(at) = keys.iter().position(|k| t.running.contains_key(k)) {
        return Err(format!("{} is in use: a CLI is still running on it", name(&account_paths[at])));
    }
    t.held.extend(keys.iter().cloned());
    Ok(Held(keys))
}

/// `hold`, and no session of Luna's on any of the accounts either: a helper
/// between turns has no process, but it is on the account.
pub fn quiet(account_paths: &[String]) -> Result<Held, String> {
    let held = hold(account_paths)?;
    match account_paths.iter().find(|p| crate::pty::manager().account_in_use(p)) {
        Some(p) => Err(format!("{} is in use: a session runs on it", name(p))),
        None => Ok(held),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    #[test]
    fn a_process_keeps_its_account_until_it_is_gone() {
        let path = "/accounts/use-one".to_string();
        let first = running_on(&path);
        let second = running_on(&path);
        assert!(running(&path));
        assert!(hold(std::slice::from_ref(&path)).unwrap_err().contains("use-one"));
        drop(first);
        assert!(running(&path), "the other one is still on it");
        assert!(hold(std::slice::from_ref(&path)).is_err());
        drop(second);
        assert!(!running(&path));
        assert!(hold(std::slice::from_ref(&path)).is_ok());
    }

    #[test]
    fn a_process_waits_out_a_hold_and_counts_from_then_on() {
        let path = "/accounts/use-two".to_string();
        let held = hold(std::slice::from_ref(&path)).unwrap();
        let (started, came) = channel();
        let spawner = {
            let path = path.clone();
            std::thread::spawn(move || {
                let on = running_on(&path);
                started.send(()).unwrap();
                on
            })
        };
        assert!(came.recv_timeout(Duration::from_millis(150)).is_err(), "it started under the hold");
        assert!(!running(&path));
        drop(held);
        came.recv_timeout(Duration::from_secs(5)).unwrap();
        let on = spawner.join().unwrap();
        assert!(running(&path));
        assert!(hold(std::slice::from_ref(&path)).is_err(), "and now the next write is refused");
        drop(on);
    }

    #[test]
    fn a_hold_takes_all_its_accounts_or_none() {
        let (free, busy) = ("/accounts/use-free".to_string(), "/accounts/use-busy".to_string());
        let on = running_on(&busy);
        assert!(hold(&[free.clone(), busy.clone()]).unwrap_err().contains("use-busy"));
        // The refused hold kept nothing.
        assert!(hold(std::slice::from_ref(&free)).is_ok());
        drop(on);
    }

    #[test]
    fn spellings_of_one_folder_are_one_account() {
        let on = running_on("/accounts/use-three/");
        assert!(running("/accounts/use-three"));
        drop(on);
    }
}
