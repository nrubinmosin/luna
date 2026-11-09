import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { EditorView } from '@codemirror/view';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { SystemSkillState } from '../../ipc/applied';
import * as ipc from '../../ipc/library';
import type { LibraryOverviewDto, LibrarySkillDto } from '../../ipc/library';
import { LibraryEditor } from './LibraryEditor';
import { useLibrary } from './library.store';
import { SkillsGrid } from './SkillsGrid';
import { stateWords, SystemSkills } from './SystemSkills';

vi.mock('../../ipc/library', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/library')>()),
  librarySystemCompare: vi.fn(),
  librarySystemReset: vi.fn(() => Promise.resolve(null)),
  librarySystemAdopt: vi.fn(() => Promise.resolve(null)),
  libraryRead: vi.fn(),
  libraryWrite: vi.fn(),
  libraryCheckSystem: vi.fn(() => Promise.resolve(null)),
  libraryCheckSkill: vi.fn(() => Promise.resolve(null))
}));

const PATH = 'C:\\lib\\system\\session-handoff\\SKILL.md';
const SHA = '3054a9e14c774f592c9c2fed08da4ff7eb38befdb00321184f3d1b324c8aedd9';
const state = (s: Partial<SystemSkillState>): SystemSkillState => ({
  name: 'session-handoff',
  state: 'luna',
  version: '0.12.0',
  sha256: SHA,
  path: PATH,
  problem: null,
  ...s
});
const overview = (system: SystemSkillState, skills: LibrarySkillDto[] = []): LibraryOverviewDto => ({
  root: 'C:\\lib',
  isDefault: true,
  found: true,
  instructions: [],
  noInstructions: [],
  instructionsFor: {},
  oldInstructions: null,
  skills,
  accounts: [{ provider: 'claude', name: 'work' }],
  folders: [],
  places: [],
  foundInstructions: [],
  overridden: [],
  kept: [],
  system: [system],
  systemAside: []
});
const reserved: LibrarySkillDto = {
  name: 'session-handoff',
  description: '',
  problem: "session-handoff is the name of Luna's system skill for handoffs: no skill of that name is laid out",
  accounts: ['all'],
  audience: 'both'
};

const button = (name: RegExp | string) => screen.getByRole('button', { name });

beforeEach(() => vi.clearAllMocks());

describe('SystemSkills', () => {
  it('shows Luna’s own with its version and short sha, and nothing to compare or go back from', () => {
    render(<SystemSkills o={overview(state({}))} act={vi.fn()} edit={vi.fn()} />);
    expect(screen.getByText('Luna’s own: 0.12.0, sha 3054a9e14c77')).toBeInTheDocument();
    expect(screen.getByText(/never put into an account or a folder/)).toBeInTheDocument();
    expect(button('Compare with Luna’s')).toBeDisabled();
    expect(button('Back to Luna’s')).toBeDisabled();
    expect(button('Edit')).toBeEnabled();
    // No reach, audience or folder to pick for it.
    expect(screen.queryByRole('combobox')).toBeNull();
    expect(screen.queryByRole('radio')).toBeNull();
  });

  it('says whose text is served in each state', () => {
    expect(stateWords(state({ state: 'library', sha256: 'abcdef0123456789' }))).toBe(
      `Yours: ${PATH}, sha abcdef012345`
    );
    expect(stateWords(state({ state: 'broken', problem: 'The front matter has no description' }))).toBe(
      'Yours is broken: The front matter has no description. Luna’s own is used (0.12.0, sha 3054a9e14c77)'
    );
    render(
      <SystemSkills
        o={overview(state({ state: 'broken', problem: 'SKILL.md is over 64 KB' }))}
        act={vi.fn()}
        edit={vi.fn()}
      />
    );
    expect(screen.getByText(/Yours is broken: SKILL.md is over 64 KB\. Luna’s own is used/)).toBeInTheDocument();
    expect(button('Compare with Luna’s')).toBeEnabled();
    expect(button('Back to Luna’s')).toBeEnabled();
  });

  it('names a former override kept aside, which nothing reads', () => {
    const kept = 'C:\\lib\\system\\.session-handoff.leaving-ab12';
    render(<SystemSkills o={{ ...overview(state({})), systemAside: [kept] }} act={vi.fn()} edit={vi.fn()} />);
    expect(screen.getByRole('alert')).toHaveTextContent(`Kept aside, not served: ${kept}`);
    expect(screen.getByText('Luna’s own: 0.12.0, sha 3054a9e14c77')).toBeInTheDocument();
  });

  it('opens the editor on the system skill', async () => {
    const edit = vi.fn();
    render(<SystemSkills o={overview(state({}))} act={vi.fn()} edit={edit} />);
    await userEvent.click(button('Edit'));
    expect(edit).toHaveBeenCalledWith({ system: 'session-handoff' });
  });

  it('compares the override with Luna’s own text', async () => {
    vi.mocked(ipc.librarySystemCompare).mockResolvedValue({ luna: 'a\nb\n', yours: 'a\nmine\n' });
    render(<SystemSkills o={overview(state({ state: 'library' }))} act={vi.fn()} edit={vi.fn()} />);
    await userEvent.click(button('Compare with Luna’s'));
    expect(ipc.librarySystemCompare).toHaveBeenCalledWith('session-handoff');
    expect(await screen.findByText('Luna’s own → yours')).toBeInTheDocument();
    expect(screen.getByText('mine')).toBeInTheDocument();
    await userEvent.click(button('Hide comparison'));
    expect(screen.queryByText('Luna’s own → yours')).toBeNull();
  });

  it('compares again with each new look at the library, though the row says the same', async () => {
    vi.mocked(ipc.librarySystemCompare)
      .mockReset()
      .mockResolvedValueOnce({ luna: 'a\n', yours: 'a\nbefore\n' })
      .mockResolvedValue({ luna: 'a\n', yours: 'a\nafter\n' });
    const broken = state({ state: 'broken', problem: 'The front matter has no description' });
    render(<SystemSkills o={overview(broken)} act={vi.fn()} edit={vi.fn()} />);
    await userEvent.click(button('Compare with Luna’s'));
    expect(await screen.findByText('before')).toBeInTheDocument();
    // Edited elsewhere with the same fault, or another library with one: a new look, the same state.
    act(() => useLibrary.setState({ at: useLibrary.getState().at + 1 }));
    expect(await screen.findByText('after')).toBeInTheDocument();
    expect(ipc.librarySystemCompare).toHaveBeenCalledTimes(2);
  });

  it('goes back to Luna’s own only once confirmed', async () => {
    const act = vi.fn((p: Promise<unknown>) => p.then(() => true));
    render(<SystemSkills o={overview(state({ state: 'library' }))} act={act} edit={vi.fn()} />);
    await userEvent.click(button('Back to Luna’s'));
    expect(screen.getByRole('dialog')).toHaveTextContent('into Luna’s backups, kept for 30 days');
    expect(ipc.librarySystemReset).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(ipc.librarySystemReset).not.toHaveBeenCalled();
    await userEvent.click(button('Back to Luna’s'));
    await userEvent.click(screen.getAllByRole('button', { name: 'Back to Luna’s' }).at(-1)!);
    expect(ipc.librarySystemReset).toHaveBeenCalledWith('session-handoff');
    expect(act).toHaveBeenCalledTimes(1);
  });
});

