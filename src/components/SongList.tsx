import { useRef } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { Song } from '../types';
import { SongRow } from './SongRow';

interface SongListProps {
  songs: Song[];
  activeId: number | null;
  onSelect: (song: Song) => void;
  onPlay: (song: Song) => void;
}

export function SongList({ songs, activeId, onSelect, onPlay }: SongListProps) {
  const scrollEl = useRef<HTMLDivElement>(null);
  const rowVirtualizer = useVirtualizer({
    count: songs.length,
    getScrollElement: () => scrollEl.current,
    estimateSize: () => 56,
    overscan: 10,
  });

  const handleSelect = (song: Song) => { onSelect(song); };

  return (
    <div className="h-full overflow-hidden flex flex-col">
      <div ref={scrollEl} className="flex-1 overflow-y-auto">
        <div
          ref={rowVirtualizer.containerRef}
          className="w-full"
          style={{
            height: rowVirtualizer.getTotalSize(),
            position: 'relative',
            width: '100%',
          }}
        >
          {rowVirtualizer.getVirtualItems().map((virtualRow) => {
            const song = songs[virtualRow.index];
            return (
              <div
                key={song.id}
                ref={rowVirtualizer.measureElement}
                style={{
                  position: 'absolute',
                  top: 0,
                  left: 0,
                  width: '100%',
                  height: virtualRow.size,
                  transform: `translateY(${virtualRow.start}px)`,
                }}
                className="flex items-center"
              >
                <SongRow
                  song={song}
                  activeId={activeId}
                  onClick={handleSelect}
                  onDblClick={onPlay}
                />
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
