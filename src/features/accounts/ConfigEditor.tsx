import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { json } from '@codemirror/lang-json';
import { HighlightStyle, StreamLanguage, bracketMatching, indentOnInput, syntaxHighlighting } from '@codemirror/language';
import { toml } from '@codemirror/legacy-modes/mode/toml';
import { linter, lintGutter, type Diagnostic } from '@codemirror/lint';
import { EditorState } from '@codemirror/state';
import { EditorView, highlightActiveLine, highlightActiveLineGutter, keymap, lineNumbers } from '@codemirror/view';
import { tags } from '@lezer/highlight';
import {
  checkAccountConfig, formatAccountConfig, readAccountConfig, writeAccountConfig, type ConfigFileDto, type ConfigProblemDto
} from '../../ipc/commands';
import { useDialogHost } from '../../shared/ui/dialogHost';
import { Icon } from '../../shared/ui/icons';
import { PROVIDER_LABEL, type Account, type Provider } from '../../shared/types';

/** Colours from the theme's tokens, so light and dark need nothing of their own. */
export const highlight = HighlightStyle.define([
  { tag: [tags.propertyName, tags.definition(tags.propertyName), tags.variableName], color: 'var(--code-key)' },
  { tag: [tags.string, tags.special(tags.string)], color: 'var(--code-string)' },
  { tag: [tags.number, tags.bool, tags.null, tags.atom], color: 'var(--code-literal)' },
  { tag: [tags.comment, tags.lineComment], color: 'var(--faint)', fontStyle: 'italic' },
  { tag: [tags.heading, tags.meta, tags.bracket, tags.squareBracket], color: 'var(--code-key)', fontWeight: '700' }
]);

export const look = EditorView.theme({
  '&': { flex: '1', minHeight: '0', background: 'var(--term)', color: 'var(--fg)', font: '13px/1.55 "JetBrains Mono", ui-monospace, monospace' },
  '&.cm-focused': { outline: 'none' },
  '.cm-scroller': { fontFamily: 'inherit', lineHeight: 'inherit' },
  '.cm-content': { caretColor: 'var(--fg)', padding: '10px 0' },
  '.cm-cursor': { borderLeftColor: 'var(--fg)' },
  '.cm-gutters': { background: 'var(--term)', color: 'var(--faint)', border: 'none', borderRight: '1px solid var(--line)' },
  '.cm-activeLine, .cm-activeLineGutter': { background: 'color-mix(in oklab, var(--selection) 35%, transparent)' },
  '&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection': { background: 'var(--selection) !important' },
  '.cm-tooltip': { background: 'var(--card)', color: 'var(--fg)', border: '1px solid var(--line)' }
});

/** A problem as the editor marks it: the whole line it is on. */
function diagnose(view: EditorView, p: ConfigProblemDto): Diagnostic[] {
  const doc = view.state.doc;
  const line = doc.line(Math.min(Math.max(p.line ?? 1, 1), doc.lines));
  return [{ from: line.from, to: Math.max(line.to, line.from), severity: 'error', message: p.message }];
}

/**
 * The account's own settings file, edited by hand: Claude Code's
 * settings.json, Codex's config.toml. Checked as it is typed — by the side
 * that writes the file, so the editor says exactly what the save would — and
 * laid out on save the way the CLI writes it, comments and key order kept.
 * A file that does not parse is never written. Ctrl+S saves.
 */
export function ConfigEditor({ account, onClose }: { account: Account; onClose: () => void }) {
  const host = useDialogHost();
  const [file, setFile] = useState<ConfigFileDto | null>(null);
  const [text, setText] = useState('');
  const [problem, setProblem] = useState<ConfigProblemDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [saving, setSaving] = useState(false);
  const [leaving, setLeaving] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const claude = account.provider === 'claude';
  const fileName = claude ? 'settings.json' : 'config.toml';
  const dirty = !!file && text !== file.text;

  useEffect(() => {
    void readAccountConfig(account.provider, account.path)
      .then(f => {
        setFile(f);
        setText(f.text);
      })
      .catch(e => setError(String(e)));
  }, [account.provider, account.path]);

  // The editor is made once the file is in, and lives as long as the dialog.
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
          indentOnInput(),
          bracketMatching(),
          EditorState.tabSize.of(2),
          keymap.of([indentWithTab, ...defaultKeymap, ...historyKeymap]),
          claude ? json() : StreamLanguage.define(toml),
          syntaxHighlighting(highlight),
          lintGutter(),
          linter(lint(account.provider, setProblem), { delay: 250 }),
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

  /** Puts text into the editor as one undoable change. */
  const replace = (next: string) => {
    const v = view.current;
    if (v && next !== v.state.doc.toString()) v.dispatch({ changes: { from: 0, to: v.state.doc.length, insert: next } });
  };

  const format = async () => {
    try {
      replace(await formatAccountConfig(account.provider, text));
    } catch {
      // the problem line already says why
    }
  };

  const save = async () => {
    if (!dirty || saving || problem) return;
    setSaving(true);
    setError(null);
    try {
      const laidOut = await formatAccountConfig(account.provider, text);
      replace(laidOut);
      await writeAccountConfig(account.provider, account.path, laidOut);
      setFile(f => (f ? { ...f, text: laidOut, exists: true } : f));
      setSaved(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  // Escape asks once before throwing edits away.
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
  const where = problem?.line ? `Line ${problem.line}${problem.column ? `:${problem.column}` : ''} — ` : '';

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
          <div className="title-bar-text">{fileName} — {account.name}</div>
          <div className="title-bar-controls">
            <button aria-label="Close" onClick={close} />
          </div>
        </div>
        <div className="window-body" style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column', gap: 8 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, fontSize: 'var(--fs-2)', color: 'var(--faint)' }}>
            <Icon name={account.provider} />
            <span style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }} title={file?.path}>
              {file ? `${file.path}${file.exists ? '' : ' — not there yet, saving creates it'}` : 'Reading…'}
            </span>
            <span>{PROVIDER_LABEL[account.provider]} reads it when a session starts</span>
          </div>
          <div ref={box} className="config-text" />
          <div style={{ minHeight: 18, fontSize: 'var(--fs-3)' }}>
            {problem ? (
              <span style={{ color: 'oklch(.58 .2 25)' }}>{where}{problem.message}</span>
            ) : error ? (
              <span style={{ color: 'oklch(.58 .2 25)' }}>{error}</span>
            ) : leaving ? (
              <span style={{ color: 'var(--wait)' }}>Unsaved changes — Esc again to throw them away</span>
            ) : saved && !dirty ? (
              <span style={{ color: 'var(--dim)', display: 'inline-flex', alignItems: 'center', gap: 6 }}>
                <Icon name="check" size={13} /> Saved. New sessions on this account start with it.
              </span>
            ) : null}
          </div>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <button onClick={() => void format()} disabled={!!problem || !file} title="Lay the file out the way the CLI writes it; saving does this too">
              Format
            </button>
            <span style={{ flex: 1 }} />
            <button onClick={close}>Close</button>
            <button className="primary" onClick={() => void save()} disabled={!dirty || saving || !!problem}>
              {saving ? 'Saving…' : 'Save'}
            </button>
          </div>
        </div>
      </div>
    </div>,
    host
  );
}

/** The linter: asks the core, and keeps the problem for the line under the editor. */
function lint(provider: Provider, report: (p: ConfigProblemDto | null) => void) {
  return async (view: EditorView): Promise<Diagnostic[]> => {
    const p = await checkAccountConfig(provider, view.state.doc.toString()).catch(() => null);
    report(p);
    return p ? diagnose(view, p) : [];
  };
}
