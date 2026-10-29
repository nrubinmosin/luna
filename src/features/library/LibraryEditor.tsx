import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { EditorState } from '@codemirror/state';
import { EditorView, highlightActiveLine, highlightActiveLineGutter, keymap, lineNumbers } from '@codemirror/view';
import { libraryCheckSkill, libraryRead, libraryWrite, type LibraryFileDto } from '../../ipc/commands';
import { useDialogHost } from '../../shared/ui/dialogHost';
import { Icon } from '../../shared/ui/icons';
import { look } from '../accounts/ConfigEditor';
import { useLibrary } from './library.store';

/**
 * The library's instructions file, or one skill's SKILL.md, edited in Luna.
 * Saving lays it out at once. A SKILL.md is checked as it is typed the way a
 * pass checks it — front matter with this name and a description — and saved
 * even when it does not do yet: a draft, laid out nowhere until it does.
 * Ctrl+S saves; Esc asks once before throwing edits away.
 */
export function LibraryEditor({ skill, onClose }: { skill: string | null; onClose: () => void }) {
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
  const title = skill ? `SKILL.md — ${skill}` : 'Instructions — every account';

  useEffect(() => {
    void libraryRead(skill)
      .then(f => {
        setFile(f);
        setText(f.text);
      })
      .catch(e => setError(String(e)));
  }, [skill]);

  // Checked as typed, a moment after the last key.
  useEffect(() => {
    if (!skill || !file) return;
    const t = setTimeout(() => void libraryCheckSkill(skill, text).then(setProblem), 250);
    return () => clearTimeout(t);
  }, [skill, file, text]);

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
    if (await useLibrary.getState().run(libraryWrite(skill, text))) {
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
      style={{ position: 'fixed', inset: 0, background: 'rgba(0,0,0,.35)', display: 'grid', placeItems: 'center', zIndex: 80 }}
    >
      <div
        onClick={e => e.stopPropagation()}
        className="window"
        style={{
          width: 760, maxWidth: 'calc(100vw - 32px)', height: 'min(640px, calc(100vh - 48px))',
          display: 'flex', flexDirection: 'column',
          boxShadow: 'var(--shadow), var(--border-window-outer), var(--border-window-inner)'
        }}
      >
        <div className="title-bar" style={{ flex: 'none' }}>
          <div className="title-bar-text">{title}</div>
          <div className="title-bar-controls">
            <button aria-label="Close" onClick={close} />
          </div>
        </div>
        <div className="window-body" style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column', gap: 8 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, fontSize: 'var(--fs-2)', color: 'var(--faint)' }}>
            <Icon name="library" />
            <span style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }} title={file?.path}>
              {file ? `${file.path}${file.exists ? '' : ' — not there yet, saving creates it'}` : 'Reading…'}
            </span>
            <span>{skill ? 'The description is what the model sees until it opens the skill' : 'CLAUDE.md and AGENTS.md in every account'}</span>
          </div>
          <div ref={box} className="config-text" />
          <div style={{ minHeight: 18, fontSize: 'var(--fs-3)' }}>
            {error ? (
              <span style={{ color: 'oklch(.58 .2 25)' }}>{error}</span>
            ) : leaving ? (
              <span style={{ color: 'var(--wait)' }}>Unsaved changes — Esc again to throw them away</span>
            ) : problem ? (
              <span style={{ color: 'var(--wait)' }}>{problem} — saved as a draft, laid out nowhere until it is fixed</span>
            ) : saved && !dirty ? (
              <span style={{ color: 'var(--dim)', display: 'inline-flex', alignItems: 'center', gap: 6 }}>
                <Icon name="check" size={13} /> Saved and laid out. New sessions start with it.
              </span>
            ) : null}
          </div>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <span style={{ flex: 1 }} />
            <button onClick={close}>Close</button>
            <button className="primary" onClick={() => void save()} disabled={!dirty || saving}>
              {saving ? 'Saving…' : 'Save'}
            </button>
          </div>
        </div>
      </div>
    </div>,
    host
  );
}
