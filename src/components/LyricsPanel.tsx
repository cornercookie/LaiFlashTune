import { useState, useEffect, useRef } from 'react';
import type { LyricLine, Song } from '../types';
import { getLyrics, setLyricsOffset } from '../lib/api';

interface LyricsPanelProps {
  song: Song | null;
  positionMs: number;
  durationMs: number;
  onSeek: (ms: number) => void;
  collapsed: boolean;
  onToggleCollapse: () => void;
}

export function LyricsPanel({
  song, positionMs, durationMs, onSeek, collapsed, onToggleCollapse,
}: LyricsPanelProps) {
  const [lines, setLines] = useState<LyricLine[]>([]);
  const [activeIndex, setActiveIndex] = useState(-1);
  const [offset, setOffset] = useState(0);
  const [source, setSource] = useState<string | null>(null);
  const raf = useRef<number>();

  useEffect(() => {
    if (!song) { setLines([]); setActiveIndex(-1); setSource(null); return; }
    (async () => {
      try {
        const data = await getLyrics(song.id);
        if (data) {
          setLines(data.lines);
          setOffset(data.offset_ms ?? 0);
          setSource(data.source ?? null);
        }
      } catch {}
    })();
  }, [song?.id]);

  useEffect(() => {
    if (!lines.length) return;
    const tick = () => {
      const pos = positionMs + offset;
      let lo = 0, hi = lines.length - 1, idx = -1;
      while (lo <= hi) {
        const mid = (lo + hi) >> 1;
        if (lines[mid].time_ms <= pos) { idx = mid; lo = mid + 1; }
        else { hi = mid - 1; }
      }
      setActiveIndex(idx);
      raf.current = requestAnimationFrame(tick);
    };
    tick();
    return () => { if (raf.current) cancelAnimationFrame(raf.current); };
  }, [positionMs, offset, lines]);

  const handleSeekLine = (timeMs: number) => { onSeek(timeMs - offset); };

  const adjustOffset = (delta: number) => {
    const nv = offset + delta;
    setOffset(nv);
    if (song) setLyricsOffset(song.id, nv);
  };

  if (collapsed) {
    return (
      <button
        onClick={onToggleCollapse}
        className="w-full py-3 text-xs text-[#6b7280] hover:text-white hover:bg-[#1b1c25] transition-colors border-t border-[#252733]"
      >
        展開歌詞
      </button>
    );
  }

  return (
    <div className="flex flex-col h-full border-l border-[#252733] bg-[#13141b]/80 backdrop-blur-sm">
      <div className="px-4 py-3 flex items-center justify-between border-b border-[#252733]">
        <div className="flex items-center gap-2">
          <div className="w-8 h-8 rounded-lg bg-[#1b1c25] flex items-center justify-center text-xs">📝</div>
          <div className="flex-1 min-w-0">
            <div className="text-sm font-medium text-white">{song?.title || '暫無歌曲'}</div>
            <div className="text-[10px] text-[#6b7280]">{song?.artist || ''}</div>
          </div>
          <div className="text-[10px] text-[#6b7280] ml-2">{source ? `(${source})` : '無歌詞'}</div>
        </div>
        <button onClick={onToggleCollapse} className="text-[10px] text-[#6b7280] hover:text-white">
          摺疊
        </button>
      </div>
      <div className="flex-1 overflow-y-auto px-4 py-2 space-y-1">
        {lines.length === 0 ? (
          <div className="text-center py-10 text-[#6b7280] text-sm">暫無歌詞</div>
        ) : (
          lines.map((line, i) => {
            const active = i === activeIndex;
            return (
              <div
                key={i}
                onClick={() => handleSeekLine(line.time_ms)}
                className={`cursor-pointer select-none px-2 py-1.5 rounded-lg transition-all ${
                  active
                    ? 'text-xl text-[#3b82f6] shadow-[0_0_15px_rgba(59,130,246,0.35)]'
                    : 'text-sm text-[#6b7280] hover:text-white'
                }`}
              >
                {line.text}
              </div>
            );
          })
        )}
      </div>
      <div className="px-4 py-2 border-t border-[#252733] flex gap-2">
        <button
          onClick={() => adjustOffset(-500)}
          className="flex-1 px-2 py-1.5 rounded text-[10px] text-[#6b7280] hover:text-white bg-[#1b1c25]"
        >
          −0.5s
        </button>
        <button
          onClick={() => adjustOffset(0)}
          className="px-2 py-1.5 rounded text-[10px] text-[#6b7280] hover:text-white bg-[#1b1c25]"
        >
          重置
        </button>
        <button
          onClick={() => adjustOffset(500)}
          className="flex-1 px-2 py-1.5 rounded text-[10px] text-[#6b7280] hover:text-white bg-[#1b1c25]"
        >
          +0.5s
        </button>
      </div>
    </div>
  );
}
