//! One of the library's files in the editor: an instructions file, a skill's
//! SKILL.md, or a system text's override (which opens on Luna's own text
//! where there is none yet). gpui-component's editor with Markdown
//! highlighting and a steady caret: left alone it schedules nothing.
//!
//! Saving lays it out at once (the core's pass after the write). A SKILL.md
//! or a system text is checked a moment after the last key, the way the core
//! checks it before laying it out or serving it, and is saved even when it
//! does not do yet: a draft, laid out nowhere (or Luna's own served) until it
//! does.
//!
//! A save is bound to what the text was read from (`library_write_checked`):
//! the library's folder and the file's revision. Changed on disk since, or
//! the library moved, nothing is written; the text stays, and the editor
//! offers to reload the file or overwrite it. Text not saved is kept with
//! the host as it changes (`set_library_draft`), so a window that goes with
//! it (the tray's Quit or Restart window) offers it at the next open.

use super::{Draft, Entry, Library, LibraryFile, Written};
use crate::host::Host;
use crate::panels::{ask, cmd};
use gpui::{App, AppContext, Context, Entity, Subscription, Task, Window};
use gpui_component::input::{EditorState, InputEvent};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// How long after the last key a SKILL.md is checked.
const CHECK_AFTER: Duration = Duration::from_millis(250);

pub struct EditorDoc {
    host: Arc<dyn Host>,
    library: Entity<Library>,
    pub entry: Entry,
    pub state: Entity<EditorState>,
    /// The file as read or last saved: what the text is dirty against, and
    /// what a save is bound to. None until the read is answered, while the
    /// editor takes no input.
    pub file: Option<LibraryFile>,
    /// Why the text would not do, as the core checks it.
    pub problem: Option<String>,
    /// Why reading or saving failed.
    pub error: Option<String>,
    /// Why the last save wrote nothing: the file changed on disk, or the
    /// library moved. Reload or Overwrite answers it.
    pub conflict: Option<String>,
    /// Saved, and not changed since.
    pub saved: bool,
    pub saving: bool,
    /// This editor's text is the draft the host keeps.
    drafted: bool,
    /// The id the host keeps this editor's draft under.
    draft_id: String,
    /// Saved, but laying the library out after failed: why.
    pub layout_error: Option<String>,
    check: Option<Task<()>>,
    _changes: Subscription,
}

