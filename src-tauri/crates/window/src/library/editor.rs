//! One of the library's files in the editor: an instructions file, a skill's
//! SKILL.md, or a system text's override (which opens on Luna's own text
//! where there is none yet). gpui-component's editor with Markdown
//! highlighting and a steady caret: left alone it schedules nothing.
//!
//! Saving lays it out at once (the core's pass after `library_write`). A
//! SKILL.md or a system text is checked a moment after the last key, the way
//! the core checks it before laying it out or serving it, and is saved even
//! when it does not do yet: a draft, laid out nowhere (or Luna's own served)
//! until it does.

use super::{Entry, Library, LibraryFile};
use crate::host::Host;
use crate::panels::{ask, cmd};
use gpui::{App, AppContext, Context, Entity, Subscription, Task, Window};
use gpui_component::input::{EditorState, InputEvent};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

/// How long after the last key a SKILL.md is checked.
const CHECK_AFTER: Duration = Duration::from_millis(250);

pub struct EditorDoc {
    host: Arc<dyn Host>,
    library: Entity<Library>,
    pub entry: Entry,
    pub state: Entity<EditorState>,
    /// The file as read or last saved: what the text is dirty against.
    pub file: Option<LibraryFile>,
    /// Why the text would not do, as the core checks it.
    pub problem: Option<String>,
    /// Why reading or saving failed.
    pub error: Option<String>,
    /// Saved, and not changed since.
    pub saved: bool,
    pub saving: bool,
    check: Option<Task<()>>,
    _changes: Subscription,
}

impl EditorDoc {
    pub fn new(host: Arc<dyn Host>, library: Entity<Library>, entry: Entry, window: &mut Window, cx: &mut Context<Self>) -> EditorDoc {
        let state = cx.new(|cx| EditorState::new(window, cx).language("markdown").soft_wrap(true).line_number(true));
        let changes = cx.subscribe_in(&state, window, |this, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.changed(cx);
            }
        });
        let read = ask::<LibraryFile>(&host, cmd::LIBRARY_READ, json!({ "entry": entry }));
        cx.spawn_in(window, async move |this, cx| {
            let got = read.await;
            let _ = this.update_in(cx, |doc, window, cx| {
                match got {
                    Ok(file) => {
                        doc.state.update(cx, |s, cx| s.set_value(file.text.clone(), window, cx));
                        doc.file = Some(file);
                        doc.check_now(cx);
                    }
                    Err(e) => doc.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
        EditorDoc {
            host,
            library,
            entry,
            state,
            file: None,
            problem: None,
            error: None,
            saved: false,
            saving: false,
            check: None,
            _changes: changes,
        }
    }

    pub fn text(&self, cx: &App) -> String {
        self.state.read(cx).value().to_string()
    }

    /// Changed since it was read or saved.
    pub fn dirty(&self, cx: &App) -> bool {
        self.file.as_ref().is_some_and(|f| f.text != self.text(cx))
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.saved = false;
        self.error = None;
        let timer = cx.background_executor().timer(CHECK_AFTER);
        // The last key's check alone: the one before is dropped with its task.
        self.check = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |doc, cx| doc.check_now(cx));
        }));
        cx.notify();
    }

    /// The text checked as the core checks it; instructions are not.
    fn check_now(&mut self, cx: &mut Context<Self>) {
        let text = self.text(cx);
        let answer = match &self.entry {
            Entry::Skill(n) => ask::<Option<String>>(&self.host, cmd::LIBRARY_CHECK_SKILL, json!({ "skill": n, "text": text })),
            Entry::System(n) => ask::<Option<String>>(&self.host, cmd::LIBRARY_CHECK_SYSTEM, json!({ "name": n, "text": text })),
            Entry::Instructions(_) => return,
        };
        self.check = Some(cx.spawn(async move |this, cx| {
            let problem = answer.await.ok().flatten();
            let _ = this.update(cx, |doc, cx| {
                if doc.problem != problem {
                    doc.problem = problem;
                    cx.notify();
                }
            });
        }));
    }

    /// Saves the text and lays it out; nothing while it is as saved.
    pub fn save(&mut self, cx: &mut Context<Self>) {
        if !self.dirty(cx) || self.saving {
            return;
        }
        let text = self.text(cx);
        self.saving = true;
        self.error = None;
        let entry = self.entry.clone();
        let task = self.library.update(cx, |l, cx| l.run(cmd::LIBRARY_WRITE, json!({ "entry": entry, "text": text }), cx));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let ok = task.await;
            let _ = this.update(cx, |doc, cx| {
                doc.saving = false;
                if ok {
                    if let Some(f) = &mut doc.file {
                        f.text = text;
                        f.exists = true;
                    }
                    doc.saved = true;
                } else {
                    doc.error = doc.library.read(cx).error.clone();
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// What the line under the editor says of where the text stands.
    pub fn status(&self, cx: &App) -> (StatusKind, String) {
        if let Some(e) = &self.error {
            return (StatusKind::Trouble, e.clone());
        }
        if self.file.is_none() {
            return (StatusKind::Plain, "Reading...".into());
        }
        if self.saving {
            return (StatusKind::Working, "Saving and laying it out...".into());
        }
        if let Some(p) = &self.problem {
            let then = match self.entry {
                Entry::System(_) => " - saved as it is, Luna serves its own until it is fixed",
                _ => " - saved as a draft, laid out nowhere until it is fixed",
            };
            return (StatusKind::Attention, format!("{p}{then}"));
        }
        if self.dirty(cx) {
            let key = if cfg!(target_os = "macos") { "Cmd+S" } else { "Ctrl+S" };
            return (StatusKind::Plain, format!("Not saved: {key} saves."));
        }
        if self.saved {
            let said = match self.entry {
                Entry::System(_) => "Saved. Sessions take it as they next read their role's text.",
                _ => "Saved and laid out. New sessions start with it.",
            };
            return (StatusKind::Done, said.into());
        }
        let file = self.file.as_ref().expect("checked above");
        let where_ = if file.exists {
            file.path.clone()
        } else if matches!(self.entry, Entry::System(_)) {
            format!("{}: Luna's own text to start from, saving makes it yours", file.path)
        } else {
            format!("{}: not there yet, saving makes it", file.path)
        };
        (StatusKind::Plain, where_)
    }
}

/// How the editor's status line is colored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKind {
    Plain,
    Working,
    Attention,
    Trouble,
    Done,
}
