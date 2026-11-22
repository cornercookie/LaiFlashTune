import { useEffect, useRef, useState } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import type { Song } from '../types';

export type SortKey = 'title' | 'artist' | 'album' | 'duration_ms' | 'play_count' | 'file_mtime';

function fmtDate(ms?: number | null) {
  if (!ms) return '—';
  const d = new Date(ms);
  if (Number.isNaN(d.getTime())) return '—';
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
}

function fmt(ms: number) {
  if (!isFinite(ms) || ms < 0) return '0:00';
  const s = Math.floor(ms / 1000);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
}

export function TrackTable({
  songs, activeId, playing, selected, sortKey, sortDir, onSort, onSelect, onPlay, onContext, onNearEnd,
  scrollNonce = 0, onReorder,
}: {
  songs: Song[];
  activeId: number | null;
  playing: boolean;
  selected: number[];
  sortKey: SortKey;
  sortDir: 'asc' | 'desc';
  onSort: (k: SortKey) => void;
  onSelect: (song: Song, e: React.MouseEvent) => void;
  onPlay: (song: Song) => void;
  onContext: (song: Song, x: number, y: number) => void;
  onNearEnd: () => void;
  scrollNonce?: number;
  onReorder?: (from: number, to: number) => void;
}) {
  const scroller = useRef<HTMLDivElement>(null);
  const [over, setOver] = useState<number | null>(null);
  const virtualizer = useVirtualizer({
    count: songs.length,
    getScrollElement: () => scroller.current,
    estimateSize: () => 30,
    overscan: 12,
  });

  useEffect(() => {
    if (!scrollNonce) return;
    const index = songs.findIndex((song) => song.id === activeId);
    if (index >= 0) virtualizer.scrollToIndex(index, { align: 'center' });
  }, [scrollNonce]);

  const head = (key: SortKey, label: string, align = 'left') => (
    <button type="button" onClick={() => onSort(key)} className={`text-left text-[11px] text-[#8a847a] ${align === 'right' ? 'text-right' : ''}`}>
      {label}{sortKey === key ? (sortDir === 'asc' ? ' ↑' : ' ↓') : ''}
    </button>
  );

  return (
    <div className="flex flex-col min-h-0 h-full">
      <div className="grid grid-cols-[42px_36px_minmax(0,1.4fr)_minmax(0,0.8fr)_minmax(0,0.8fr)_64px_48px_104px] pl-5 pr-6 h-8 items-center border-b border-[#2a2723]">
        <span className="text-[11px] text-[#8a847a]">No</span>
        <span className="text-[11px] text-[#8a847a]">軌</span>
        {head('title', '歌名')}
        {head('artist', '歌手')}
        {head('album', '專輯')}
        {head('duration_ms', '時間', 'right')}
        {head('play_count', '次數', 'right')}
        {head('file_mtime', '日期', 'right')}
      </div>
      <div
        ref={scroller}
        className="flex-1 overflow-y-auto"
        onScroll={() => {
          const el = scroller.current;
          if (el && el.scrollTop + el.clientHeight > el.scrollHeight - 240) onNearEnd();
        }}
      >
        <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
          {virtualizer.getVirtualItems().map((row) => {
            const song = songs[row.index];
            const on = song.id === activeId;
            const sel = selected.includes(song.id);
            return (
              <div
                key={`${song.id}-${row.index}`}
                draggable={!!onReorder}
                onDragStart={(e) => {
                  e.dataTransfer.effectAllowed = 'move';
                  e.dataTransfer.setData('text/plain', String(row.index));
                }}
                onDragOver={(e) => {
                  if (!onReorder) return;
                  e.preventDefault();
                  setOver(row.index);
                }}
                onDrop={(e) => {
                  if (!onReorder) return;
                  e.preventDefault();
                  const from = Number(e.dataTransfer.getData('text/plain'));
                  setOver(null);
                  if (!Number.isNaN(from) && from !== row.index) onReorder(from, row.index);
                }}
                onDragEnd={() => setOver(null)}
                onClick={(e) => onSelect(song, e)}
                onDoubleClick={() => onPlay(song)}
                onContextMenu={(e) => { e.preventDefault(); onContext(song, e.clientX, e.clientY); }}
                className={`grid grid-cols-[42px_36px_minmax(0,1.4fr)_minmax(0,0.8fr)_minmax(0,0.8fr)_64px_48px_104px] pl-5 pr-6 items-center text-sm border-b border-[#221f1c] cursor-default ${on ? 'bg-[#2a221c]' : ''} ${sel && !on ? 'bg-[#1a1816]' : ''} ${song.missing ? 'text-[#8a847a]' : ''} ${over === row.index ? 'border-t border-t-[#e23b2f]' : ''}`}
                style={{ position: 'absolute', top: 0, left: 0, width: '100%', height: row.size, transform: `translateY(${row.start}px)` }}
              >
                <span className={`catalog-live num text-[13px] ${on ? 'text-[#e23b2f]' : 'text-[#8a847a]'} ${on && playing ? 'is-on' : ''}`}>
                  {on && <span className="ink-pass" aria-hidden="true">{String(row.index + 1).padStart(2, '0')}</span>}
                  {String(row.index + 1).padStart(2, '0')}
                </span>
                <span className="num text-[12px] text-[#8a847a]">{song.track_number && song.track_number > 0 ? song.track_number : '—'}</span>
                <span className={`truncate pr-3 min-w-0 ${song.missing ? 'line-through text-[#8a847a]' : on ? 'text-[#f4f0e8]' : ''}`}>
                  {song.title || '未命名'}{song.missing ? '　缺檔' : ''}
                </span>
                <span className="truncate pr-2 text-[#c8c2b8]">{song.artist || '未知歌手'}</span>
                <span className="truncate pr-2 text-[#8a847a]">{song.album || '未知專輯'}</span>
                <span className="num text-right text-[#8a847a]">{fmt(song.duration_ms)}</span>
                <span className="num text-right text-[#8a847a]">{song.play_count}</span>
                <span className="num text-right text-[#8a847a] pr-1">{fmtDate(song.file_mtime)}</span>
              </div>
            );
          })}
        </div>
        {songs.length === 0 && <div className="px-[18px] py-10 text-sm text-[#8a847a]">這個清單是空的。</div>}
      </div>
    </div>
  );
}
