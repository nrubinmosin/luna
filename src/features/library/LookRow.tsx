import { useState } from 'react';
import { libraryResolve, librarySkip, type LibraryOverviewDto, type PlaceDto } from '../../ipc/library';
import { TargetLabel } from './cells';
import { sayStatus, what } from './places';
import { RED } from './presentation';
import { DiffView } from './DiffView';

/** One copy that waits on the user, with what can be done about it; `count`
 *  copies when they fail for one reason, which is fixed in the library. */
export function LookRow({
  place,
  count,
  act
}: {
  place: PlaceDto;
  count: number;
  act: (p: Promise<LibraryOverviewDto>) => void;
}) {
  const [showDiff, setShowDiff] = useState(false);
  const differs = place.status === 'changed' || place.status === 'taken';
  return (
    <div style={{ borderTop: '1px solid var(--line)', padding: '6px 0' }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, flexWrap: 'wrap' }}>
        {count > 1 ? (
          <span style={{ whiteSpace: 'nowrap' }}>{count} places</span>
        ) : (
          <TargetLabel target={place.target} />
        )}
        <b>{what(place)}</b>
        <span
          style={{
            flex: 1,
            minWidth: 160,
            fontSize: 'var(--fs-2)',
            color: place.status === 'failed' ? RED : 'var(--dim)'
          }}
          title={place.dest}
        >
          {sayStatus(place)}
        </span>
        {differs && (
          <button className="slim" onClick={() => setShowDiff(!showDiff)}>
            {showDiff ? 'Hide differences' : 'Differences'}
          </button>
        )}
        {differs && (
          <button
            className="slim"
            title="This copy becomes the library's, and so every other copy's"
            onClick={() => act(libraryResolve(place.dest, true))}
          >
            Take into library
          </button>
        )}
        {(differs || place.status === 'gone') && (
          <button
            className="slim"
            title="The library's copy goes there; what is there now is backed up first"
            onClick={() => act(libraryResolve(place.dest, false))}
          >
            {place.status === 'gone' ? 'Put back' : place.status === 'taken' ? 'Replace' : 'Put library’s back'}
          </button>
        )}
        {count === 1 && (
          <button
            className="slim"
            title="Stop laying it out there; whatever is there stays as it is"
            onClick={() => act(librarySkip(place.dest, true))}
          >
            Leave out
          </button>
        )}
      </div>
      {showDiff && <DiffView dest={place.dest} />}
    </div>
  );
}
