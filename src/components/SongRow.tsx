import { Song } from '../types';

interface SongRowProps {
  song: Song;
  activeId: number | null;
  onClick: (song: Song) => void;
  onDblClick?: (song: Song) => void;
}

function formatDuration(ms: number): string {
  if (!isFinite(ms) || ms < 0) return '0:00';
  const totalSeconds = Math.floor(ms / 1000);
  const m = Math.floor(totalSeconds / 60);
  const s = totalSeconds % 60;
  return `${m}:${String(s).padStart(2, '0')}`;
}

export function SongRow({ song, activeId, onClick, onDblClick }: SongRowProps) {
  const isActive = activeId === song.id;
  const title = song.title || '-';
  const artist = song.artist || '未知歌手';
  const album = song.album || '未知專輯';

  return (
    <div
      className={`row ${isActive ? 'row.active' : ''}`}
      onClick={() => onClick(song)}
      onDoubleClick={onDblClick ? () => onDblClick!(song) : undefined}
      title={`${title} - ${artist}`}
    >
      <div className="flex-1 min-w-0">
        <div className="text-sm font-medium text-white truncate">{title}</div>
        <div className="text-xs text-[#9aa3ad] truncate">{artist}</div>
        <div className="text-[10px] text-[#6b7280] truncate">{album}</div>
      </div>
      <div className="text-[10px] text-[#6b7280] font-mono tabular-nums">{formatDuration(song.duration_ms)}</div>
      <div className="text-[10px] text-[#6b7280] tabular-nums">{song.play_count}</div>
    </div>
  );
}
