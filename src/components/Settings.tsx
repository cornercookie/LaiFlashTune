import { useState, useEffect, useRef } from 'react';
import type { Song } from '../types';
import { addMusicFolder, listMusicFolders, scanFolders } from '../lib/api';
import { onScanProgress } from '../lib/api';

interface SettingsProps {
  onFoldersChange: (folders: string[]) => void;
}

export function Settings({ onFoldersChange }: SettingsProps) {
  const [folders, setFolders] = useState<string[]>([]);
  const [newFolder, setNewFolder] = useState('');
  const [scan, setScan] = useState<{ scanned: number; total: number; current_file: string; stage: string } | null>(null);
  const unlistenRef = useRef<(() => void) | null>(null);

  useEffect(() => {
    let mounted = true;
    (async () => {
      try {
        const fs = await listMusicFolders();
        if (mounted) {
          setFolders(fs.map((f) => f.path));
          onFoldersChange(fs.map((f) => f.path));
        }
      } catch {}
    })();
    (async () => {
      const un = await onScanProgress((p) => {
        if (!mounted) return;
        setScan({ scanned: p.scanned, total: p.total, current_file: p.current_file, stage: p.stage });
        if (p.stage === 'finished') {
          onFoldersChange(folders);
        }
      });
      unlistenRef.current = un;
    })();
    return () => {
      mounted = false;
      unlistenRef.current?.();
    };
  }, [folders, onFoldersChange]);

  const addFolder = async () => {
    if (!newFolder.trim()) return;
    try {
      await addMusicFolder({ path: newFolder.trim() });
      const fs = await listMusicFolders();
      const paths = fs.map((f) => f.path);
      setFolders(paths);
      onFoldersChange(paths);
      setNewFolder('');
    } catch {}
  };

  const startScan = async () => {
    if (folders.length === 0) return;
    setScan({ scanned: 0, total: 0, current_file: '開始掃描...', stage: 'scanning' });
    try { await scanFolders(folders); } catch {}
  };

  return (
    <div className="p-6 space-y-6 max-w-md w-full">
      <div>
        <h2 className="text-lg font-bold text-white mb-3">音樂資料夾</h2>
        <div className="flex gap-2 mb-3">
          <input
            className="flex-1 bg-[#0a0a0f] border border-[#252733] rounded-xl px-4 py-2.5 text-sm text-white focus:outline-none focus:ring-2 focus:ring-[#3b82f6] focus:border-transparent transition-all placeholder-[#6b7280]"
            placeholder="例如 D:\\Music"
            value={newFolder}
            onChange={(e) => setNewFolder(e.target.value)}
          />
          <button
            onClick={addFolder}
            className="w-10 h-10 rounded-xl bg-[#3b82f6] text-white hover:bg-[#2563eb] transition-colors flex items-center justify-center"
          >
            ＋
          </button>
        </div>
        <ul className="space-y-1 text-sm">
          {folders.map((f) => (
            <li key={f} className="flex items-center justify-between px-3 py-2 rounded-lg bg-[#1b1c25]/50 truncate">
              <span className="max-w-[200px] truncate text-[#9aa3ad]">{f}</span>
              <span className="text-[10px] text-[#6b7280]">{f.charAt(0)}:</span>
            </li>
          ))}
        </ul>
        <button
          onClick={startScan}
          disabled={folders.length === 0}
          className="w-full py-3 rounded-xl bg-[#3b82f6] text-white font-medium hover:bg-[#2563eb] disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
        >
          立即掃描音樂庫
        </button>
      </div>
      {scan && (
        <div className="bg-[#13141b] rounded-xl p-4 border border-[#252733]">
          <div className="flex justify-between text-[11px] mb-1">
            <span className="text-[#6b7280]">進度</span>
            <span className="text-white">{scan.scanned} / {scan.total}</span>
          </div>
          <div className="w-full bg-[#1b1c25] h-1.5 rounded-full overflow-hidden mb-2">
            <div className="h-full bg-[#3b82f6] rounded-full" style={{ width: `${scan.total ? Math.min(100, (scan.scanned / scan.total) * 100) : 0}%` }} />
          </div>
          <div className="text-[10px] text-[#6b7280] mt-1">{scan.current_file}</div>
          <div className="text-[10px] text-[#6b7280] mt-1">階段：{scan.stage}</div>
        </div>
      )}
      <div>
        <h2 className="text-lg font-bold text-white mb-3">外觀</h2>
        <div className="flex items-center gap-3">
          <label className="flex items-center gap-2 text-sm text-[#9aa3ad] cursor-pointer select-none">
            <input type="checkbox" className="rounded text-[#3b82f6] focus:ring-[#3b82f6]" /> 深色模式
          </label>
        </div>
      </div>
    </div>
  );
}
