import { useState, useRef, useEffect, FormEvent } from 'react';
import { searchSongs } from '../lib/api';
import type { Song } from '../types';

export function SearchBar({ onResults, onOpenSong }: { onResults: (songs: Song[]) => void; onOpenSong: (song: Song) => void }) {
  const [query, setQuery] = useState('');
  const [results, setResults] = useState<Song[]>([]);
  const [loading, setLoading] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    if (timer.current) clearTimeout(timer.current);
    if (!query.trim()) {
      setResults([]);
      setLoading(false);
      return;
    }
    setLoading(true);
    timer.current = setTimeout(async () => {
      try {
        const { songs } = await searchSongs(query, 50);
        setResults(songs);
      } catch {
        setResults([]);
      } finally {
        setLoading(false);
      }
    }, 150);
    return () => { if (timer.current) clearTimeout(timer.current); };
  }, [query]);

  const handleSubmit = (e: FormEvent) => {
    e.preventDefault();
    if (results[0]) onOpenSong(results[0]);
  };

  return (
    <form onSubmit={handleSubmit} className="flex w-full items-end gap-2">
      <input
        type="text"
        className="w-full bg-[#0a0a0f] border border-[#252733] rounded-xl px-4 py-2.5 text-sm text-white focus:outline-none focus:ring-2 focus:ring-[#3b82f6] focus:border-transparent transition-all placeholder-[#6b7280]"
        placeholder="搜尋歌曲、歌手、專輯、拼音（如 zjl / 周杰倫）..."
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        autoComplete="off"
      />
      {loading && (
        <div className="animate-spin text-[#3b82f6]">載入中</div>
      )}
      <button
        type="button"
        onClick={() => results[0] && onOpenSong(results[0])}
        disabled={!results[0]}
        className="w-8 h-8 rounded-full flex items-center justify-center text-[#9aa3ad] hover:text-white hover:bg-[#1b1c25] disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
      >▶</button>
    </form>
  );
}
