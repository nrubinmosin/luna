import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { EditorState } from '@codemirror/state';
import { EditorView, highlightActiveLine, highlightActiveLineGutter, keymap, lineNumbers } from '@codemirror/view';
import {
  libraryCheckSkill,
  libraryCheckSystem,
  libraryRead,
  libraryWrite,
  type EditorEntry,
  type LibraryFileDto
} from '../../ipc/library';
import { useDialogHost } from '../../shared/ui/dialogHost';
import { Icon } from '../../shared/ui/icons';
import { look } from '../accounts/ConfigEditor';
import { useLibrary } from './library.store';

/**
 * One of the library's instructions files, or one skill's SKILL.md, edited in
 * Luna. Saving lays it out at once. A SKILL.md is checked as it is typed the
 * way a pass checks it — front matter with this name and a description — and
 * saved even when it does not do yet: a draft, laid out nowhere until it does.
 * A system skill's override opens on Luna's own text where there is none yet,
 * and is checked as Luna checks it before serving it; one that does not do is
 * saved too, and Luna serves its own until it does.
 * Ctrl+S saves; Esc asks once before throwing edits away.
 */
export function LibraryEditor({ entry, onClose }: { entry: EditorEntry; onClose: () => void }) {
  const skill = 'skill' in entry ? entry.skill : null;
  const system = 'system' in entry ? entry.system : null;
  const name = 'skill' in entry ? entry.skill : 'system' in entry ? entry.system : entry.instructions;
  const host = useDialogHost();
  const [file, setFile] = useState<LibraryFileDto | null>(null);
  const [text, setText] = useState('');
  const [problem, setProblem] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [saving, setSaving] = useState(false);
  const [leaving, setLeaving] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const dirty = !!file && text !== file.text;
  const title = system
    ? `SKILL.md — ${system}, system skill`
    : skill
      ? `SKILL.md — ${skill}`
      : `Instructions — ${name}`;

  useEffect(() => {
    void libraryRead(entry)
      .then(f => {
        setFile(f);
        setText(f.text);
      })
      .catch(e => setError(String(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [skill, system, name]);

  // Checked as typed, a moment after the last key; an answer about text
  // typed over since is dropped, or a slow one would outlive a quick one.
  useEffect(() => {
    if ((!skill && !system) || !file) return;
    let stale = false;
    const check = () => (system ? libraryCheckSystem(system, text) : libraryCheckSkill(skill ?? '', text));
    const t = setTimeout(() => void check().then(p => !stale && setProblem(p)), 250);
    return () => {
      stale = true;
      clearTimeout(t);
    };
  }, [skill, system, file, text]);

  useEffect(() => {
    if (!file || !box.current || view.current) return;
    view.current = new EditorView({
      parent: box.current,
      state: EditorState.create({
        doc: file.text,
        extensions: [
          lineNumbers(),
          highlightActiveLineGutter(),
          highlightActiveLine(),
          history(),
          EditorView.lineWrapping,
          keymap.of([indentWithTab, ...defaultKeymap, ...historyKeymap]),
          look,
          EditorView.updateListener.of(u => {
            if (!u.docChanged) return;
            setText(u.state.doc.toString());
            setSaved(false);
            setLeaving(false);
          })
        ]
      })
    });
    view.current.focus();
    return () => {
      view.current?.destroy();
      view.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [file === null]);

  const save = async () => {
    if (!dirty || saving) return;
    setSaving(true);
    setError(null);
    if (await useLibrary.getState().run(libraryWrite(entry, text))) {
      setFile(f => (f ? { ...f, text, exists: true } : f));
      setSaved(true);
    } else {
      setError(useLibrary.getState().error);
    }
    setSaving(false);
  };

  const close = () => {
    if (dirty && !leaving) {
      setLeaving(true);
      return;
    }
    onClose();
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        close();
      } else if ((e.ctrlKey || e.metaKey) && e.code === 'KeyS') {
        e.preventDefault();
        e.stopPropagation();
        void save();
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  });

  if (!host) return null;
  return createPortal(
    <div
      className="scrim"
      onClick={e => {
        e.stopPropagation();
        close();
      }}
      style={{
        position: 'fixed',
        inset: 0,
        background: 'rgba(0,0,0,.35)',
        display: 'grid',
        placeItems: 'center',
        zIndex: 80
      }}
    >
      <div
        onClick={e => e.stopPropagation()}
        className="window"
        style={{
          width: 760,
          height: 'min(640px, calc(100vh - 48px))',
          boxShadow: 'var(--shadow), var(--border-window-outer), var(--border-window-inner)'
        }}
      >
        <div className="title-bar">
          <div className="title-bar-text">{title}</div>
          <div className="title-bar-controls">
            <button aria-label="Close" onClick={close} />
          </div>
        </div>
        <div className="window-body" style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
          <div
            style={{ display: 'flex', alignItems: 'center', gap: 8, fontSize: 'var(--fs-2)', color: 'var(--faint)' }}
          >
            <Icon name="library" />
            <span
              style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}
              title={file?.path}
            >
              {file
                ? `${file.path}${file.exists ? '' : system ? ' — Luna’s own text to start from: saving makes it yours' : ' — not there yet, saving creates it'}`
                : 'Reading…'}
            </span>
            <span>
              {system
                ? 'Served to chats with Auto-handoff on, never laid out'
                : skill
                  ? 'The description is what the model sees until it opens the skill'
                  : 'CLAUDE.md and AGENTS.md in the accounts that get it'}
            </span>
          </div>
          <div ref={box} className="config-text" />
          <div style={{ minHeight: 18, fontSize: 'var(--fs-3)' }}>
            {error ? (
              <span style={{ color: 'oklch(.58 .2 25)' }}>{error}</span>
            ) : leaving ? (
              <span style={{ color: 'var(--wait)' }}>Unsaved changes — Esc again to throw them away</span>
            ) : problem ? (
              <span style={{ color: 'var(--wait)' }}>
                {problem}
                {system
                  ? ' — saved, but Luna serves its own until it is fixed'
                  : ' — saved as a draft, laid out nowhere until it is fixed'}
              </span>
            ) : saved && !dirty ? (
              <span style={{ color: 'var(--dim)', display: 'inline-flex', alignItems: 'center', gap: 6 }}>
                <Icon name="check" size={13} />{' '}
                {system
                  ? 'Saved. Chats with Auto-handoff on are served it from their next handoff.'
                  : 'Saved and laid out. New sessions start with it.'}
              </span>
            ) : null}
          </div>
        </div>
        <div className="dialog-foot">
          <button onClick={close}>Close</button>
          <button className="primary" onClick={() => void save()} disabled={!dirty || saving}>
            {saving ? 'Saving…' : 'Save'}
          </button>
        </div>
      </div>
    </div>,
    host
  );
}
