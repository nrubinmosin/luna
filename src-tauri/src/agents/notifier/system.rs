//! System news waiting for the person's chat.

use crate::agents::notifier::lines::breaker_lifted;
use crate::agents::registry::{named, reg, top_chat, Known, Registry};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::agents) struct SystemNews {
    kind: SystemNewsKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum SystemNewsKind {
    Skills { helper: String, cause: String, told: bool },
    Breaker { caller: String, helper: String, at: u64 },
}

impl SystemNews {
    pub(in crate::agents) fn about(&self, id: &str) -> bool {
        self.sessions().any(|session| session == id)
    }

    pub(super) fn sessions(&self) -> impl Iterator<Item = &str> {
        let other = match &self.kind {
            SystemNewsKind::Skills { .. } => None,
            SystemNewsKind::Breaker { helper, .. } => Some(helper.as_str()),
        };
        [Some(self.subject()), other].into_iter().flatten()
    }

    pub(in crate::agents) fn subject(&self) -> &str {
        match &self.kind {
            SystemNewsKind::Skills { helper, .. } => helper,
            SystemNewsKind::Breaker { caller, .. } => caller,
        }
    }

    pub(in crate::agents) fn survives_restart(&self) -> bool {
        matches!(self.kind, SystemNewsKind::Skills { .. })
    }

    /// Older cores saved this as plain text for the immediate parent.
    pub(in crate::agents) fn old_skills(line: &str) -> Option<Self> {
        let (helper, cause) =
            line.strip_prefix("Your helper ")?.split_once(" started a turn without the folder skills the library gives its folder: ")?;
        if helper.is_empty() || helper.contains(char::is_whitespace) {
            return None;
        }
        Some(Self::skills(helper, cause))
    }

    fn skills(helper: &str, cause: &str) -> Self {
        Self { kind: SystemNewsKind::Skills { helper: helper.to_string(), cause: cause.to_string(), told: false } }
    }

    fn recipient(&self, r: &Registry) -> Option<String> {
        let subject = self.subject();
        top_chat(r, subject).filter(|chat| chat != subject)
    }

    fn line(&self, r: &Registry) -> Option<String> {
        match &self.kind {
            SystemNewsKind::Skills { helper, cause, told: false } => {
                Some(format!("Delegate {} [folder skills unavailable]: {cause}", named(r, helper)))
            }
            SystemNewsKind::Skills { told: true, .. } => None,
            SystemNewsKind::Breaker { caller, helper, .. } => Some(format!(
                "Notices to {} about {} [turn notices stopped; type in either session to resume].",
                named(r, caller),
                named(r, helper),
            )),
        }
    }

    fn lifted(&self, pty: &crate::pty::PtyManager) -> bool {
        match &self.kind {
            SystemNewsKind::Breaker { caller, helper, at } => breaker_lifted(pty, caller, helper, *at),
            SystemNewsKind::Skills { .. } => false,
        }
    }

    fn active(&self, known: &HashMap<String, Known>, stopped: &HashMap<(String, String), u64>) -> bool {
        self.sessions().all(|id| known.contains_key(id))
            && match &self.kind {
                SystemNewsKind::Breaker { caller, helper, at } => stopped.get(&(caller.clone(), helper.clone())) == Some(at),
                SystemNewsKind::Skills { .. } => true,
            }
    }
}

/// Ignore kinds a newer core may have written and keep the rest of the file.
pub(in crate::agents) fn read_saved<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<SystemNews>, D::Error> {
    let entries = Vec::<serde_json::Value>::deserialize(d)?;
    Ok(entries.into_iter().filter_map(|value| serde_json::from_value(value).ok()).collect())
}

/// Remember a delivered cause until folder skills work again or the cause changes.
pub(in crate::agents) fn skills_result(r: &mut Registry, helper: &str, cause: Option<&str>) -> bool {
    let index = r.system_news.iter().position(|n| n.survives_restart() && n.subject() == helper);
    if let Some(i) = index {
        if matches!(&r.system_news[i].kind, SystemNewsKind::Skills { cause: was, .. } if Some(was.as_str()) == cause) {
            return false;
        }
        r.system_news.remove(i);
    }
    if let Some(cause) = cause.filter(|_| top_chat(r, helper).is_some_and(|chat| chat != helper)) {
        r.system_news.push(SystemNews::skills(helper, cause));
        return true;
    }
    index.is_some()
}

pub(super) fn breaker(r: &mut Registry, caller: &str, helper: &str, at: u64) {
    let entry = SystemNews { kind: SystemNewsKind::Breaker { caller: caller.to_string(), helper: helper.to_string(), at } };
    if entry.recipient(r).is_some() && !r.system_news.contains(&entry) {
        r.system_news.push(entry);
    }
}

pub(super) fn news(pty: &crate::pty::PtyManager) -> HashMap<String, Vec<(SystemNews, String)>> {
    let snapshot = reg().lock().unwrap_or_else(|e| e.into_inner()).system_news.clone();
    #[cfg(test)]
    let snapshot: Vec<_> = snapshot.into_iter().filter(|n| crate::agents::sweep_includes(n.subject())).collect();
    let lifted: Vec<_> = snapshot.iter().filter(|n| n.lifted(pty)).collect();
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let Registry { system_news, known, stopped, .. } = &mut *r;
    let before = system_news.len();
    system_news.retain(|n| {
        #[cfg(test)]
        if !crate::agents::sweep_includes(n.subject()) {
            return true;
        }
        !lifted.contains(&n) && n.active(known, stopped)
    });
    let pruned = system_news.len() != before;
    let mut out: HashMap<String, Vec<_>> = HashMap::new();
    for n in &r.system_news {
        #[cfg(test)]
        if !crate::agents::sweep_includes(n.subject()) {
            continue;
        }
        let Some(chat) = n.recipient(&r) else { continue };
        if r.known.get(&chat).is_some_and(|k| !k.waiting) {
            if let Some(line) = n.line(&r) {
                out.entry(chat).or_default().push((n.clone(), line));
            }
        }
    }
    drop(r);
    if pruned {
        crate::agents::save_chats();
    }
    out
}

pub(super) fn told(r: &mut Registry, news: &[SystemNews]) {
    r.system_news.retain_mut(|n| {
        if !news.contains(n) {
            return true;
        }
        match &mut n.kind {
            SystemNewsKind::Skills { told, .. } => {
                *told = true;
                true
            }
            SystemNewsKind::Breaker { .. } => false,
        }
    });
}

pub(super) fn still_due(pty: &crate::pty::PtyManager, chat: &str, news: &[SystemNews]) -> bool {
    if news.iter().any(|n| n.lifted(pty)) {
        return false;
    }
    let r = reg().lock().unwrap_or_else(|e| e.into_inner());
    news.iter().all(|n| r.system_news.contains(n) && n.active(&r.known, &r.stopped) && n.recipient(&r).as_deref() == Some(chat))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::persist::{take_up_chats, SavedChats};
    use crate::agents::registry::register;
    use crate::agents::teardown::{forget, turn_folder_skills};
    use crate::provider::Provider;

    #[test]
    fn a_folder_skills_failure_is_told_to_the_top_chat_once_until_it_changes() {
        let (chat, lead, helper) = ("skills-top", "skills-lead", "skills-helper");
        let _sweep = crate::agents::test_support::sweep_only(&[helper]);
        let account = "C:/acc/anthropic/one";
        let bg = serde_json::json!({ "background": true });
        register(lead, Provider::Claude, "C:/p", account, Some(chat), true, bg.clone());
        register(helper, Provider::Claude, "C:/p", account, Some(lead), false, bg);
        let pty = crate::pty::manager();
        let failure = |cause| turn_folder_skills(helper, Some(cause));
        failure("disk full");
        assert!(reg().lock().unwrap().system_news.iter().any(|n| n.about(helper)));
        assert!(!news(pty).contains_key(chat));
        register(chat, Provider::Codex, "C:/p", "C:/acc/openai/one", None, true, serde_json::json!({}));
        failure("disk full");
        let mut pending = news(pty);
        assert!(!pending.contains_key(lead));
        let lines = pending.remove(chat).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].1.contains("Delegate skills-helper") && lines[0].1.contains("disk full"));
        told(&mut reg().lock().unwrap(), std::slice::from_ref(&lines[0].0));
        failure("disk full");
        assert!(!news(pty).contains_key(chat));
        failure("access denied");
        assert!(news(pty)[chat][0].1.contains("access denied"));
        turn_folder_skills(helper, None);
        assert!(!news(pty).contains_key(chat));
        failure("disk full");
        assert_eq!(news(pty)[chat].len(), 1);
        forget(chat);
        failure("disk full");
        let pending = news(pty);
        assert!(!pending.contains_key(lead) && !pending.contains_key(chat), "an orphaned line has no recipient");
        forget(helper);
        assert!(!reg().lock().unwrap().system_news.iter().any(|n| n.about(helper)), "deleting the subject drops the failure");
        forget(lead);
    }

    #[test]
    fn a_restored_skills_line_goes_to_the_old_recipients_top_chat() {
        let (chat, lead, helper) = ("old-skills-top", "old-skills-lead", "old-skills-helper");
        let _sweep = crate::agents::test_support::sweep_only(&[helper]);
        let line = |cause| format!("Your helper {helper} started a turn without the folder skills the library gives its folder: {cause}");
        let outcome = "Your helper stopped on a usage limit.".to_string();
        let account = "C:/acc/anthropic/one";
        register(lead, Provider::Claude, "C:/p", account, Some(chat), true, serde_json::json!({ "background": true }));
        register(helper, Provider::Claude, "C:/p", account, Some(lead), false, serde_json::json!({ "background": true }));
        take_up_chats(SavedChats {
            news: vec![(lead.into(), line("access denied")), (lead.into(), line("disk full")), (lead.into(), outcome.clone())],
            ..SavedChats::default()
        });
        let pty = crate::pty::manager();
        assert!(!news(pty).contains_key(lead));
        register(chat, Provider::Claude, "C:/p", account, None, true, serde_json::json!({}));
        let restored = news(pty);
        assert_eq!(restored[chat].len(), 1);
        assert!(restored[chat][0].1.contains("disk full"));
        assert_eq!(crate::agents::teardown::news_for(lead), [outcome]);
        let mut r = reg().lock().unwrap();
        r.news.retain(|(to, _)| to != lead);
        drop(r);
        for id in [helper, lead, chat] {
            forget(id);
        }
    }

    #[test]
    fn take_up_restores_known_skills_and_ignores_old_breakers_and_unknown_kinds() {
        let (chat, lead, helper, gone) = ("restore-news-top", "restore-news-lead", "restore-news-helper", "restore-news-deleted");
        let _sweep = crate::agents::test_support::sweep_only(&[helper, lead, gone]);
        let account = "C:/acc/anthropic/one";
        register(lead, Provider::Claude, "C:/p", account, Some(chat), true, serde_json::json!({ "background": true }));
        register(helper, Provider::Claude, "C:/p", account, Some(lead), false, serde_json::json!({ "background": true }));
        let data = serde_json::json!({
            "chats": [], "sends": [], "systemNews": [
                {"chat": chat, "kind": {"skills": {"helper": helper, "cause": "disk full", "told": false}}},
                {"kind": {"futureNotice": {"helper": helper}}},
                {"chat": chat, "kind": {"breaker": {"caller": lead, "helper": helper, "at": 1}}},
                {"kind": {"skills": {"helper": gone, "cause": "disk full", "told": false}}}
            ]
        });
        take_up_chats(serde_json::from_value(data).unwrap());
        assert!(!reg().lock().unwrap().system_news.iter().any(|n| n.about(lead)), "take-up drops breaker news immediately");
        let pty = crate::pty::manager();
        assert!(!news(pty).contains_key(chat));
        let r = reg().lock().unwrap();
        assert!(!r.system_news.iter().any(|n| n.about(lead) || n.about(gone)));
        assert!(r.system_news.iter().any(|n| n.about(helper)), "the known delegate still has news");
        drop(r);
        register(chat, Provider::Claude, "C:/p", account, None, true, serde_json::json!({}));
        assert_eq!(news(pty)[chat].len(), 1);
        for id in [helper, lead, chat] {
            forget(id);
        }
    }
}