describe('a skill of a system skill’s name', () => {
  it('is said to be laid out nowhere, with no columns to pick, and made the override once confirmed', async () => {
    const act = vi.fn((p: Promise<unknown>) => p.then(() => true));
    const other: LibrarySkillDto = { ...reserved, name: 'review', problem: null, description: 'Reviews.' };
    render(<SkillsGrid o={overview(state({}), [other, reserved])} act={act} edit={vi.fn()} remove={vi.fn()} />);
    const row = screen.getByText(/no skill of that name is laid out/).closest('tr')!;
    expect(row).toHaveAttribute('data-reserved', 'yes');
    // Its own row has no reach or audience picker; review's has them.
    expect(row.querySelectorAll('[role="radio"]')).toHaveLength(0);
    expect(screen.getAllByRole('radio')).toHaveLength(3);
    await userEvent.click(button('Make it the override…'));
    const dialog = screen.getByRole('dialog');
    expect(dialog).toHaveTextContent('skills/session-handoff');
    expect(dialog).not.toHaveTextContent('backups');
    await userEvent.click(screen.getByRole('button', { name: 'Make it the override' }));
    expect(ipc.librarySystemAdopt).toHaveBeenCalledWith('session-handoff');
    expect(act).toHaveBeenCalledTimes(1);
  });

  it('says an override already there goes to the backups', async () => {
    render(
      <SkillsGrid o={overview(state({ state: 'library' }), [reserved])} act={vi.fn()} edit={vi.fn()} remove={vi.fn()} />
    );
    await userEvent.click(button('Make it the override…'));
    expect(screen.getByRole('dialog')).toHaveTextContent('The override there now goes to Luna’s backups first');
    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(ipc.librarySystemAdopt).not.toHaveBeenCalled();
  });
});

describe('the editor on a system skill', () => {
  it('starts from Luna’s own text, checks it as Luna does and saves it as the override', async () => {
    vi.mocked(ipc.libraryRead).mockResolvedValue({
      path: PATH,
      text: '---\nname: session-handoff\n---\n',
      exists: false
    });
    vi.mocked(ipc.libraryCheckSystem).mockResolvedValue('The front matter has no description');
    vi.mocked(ipc.libraryWrite).mockResolvedValue(overview(state({ state: 'library' })));
    render(
      <div data-app>
        <LibraryEditor entry={{ system: 'session-handoff' }} onClose={vi.fn()} />
      </div>
    );
    expect(await screen.findByText(/Luna’s own text to start from: saving makes it yours/)).toBeInTheDocument();
    expect(ipc.libraryRead).toHaveBeenCalledWith({ system: 'session-handoff' });
    expect(screen.getByText('SKILL.md — session-handoff, system skill')).toBeInTheDocument();
    await waitFor(() =>
      expect(ipc.libraryCheckSystem).toHaveBeenCalledWith('session-handoff', '---\nname: session-handoff\n---\n')
    );
    expect(ipc.libraryCheckSkill).not.toHaveBeenCalled();
    expect(await screen.findByText(/Luna serves its own until it is fixed/)).toBeInTheDocument();

    const view = EditorView.findFromDOM(document.querySelector<HTMLElement>('.cm-editor')!)!;
    view.dispatch({ changes: { from: view.state.doc.length, insert: 'Mine.\n' } });
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(ipc.libraryWrite).toHaveBeenCalledWith(
      { system: 'session-handoff' },
      '---\nname: session-handoff\n---\nMine.\n'
    );
  });
});