impl EditorDoc {
    /// Opens `entry`; with `draft`, its text in place of the file's once the
    /// file is read, bound to what the draft was read from.
    pub fn new(
        host: Arc<dyn Host>,
        library: Entity<Library>,
        entry: Entry,
        draft: Option<Draft>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> EditorDoc {
        let state = cx.new(|cx| EditorState::new(window, cx).language("markdown").soft_wrap(true).line_number(true));
        let changes = cx.subscribe_in(&state, window, |this, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.changed(cx);
            }
        });
        let doc = EditorDoc {
            host,
            library,
            entry,
            state,
            file: None,
            problem: None,
            error: None,
            conflict: None,
            saved: false,
            saving: false,
            // A draft restored is this editor's: thrown away, it goes.
            drafted: draft.is_some(),
            draft_id: draft.as_ref().map_or_else(new_draft_id, |d| d.id.clone()),
            layout_error: None,
            check: None,
            _changes: changes,
        };
        doc.read(draft, window, cx);
        doc
    }

    /// Reads the file and shows it (or `draft`'s text over it).
    fn read(&self, draft: Option<Draft>, window: &mut Window, cx: &mut Context<Self>) {
        let read = ask::<LibraryFile>(&self.host, cmd::LIBRARY_READ, json!({ "entry": self.entry }));
        cx.spawn_in(window, async move |this, cx| {
            let got = read.await;
            let _ = this.update_in(cx, |doc, window, cx| {
                match got {
                    Ok(mut file) => {
                        let text = match draft {
                            Some(d) => {
                                // Bound to what the draft was read from: a
                                // file changed since conflicts on save.
                                file.root = d.root;
                                file.root_id = d.root_id;
                                file.revision = d.revision;
                                d.text
                            }
                            None => file.text.clone(),
                        };
                        doc.file = Some(file);
                        doc.state.update(cx, |s, cx| s.set_value(text, window, cx));
                        doc.check_now(cx);
                    }
                    Err(e) => doc.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
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
        self.layout_error = None;
        self.keep_draft(cx);
        let timer = cx.background_executor().timer(CHECK_AFTER);
        // The last key's check alone: the one before is dropped with its task.
        self.check = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |doc, cx| doc.check_now(cx));
        }));
        cx.notify();
    }

    /// The text kept with the host while it differs from the file, forgotten
    /// once it does not; a draft this editor did not keep (another file's,
    /// still on offer) is left be.
    fn keep_draft(&mut self, cx: &App) {
        let draft = match (&self.file, self.dirty(cx)) {
            (Some(f), true) => Some(Draft {
                id: self.draft_id.clone(),
                entry: self.entry.clone(),
                root: f.root.clone(),
                root_id: f.root_id.clone(),
                revision: f.revision.clone(),
                text: self.text(cx),
            }),
            _ => None,
        };
        if draft.is_none() && !self.drafted {
            return;
        }
        self.drafted = draft.is_some();
        match draft {
            Some(d) => keep(&self.host, d),
            None => forget(&self.host, &self.draft_id),
        }
    }

    /// A system skill's text checked as the core checks it; a skill's and
    /// instructions are not: what SKILL.md says is the CLIs' to judge.
    fn check_now(&mut self, cx: &mut Context<Self>) {
        let text = self.text(cx);
        let answer = match &self.entry {
            Entry::System(n) => ask::<Option<String>>(&self.host, cmd::LIBRARY_CHECK_SYSTEM, json!({ "name": n, "text": text })),
            Entry::Skill(_) | Entry::Instructions(_) => return,
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

    /// Saves the text and lays it out, held to what it was read from;
    /// nothing while it is as saved.
    pub fn save(&mut self, cx: &mut Context<Self>) {
        self.write(false, cx);
    }

    /// Overwrite: saves over a file changed on disk since it was read. Never
    /// into another library than the one it was read from.
    pub fn overwrite(&mut self, cx: &mut Context<Self>) {
        self.write(true, cx);
    }

    fn write(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(file) = &self.file else { return };
        if !self.dirty(cx) || self.saving {
            return;
        }
        let text = self.text(cx);
        self.saving = true;
        self.error = None;
        self.conflict = None;
        self.layout_error = None;
        let args = json!({
            "entry": self.entry,
            "text": text,
            "root": file.root,
            "rootId": file.root_id,
            "revision": file.revision,
            "force": force,
        });
        let task = self.library.update(cx, |l, cx| l.run_answer::<Written>(cmd::LIBRARY_WRITE_CHECKED, args, |w| w.overview.clone(), cx));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let got = task.await;
            let _ = this.update(cx, |doc, cx| {
                doc.saving = false;
                match got {
                    Ok(Written { conflict: Some(why), .. }) => doc.conflict = Some(why),
                    Ok(w) => {
                        if let Some(f) = &mut doc.file {
                            f.text = text;
                            f.exists = true;
                            f.revision = w.revision;
                        }
                        doc.saved = true;
                        // Saved: laying it out after is said apart.
                        doc.layout_error = w.layout_error;
                        doc.keep_draft(cx);
                    }
                    Err(e) => doc.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The text thrown away (the discard question answered yes): the draft
    /// the host keeps of it goes too.
    pub fn forget_draft(&mut self) {
        if std::mem::take(&mut self.drafted) {
            forget(&self.host, &self.draft_id);
        }
    }

    /// Reload: the file as it is on disk now, in place of the text.
    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.conflict = None;
        self.error = None;
        self.saved = false;
        self.file = None;
        if std::mem::take(&mut self.drafted) {
            forget(&self.host, &self.draft_id);
        }
        self.read(None, window, cx);
        cx.notify();
    }

    /// What the line under the editor says of where the text stands.
    pub fn status(&self, cx: &App) -> (StatusKind, String) {
        if let Some(e) = &self.error {
            return (StatusKind::Trouble, e.clone());
        }
        if let Some(c) = &self.conflict {
            return (
                StatusKind::Attention,
                format!("{c} Your text is still here: Reload takes the file's, Overwrite saves yours over it."),
            );
        }
        if self.file.is_none() {
            return (StatusKind::Plain, "Reading...".into());
        }
        if self.saving {
            return (StatusKind::Working, "Saving and laying it out...".into());
        }
        if let Some(p) = &self.problem {
            return (StatusKind::Attention, format!("{p} - saved as it is, Luna serves its own until it is fixed"));
        }
        if self.dirty(cx) {
            let key = if cfg!(target_os = "macos") { "Cmd+S" } else { "Ctrl+S" };
            return (StatusKind::Plain, format!("Not saved: {key} saves."));
        }
        if let (true, Some(e)) = (self.saved, &self.layout_error) {
            return (StatusKind::Attention, format!("Saved, but laying it out failed: {e}"));
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

/// Keeps `draft` with the host, in place of any kept.
fn keep(host: &Arc<dyn Host>, draft: Draft) {
    if let Err(e) = host.send(cmd::SET_LIBRARY_DRAFT, json!({ "draft": draft })) {
        log::warn!("set_library_draft: {e}");
    }
}

/// Forgets the draft kept under `id`; one kept since under another stays.
pub(super) fn forget(host: &Arc<dyn Host>, id: &str) {
    if let Err(e) = host.send(cmd::SET_LIBRARY_DRAFT, json!({ "draft": null, "forget": id })) {
        log::warn!("set_library_draft: {e}");
    }
}

/// An id for an editor's draft, new to this run and to the last.
fn new_draft_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("{now:x}-{}", NEXT.fetch_add(1, Ordering::Relaxed))
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
