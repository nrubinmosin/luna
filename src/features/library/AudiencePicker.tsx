import { librarySetAudience, type Audience, type LibraryEntry, type LibraryOverviewDto } from '../../ipc/library';
import { Segmented } from '../../shared/ui/Segmented';

const AUDIENCES = ['both', 'chats', 'helpers'] as const;

/** The column's heading, over a picker per row. */
export const AUDIENCE_TITLE = 'Whose sessions each is for — your own chats, the helpers other sessions start, or both';
const LABELS: Record<Audience, string> = { both: 'Both', chats: 'Chats', helpers: 'Helpers' };

/** What each choice does to a session of the other kind (library/audience.rs). Codex has no way to leave its
 *  account's AGENTS.md out of one session: instructions say there whose they are instead. */
function hints(what: string, instructions: boolean): Record<Audience, string> {
  const worded = (whose: string) =>
    instructions ? `\nThe copies say they are for ${whose} only, as Codex reads its AGENTS.md in every session` : '';
  return {
    both: `Your own chats and the helpers other sessions start both get ${what}`,
    chats: `Only your own chats get ${what}: Luna keeps it from helpers${worded('your chats')}`,
    helpers: `Only the helpers other sessions start get ${what}: Luna keeps it from your own chats${worded('helpers')}`
  };
}

/**
 * Whose sessions a skill or instructions file is for: the person's own chats,
 * the helpers other sessions start, or both. A helper is briefed by an agent,
 * not the person, so what is written for the person's chat only costs it
 * context.
 */
export function AudiencePicker({
  entry,
  audience,
  act
}: {
  entry: LibraryEntry;
  audience: Audience;
  act: (p: Promise<LibraryOverviewDto | null>) => Promise<boolean>;
}) {
  const skill = 'skill' in entry;
  const what = 'skill' in entry ? `the ${entry.skill} skill` : `the ${entry.instructions} instructions`;
  return (
    <div className="library-audience">
      <Segmented
        items={AUDIENCES}
        value={audience}
        labels={LABELS}
        hints={hints(what, !skill)}
        height={20}
        onPick={v => {
          if (v !== audience) void act(librarySetAudience(entry, v));
        }}
      />
    </div>
  );
}
