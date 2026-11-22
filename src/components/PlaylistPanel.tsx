import type { Playlist } from '../types';

interface PlaylistPanelProps {
  playlists: Playlist[];
  activePlaylistId: number | null;
  onSelect: (id: number) => void;
  onCreate: () => void;
}

export function PlaylistPanel({ playlists, activePlaylistId, onSelect, onCreate }: PlaylistPanelProps) {
  return (
    <div className="flex flex-col h-full gap-1 px-2 pb-2">
      <div className="flex items-center gap-2 px-3 py-2 text-[10px] font-semibold text-[#6b7280] uppercase tracking-wider">
        歌單
      </div>
      <div className="flex-1 overflow-y-auto space-y-0.5">
        {playlists.map((pl) => {
          const active = activePlaylistId === pl.id;
          return (
            <button
              key={pl.id}
              onClick={() => onSelect(pl.id)}
              className={`w-full flex items-center gap-3 px-3 py-2 rounded-lg text-sm transition-colors ${
                active
                  ? 'bg-[#3b82f6]/20 text-[#3b82f6] border border-[#3b82f6]/30'
                  : 'text-[#9aa3ad] hover:bg-[#1b1c25]/50'
              }`}
            >
              <div className="w-5 h-5 rounded flex items-center justify-center text-[10px]">
                {pl.is_smart === 1 ? '✦' : '☰'}
              </div>
              <div className="truncate flex-1">{pl.name}</div>
            </button>
          );
        })}
      </div>
      <button
        onClick={onCreate}
        className="w-full px-3 py-2 rounded-lg text-[10px] text-[#6b7280] hover:text-white hover:bg-[#1b1c25] transition-colors border border-[#252733]"
      >
        + 新增歌單
      </button>
    </div>
  );
}
