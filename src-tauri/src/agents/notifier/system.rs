//! System news waiting for the person's chat.

use crate::agents::notifier::lines::breaker_lifted;
use crate::agents::notifier::CALLER_RETURN_MS;
use crate::agents::registry::{named, reg, top_chat, Registry};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::agents) struct SystemNews {
    pub(in crate::agents) chat: String,
    pub(in crate::agents) kind: SystemNewsKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::agents) enum SystemNewsKind {
    Skills { helper: String, cause: String, told: bool },
    Breaker { caller: String, helper: String, at: u64 },
}

impl SystemNews {
    pub(in crate::agents) fn about(&self, id: &str) -> bool {
        match &self.kind {
            SystemNewsKind::Skills { helper, .. } => helper == id,
            SystemNewsKind::Breaker { caller, helper, .. } => caller == id || helper == id,
        }
    }

    /// Older cores saved this as plain text for the immediate parent. Read
    /// that format at restore only, then resolve the parent's top chat when
    /// its lineage is known.
    pub(in crate::agents) fn old_skills(chat: &str, line: &str) -> Option<Self> {
        let (helper, cause) =
            line.strip_prefix("Your helper ")?.split_once(" started a turn without the folder skills the library gives its folder: ")?;
        if helper.is_empty() || helper.contains(char::is_whitespace) {
            return None;
        }
        Some(Self {
            chat: chat.to_string(),
            kind: SystemNewsKind::Skills { helper: helper.to_string(), cause: cause.to_string(), told: false },
        })
    }

    fn line(&self, r: &Registry) -> Option<String> {
        match &self.kind {
            SystemNewsKind::Skills { helper, cause, told: false } => {
                Some(format!("Delegate {} started a turn without its folder skills: {cause}", named(r, helper)))
            }
            SystemNewsKind::Skills { told: true, .. } => None,
            SystemNewsKind::Breaker { caller, helper, .. } => Some(format!(
                "Luna paused turn and job notices to {} about {} after a notice loop. Prompts and questions still come. Typing into either session resumes notices.",
                named(r, caller),
                named(r, helper),
            )),
        }
    }

    pub(super) fn lifted(&self, pty: &crate::pty::PtyManager) -> bool {
        match &self.kind {
            SystemNewsKind::Breaker { caller, helper, at } => breaker_lifted(pty, caller, helper, *at),
            SystemNewsKind::Skills { .. } => false,
        }
    }
}

/// The newest failure for a delegate. Remember a delivered cause until the
/// folder skills work again or the cause changes.
pub(in crate::agents) fn skills_result(r: &mut Registry, helper: &str, cause: Option<&str>) -> bool {
    let index = r.system_news.iter().position(|n| matches!(&n.kind, SystemNewsKind::Skills { helper: id, .. } if id == helper));
    let next = cause.zip(top_chat(r, helper).filter(|chat| chat != helper)).map(|(cause, chat)| SystemNews {
        chat,
        kind: SystemNewsKind::Skills { helper: helper.to_string(), cause: cause.to_string(), told: false },
    });
    if let Some(i) = index {
        if let Some(next) = next {
            let previous = &r.system_news[i];
            let previous_chat = top_chat(r, &previous.chat);
            if previous_chat.as_deref().unwrap_or(&previous.chat) == next.chat
                && matches!(&previous.kind, SystemNewsKind::Skills { cause: was, .. } if Some(was.as_str()) == cause)
            {
                return false;
            }
            r.system_news[i] = next;
        } else {
            r.system_news.remove(i);
        }
    } else if let Some(next) = next {
        r.system_news.push(next);
    } else {
        return false;
    }
    true
}

pub(super) fn breaker(r: &mut Registry, caller: &str, helper: &str, at: u64) -> bool {
    let Some(chat) = top_chat(r, caller).filter(|chat| chat != caller) else { return false };
    let kind = SystemNewsKind::Breaker { caller: caller.to_string(), helper: helper.to_string(), at };
    let entry = SystemNews { chat, kind };
    if r.system_news.contains(&entry) {
        return false;
    }
    r.system_news.push(entry);
    true
}

