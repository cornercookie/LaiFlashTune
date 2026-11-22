import type { Song, PlayMode } from '../types';
import {
  playerToggle, playerPrev, playerNext, playerSeek, playerSetVolume, playerSetMode,
} from '../lib/api';

interface PlayerBarProps {
  song: Song | null;
  isPlaying: boolean;
  position: number;
  duration: number;
  volume: number;
  mode: PlayMode;
  onToggle: () => void;
  onPrev: () => void;
  onNext: () => void;
  onSeek: (ms: number) => void;
  onSetVolume: (v: number) => void;
  onSetMode: (m: PlayMode) => void;
  onSongFinished: () => void;
}

function formatTime(ms: number): string {
  if (!isFinite(ms) || ms < 0) return '0:00';
  const totalSeconds = Math.floor(ms / 1000);
  const m = Math.floor(totalSeconds / 60);
  const s = totalSeconds % 60;
  return `${m}:${String(s).padStart(2, '0')}`;
}

export function PlayerBar({
  song, isPlaying, position, duration, volume, mode,
  onToggle, onPrev, onNext, onSeek, onSetVolume, onSetMode,
}: PlayerBarProps) {
  const title = song?.title || '未選擇歌曲';
  const artist = song?.artist || '';
  const currentTime = formatTime(position);
  const totalTime = formatTime(duration);
  const pct = duration > 0 ? Math.min(100, (position / duration) * 100) : 0;

  const handleMode = () => {
    const next: PlayMode =
      mode === 'Sequence' ? 'All' : mode === 'All' ? 'Random' : mode === 'Random' ? 'Single' : 'Sequence';
    onSetMode(next);
  };

  return (
    <div className="h-16 border-t border-[#252733] bg-[#0a0a0f] flex items-center gap-3 px-4 select-none z-20">
      <button onClick={onPrev} className="w-8 h-8 rounded-full text-[#9aa3ad] hover:text-white hover:bg-[#1b1c25] transition-colors flex items-center justify-center">⏮</button>
      <button onClick={onToggle} className="w-10 h-10 rounded-full flex items-center justify-center text-2xl bg-[#3b82f6] text-white shadow-[0_0_20px_rgba(59,130,246,0.4)]">
        {isPlaying ? '⏸' : '▶'}
      </button>
      <button onClick={onNext} className="w-8 h-8 rounded-full text-[#9aa3ad] hover:text-white hover:bg-[#1b1c25] transition-colors flex items-center justify-center">⏭</button>

      <div className="flex-1 min-w-0 flex items-center gap-3">
        {song?.cover_path ? (
          <img src={song.cover_path} className="w-10 h-10 rounded-lg object-cover border border-[#252733] shadow-lg" alt="cover" />
        ) : (
          <div className="w-10 h-10 rounded-lg bg-[#1b1c25] flex items-center justify-center text-xs border border-[#252733]">🎵</div>
        )}
        <div className="min-w-0 flex flex-col">
          <div className="text-sm font-medium text-white truncate" title={title}>{title}</div>
          <div className="text-xs text-[#9aa3ad] truncate" title={artist}>{artist}</div>
        </div>
      </div>

      <div className="w-48 flex flex-col items-center gap-1">
        <div className="flex gap-1 text-[10px] text-[#6b7280] tabular-nums font-mono">
          <span>{currentTime}</span> <span>/</span> <span>{totalTime}</span>
        </div>
        <input
          type="range"
          min="0"
          max={Math.max(duration, 1)}
          value={position}
          onChange={(e) => onSeek(Number(e.target.value))}
          className="w-full accent-[#3b82f6] cursor-pointer"
          style={{ WebkitAppearance: 'none' }}
        />
        <div className="w-full h-[2px] bg-[#3b82f6]/30 relative overflow-hidden">
          <div className="h-full bg-[#3b82f6]" style={{ width: `${pct}%` }} />
        </div>
      </div>

      <div className="flex items-center gap-2">
        <button onClick={handleMode} className="w-8 h-8 rounded-full text-[#9aa3ad] hover:text-[#3b82f6] hover:bg-[#1b1c25] transition-colors flex items-center justify-center">
          {mode === 'Sequence' ? '🔁' : mode === 'All' ? '🔁🔁' : mode === 'Random' ? '🔀' : '🔂'}
        </button>
        <input
          type="range"
          min="0" max="1" step="0.05"
          value={volume}
          onChange={(e) => onSetVolume(Number(e.target.value))}
          className="w-24 accent-[#3b82f6] cursor-pointer"
          style={{ WebkitAppearance: 'none' }}
        />
      </div>
    </div>
  );
}
