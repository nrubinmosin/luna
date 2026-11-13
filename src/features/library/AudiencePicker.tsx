import {
  librarySetAudience,
  type Audience,
  type LibraryEntry,
  type LibraryOverviewDto,
  type Role
} from '../../ipc/library';

const ROLES: readonly Role[] = ['chat', 'lead', 'helper'];

/** The column's heading, over a picker per row. */
export const AUDIENCE_TITLE =
  'Whose sessions each is for — your own chats, leads (helpers started with Luna’s tools), helpers — every one ticked: all of them';
const LABELS: Record<Role, string> = { chat: 'Chats', lead: 'Leads', helper: 'Helpers' };
const WHOSE: Record<Role, string> = {
  chat: 'your own chats',
  lead: 'leads, the helpers started with Luna’s tools',
  helper: 'helpers started without them'
};

/** The roles an audience takes. */
export function rolesOf(audience: Audience): Role[] {
  return audience === 'every' ? [...ROLES] : audience;
}

/** The audience of these roles: every session's when it takes them all. */
export function audienceOf(roles: readonly Role[]): Audience {
  const kept = ROLES.filter(r => roles.includes(r));
  return kept.length === ROLES.length ? 'every' : kept;
}

/** What ticking a role does (library/audience.rs). An entry for every session goes into the accounts it is for;
 *  one for some roles only reaches their sessions as Luna starts them, never through an account. */
function hint(role: Role, on: boolean, last: boolean, instructions: boolean): string {
  if (on && last) return `Only ${WHOSE[role]} get it: tick another role to add it`;
  const how = instructions
    ? '\nFor some roles only, it is given to each such session as Luna starts it, never put into an account'
    : '';
  return on ? `Keep it from ${WHOSE[role]}${how}` : `Give it to ${WHOSE[role]} too${how}`;
}

/**
 * Whose sessions a skill or instructions file is for: the person's own chats, leads, helpers, or every session.
 * A session briefed by an agent, not the person, gets nothing from what is written for the person's chat but
 * cost and confusion; a lead needs what a helper without tools does not.
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
  const instructions = !('skill' in entry);
  const on = rolesOf(audience);
  const flip = (role: Role) => {
    const next = on.includes(role) ? on.filter(r => r !== role) : [...on, role];
    // Nobody's would be an entry no session gets.
    if (next.length === 0) return;
    void act(librarySetAudience(entry, audienceOf(next)));
  };
  return (
    <div className="library-audience">
      <div className="seg" role="group" aria-label="Whose sessions it is for">
        {ROLES.map(role => {
          const checked = on.includes(role);
          return (
            <div
              key={role}
              role="checkbox"
              aria-checked={checked}
              title={hint(role, checked, checked && on.length === 1, instructions)}
              tabIndex={0}
              onClick={() => flip(role)}
              onKeyDown={e => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.preventDefault();
                  flip(role);
                }
              }}
              style={{ height: 20 }}
            >
              {LABELS[role]}
            </div>
          );
        })}
      </div>
    </div>
  );
}
