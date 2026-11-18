import type { ChatStatus } from '../types';
import { STATUS } from './status';

/**
 * The chat's state, worn as a dot. Busy states have to be readable from the
 * far corner of a four-pane grid, so they get size, a white ring and a halo
 * of their own color; idle stays a quiet gray so the loud ones can be loud.
 * A hollow state (background) is a ring of its color around what lies under
 * it.
 */
export function StatusDot({ status }: { status: ChatStatus }) {
  const st = STATUS[status];
  const busy = status !== 'resting';
  return (
    <span
      title={st.label}
      data-shape={st.hollow ? 'ring' : 'dot'}
      style={{
        width: 10,
        height: 10,
        borderRadius: '50%',
        flex: 'none',
        background: st.hollow ? 'transparent' : st.color,
        // The ring separates the dot from whatever color the title bar
        // wears; the halo is what makes it findable without looking straight
        // at it.
        boxShadow: busy
          ? `${st.hollow ? `inset 0 0 0 3px ${st.color}, ` : ''}0 0 0 1.5px rgba(255,255,255,.85), 0 0 8px 1.5px ${st.color}`
          : 'none'
      }}
    />
  );
}