pub(super) fn news(pty: &crate::pty::PtyManager, up_ms: u64) -> HashMap<String, Vec<(SystemNews, String)>> {
    let snapshot = reg().lock().unwrap_or_else(|e| e.into_inner()).system_news.clone();
    let lifted: Vec<_> = snapshot.iter().filter(|n| n.lifted(pty)).collect();
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let Registry { system_news, known, .. } = &mut *r;
    let before = system_news.len();
    system_news.retain(|n| !lifted.contains(&n) && (known.contains_key(&n.chat) || up_ms < CALLER_RETURN_MS));
    let pruned = system_news.len() != before;
    let mut out: HashMap<String, Vec<_>> = HashMap::new();
    for n in &r.system_news {
        // A restored skills entry can still name the old parent. All new
        // entries name the top chat when queued.
        let Some(chat) = top_chat(&r, &n.chat) else { continue };
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
    news.iter().all(|n| r.system_news.contains(n) && top_chat(&r, &n.chat).as_deref() == Some(chat))
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
        let account = "C:/acc/anthropic/one";
        let bg = serde_json::json!({ "background": true });
        register(lead, Provider::Claude, "C:/p", account, Some(chat), true, bg.clone());
        register(helper, Provider::Claude, "C:/p", account, Some(lead), false, bg);
        let pty = crate::pty::manager();
        let failure = |cause| turn_folder_skills(helper, Some(cause));
        failure("disk full");
        assert!(reg().lock().unwrap().system_news.iter().any(|n| n.chat == chat && n.about(helper)));
        assert!(!news(pty, 0).contains_key(chat));
        register(chat, Provider::Codex, "C:/p", "C:/acc/openai/one", None, true, serde_json::json!({}));
        failure("disk full");
        let mut pending = news(pty, 0);
        assert!(!pending.contains_key(lead));
        let lines = pending.remove(chat).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].1.contains("Delegate skills-helper") && lines[0].1.contains("disk full"));
        told(&mut reg().lock().unwrap(), std::slice::from_ref(&lines[0].0));
        failure("disk full");
        assert!(!news(pty, 0).contains_key(chat));
        failure("access denied");
        assert!(news(pty, 0)[chat][0].1.contains("access denied"));
        turn_folder_skills(helper, None);
        assert!(!news(pty, 0).contains_key(chat));
        failure("disk full");
        assert_eq!(news(pty, 0)[chat].len(), 1);
        forget(chat);
        failure("disk full");
        assert!(!reg().lock().unwrap().system_news.iter().any(|n| n.about(helper)));
        forget(helper);
        forget(lead);
    }

    #[test]
    fn a_restored_skills_line_goes_to_the_old_recipients_top_chat() {
        let (chat, lead, helper) = ("old-skills-top", "old-skills-lead", "old-skills-helper");
        let line = format!("Your helper {helper} started a turn without the folder skills the library gives its folder: disk full");
        let outcome = "Your helper stopped on a usage limit.".to_string();
        take_up_chats(SavedChats { news: vec![(lead.into(), line), (lead.into(), outcome.clone())], ..SavedChats::default() });
        let pty = crate::pty::manager();
        assert!(!news(pty, 0).contains_key(lead));
        let account = "C:/acc/anthropic/one";
        register(lead, Provider::Claude, "C:/p", account, Some(chat), true, serde_json::json!({ "background": true }));
        register(chat, Provider::Claude, "C:/p", account, None, true, serde_json::json!({}));
        assert!(news(pty, 0)[chat][0].1.contains("disk full"));
        assert_eq!(crate::agents::teardown::news_for(lead), [outcome]);
        let mut r = reg().lock().unwrap();
        r.news.retain(|(to, _)| to != lead);
        drop(r);
        forget(lead);
        forget(chat);
    }
}
