import { useEffect, useState } from 'react';
import type { Song, TagEdit } from '../types';
import {
  addMusicFolder, browseFolder, convertAudio, ffmpegAvailable, listMusicFolders, readTags,
  removeMusicFolder, scanFolders, writeTags, playerPlayUrl, playerSetCrossfade,
  playerSetEq, playerSetNormalize, playerSetPreamp,
  listRadio, saveRadio, renameRadio, deleteRadio,
} from '../lib/api';
import type { RadioStation } from '../types';

const HZ = ['32', '64', '125', '250', '500', '1k', '2k', '4k', '8k', '16k'];

export function EqPane({
  bands, preamp, normalize, crossfade, gain,
  presets, selectedPreset, presetDraft,
  onBands, onPreamp, onNormalize, onCrossfade,
  onApplyPreset, onSavePreset, onDeletePreset, onSelectPreset, onPresetNameChange,
}: {
  bands: number[];
  preamp: number;
  normalize: boolean;
  crossfade: number;
  gain: number;
  presets: Array<{ name: string; bands: number[] }>;
  selectedPreset: string | null;
  presetDraft: string;
  onBands: (b: number[]) => void;
  onPreamp: (v: number) => void;
  onNormalize: (v: boolean) => void;
  onCrossfade: (ms: number) => void;
  onApplyPreset: (name: string) => void;
  onSavePreset: () => void;
  onDeletePreset: (name: string) => void;
  onSelectPreset: (name: string | null) => void;
  onPresetNameChange: (v: string) => void;
}) {
  const set = (i: number, v: number) => {
    const next = bands.slice();
    next[i] = v;
    onBands(next);
    playerSetEq(next).catch(() => {});
  };
  const setPreamp = (v: number) => {
    onPreamp(v);
    playerSetPreamp(v).catch(() => {});
  };
  const applySelected = () => {
    if (selectedPreset) onApplyPreset(selectedPreset);
  };
  return (
    <div className="px-5 py-4 h-full overflow-y-auto overflow-x-hidden">
      <div className="text-[18px] font-bold mb-1 break-words">10 段等化器</div>
      <p className="text-sm text-[#8a847a] mb-4">只作用於本地檔案。直播串流不會套用。</p>
      <label className="block mt-2 text-sm">
        前級增益 {preamp >= 0 ? '+' : ''}${preamp.toFixed(0)} dB
        <input
          type="range" min={-12} max={12} step={1} value={preamp}
          onChange={(e) => setPreamp(Number(e.target.value))}
          className="w-full accent-[#e23b2f] mt-1"
        />
      </label>
      <div className="flex gap-2 items-end h-40">
        {HZ.map((label, i) => (
          <label key={label} className="flex flex-col items-center gap-1 flex-1">
            <span className="num text-[10px] text-[#e23b2f]">{bands[i]?.toFixed(0)}</span>
            <input
              aria-label={`${label} Hz`}
              type="range" min={-12} max={12} step={1}
              value={bands[i] ?? 0}
              onChange={(e) => set(i, Number(e.target.value))}
              className="accent-[#e23b2f] h-28"
              style={{ writingMode: 'vertical-lr', direction: 'rtl' }}
            />
            <span className="text-[10px] text-[#8a847a]">{label}</span>
          </label>
        ))}
      </div>
      <label className="flex items-center gap-2 mt-6 text-sm">
        <input type="checkbox" checked={normalize} onChange={(e) => { onNormalize(e.target.checked); playerSetNormalize(e.target.checked).catch(() => {}); }} />
        峰值正規化{normalize ? `（目前增益 ${gain.toFixed(2)}）` : ''}
      </label>
      <label className="block mt-4 text-sm">
        交叉淡化 {crossfade === 0 ? '關閉' : `${(crossfade / 1000).toFixed(1)} 秒`}
        <input
          type="range" min={0} max={8000} step={200} value={crossfade}
          onChange={(e) => { const ms = Number(e.target.value); onCrossfade(ms); playerSetCrossfade(ms).catch(() => {}); }}
          className="w-full accent-[#e23b2f] mt-1"
        />
      </label>
      <p className="text-xs text-[#8a847a] mt-2">交叉淡化需要下一首已預載，只作用於本地檔案。</p>
      <div className="mt-5 border-t border-[#2a2723] pt-3">
        <div className="text-[12px] tracking-widest text-[#8a847a] mb-2">等化器預設</div>
        <select
          className="field w-full mb-2"
          value={selectedPreset ?? ''}
          onChange={(e) => { onSelectPreset(e.target.value || null); }}
        >
          <option value="">-- 選擇預設 --</option>
          {presets.map((p) => (
            <option key={p.name} value={p.name}>{p.name}</option>
          ))}
        </select>
        <div className="flex gap-1">
          <button type="button" className="round w-auto px-3 text-xs shrink-0" onClick={applySelected} disabled={!selectedPreset}>套用</button>
          <button type="button" className="round w-auto px-3 text-xs shrink-0 text-[#e23b2f]" onClick={() => { if (selectedPreset) { onDeletePreset(selectedPreset); onSelectPreset(null); } }} disabled={!selectedPreset}>刪除</button>
        </div>
        <div className="mt-2 flex gap-1">
          <input className="field flex-1" placeholder="預設名稱" value={presetDraft} onChange={(e) => onPresetNameChange(e.target.value)} />
          <button type="button" className="round w-auto px-3 text-xs shrink-0" onClick={onSavePreset} disabled={!presetDraft.trim()}>儲存目前</button>
        </div>
        <p className="text-xs text-[#8a847a] mt-1">內建「平直」「人聲」「重低音」只是種子，使用者可覆寫、刪除，不宣稱這是 AIMP 預設。</p>
      </div>
    </div>
  );
}

export function RadioView({ onPlay }: { onPlay: (url: string) => void }) {
  const [stations, setStations] = useState<RadioStation[]>([]);
  const [name, setName] = useState('');
  const [url, setUrl] = useState('');
  const [err, setErr] = useState('');
  const [msg, setMsg] = useState('');
  const [editing, setEditing] = useState<RadioStation | null>(null);
  useEffect(() => { listRadio().then(setStations).catch(() => setStations([])); }, []);
  const clear = () => { setName(''); setUrl(''); setEditing(null); setErr(''); setMsg(''); };
  const deleteStation = async (s: RadioStation) => {
    if (!window.confirm('刪除「' + s.name + '」？')) return;
    try {
      await deleteRadio(s.id);
      setMsg('已刪除');
      clear();
      setStations(await listRadio());
    } catch (e) { setErr(e instanceof Error ? e.message : String(e)); }
  };
  return (
    <div className="p-6 max-w-xl">
      <h2 className="text-[26px] font-bold">網路電台</h2>
      <p className="text-sm text-[#8a847a] mt-2 mb-4">只有你自己存過的網址，沒有內建電台目錄。這台電腦要有 ffmpeg 才播得了。</p>
      <form className="space-y-2 mb-4" onSubmit={async (e) => {
        e.preventDefault();
        setErr(''); setMsg('');
        try {
          if (editing) {
            await renameRadio(editing.id, name);
            setMsg('已改名');
          } else {
            const id = await saveRadio(name, url);
            setMsg('已存入電台');
          }
          clear();
          setStations(await listRadio());
        } catch (e) { setErr(e instanceof Error ? e.message : String(e)); }
      }}>
        <input className="field" placeholder="電台名稱" value={name} onChange={(e) => setName(e.target.value)} disabled={!!editing} />
        <input className="field" placeholder="https://example.com/stream.mp3" value={url} onChange={(e) => setUrl(e.target.value)} />
        <button className="round w-auto px-4 text-sm" type="submit">{editing ? '儲存改名' : '存入電台'}</button>
        <button type="button" className="text-xs text-[#8a847a]" onClick={clear}>取消</button>
      </form>
      <ul className="space-y-1">
        {stations.map((s) => (
          <li key={s.id} className="flex items-center gap-2 border-b border-[#2a2723] py-2">
            <span className="truncate">{s.name}</span>
            <span className="text-xs text-[#6f6a63] truncate flex-1">{s.url}</span>
            <button className="round w-6 h-6 px-0" onClick={() => { onPlay(s.url); clear(); }} title="播放">▶</button>
            <button className="round w-6 h-6 px-0" onClick={() => { setEditing(s); setName(s.name); setUrl(s.url); }} title="改名">✎</button>
            <button className="round w-6 h-6 px-0 text-[#e23b2f]" onClick={() => { deleteStation(s); }} title="刪除">×</button>
          </li>
        ))}
        {stations.length === 0 && <li className="text-xs text-[#8a847a]">還沒有電台。貼上你自己的串流網址。</li>}
      </ul>
      <div className="mt-4 text-sm text-[#8a847a]">貼上 http / https 串流網址，存進來後就能在這裡播放。</div>
      {err && <p className="text-sm text-[#e23b2f] mt-3">{err}</p>}
      {msg && <p className="text-sm text-[#e23b2f] mt-3">{msg}</p>}
    </div>
  );
}

export function TagView({ song }: { song: Song | null }) {
  const [form, setForm] = useState<TagEdit | null>(null);
  const [msg, setMsg] = useState('');
  useEffect(() => {
    if (!song) { setForm(null); return; }
    readTags(song.id).then(setForm).catch((e) => setMsg(String(e)));
  }, [song?.id]);
  if (!song || !form) return <div className="p-6 text-sm text-[#8a847a]">先在清單選一首歌，再改標籤。</div>;
  const set = (k: keyof TagEdit, v: string) => setForm({ ...form, [k]: (k === 'year' || k === 'track_number' || k === 'disc_number') ? (v ? Number(v) : null) : v });
  return (
    <form className="p-6 max-w-xl space-y-3" onSubmit={async (e) => {
      e.preventDefault();
      setMsg('');
      try { await writeTags(form); setMsg('已寫入檔案與資料庫'); }
      catch (err) { setMsg(err instanceof Error ? err.message : String(err)); }
    }}>
      <h2 className="text-[26px] font-bold">標籤</h2>
      <p className="text-sm text-[#8a847a]">{song.path}</p>
      {(['title', 'artist', 'album', 'genre'] as const).map((k) => (
        <label key={k} className="block text-sm">{k === 'title' ? '歌名' : k === 'artist' ? '歌手' : k === 'album' ? '專輯' : '曲風'}
          <input className="field mt-1" value={String(form[k] ?? '')} onChange={(e) => set(k, e.target.value)} />
        </label>
      ))}
      <label className="block text-sm">年份
        <input className="field mt-1" value={form.year ?? ''} onChange={(e) => set('year', e.target.value)} />
      </label>
      <label className="block text-sm">音軌號
        <input className="field mt-1" value={form.track_number ?? ''} onChange={(e) => set('track_number', e.target.value)} />
      </label>
      <label className="block text-sm">碟號
        <input className="field mt-1" value={form.disc_number ?? ''} onChange={(e) => set('disc_number', e.target.value)} />
      </label>
      <button className="round w-auto px-4 text-sm" type="submit">寫入標籤</button>
      {msg && <p className="text-sm text-[#e23b2f]">{msg}</p>}
    </form>
  );
}

export function ConvertView({ song }: { song: Song | null }) {
  const [ok, setOk] = useState<boolean | null>(null);
  const [format, setFormat] = useState('mp3');
  const [bitrate, setBitrate] = useState('192');
  const [dir, setDir] = useState('');
  const [msg, setMsg] = useState('');
  const [busy, setBusy] = useState(false);
  useEffect(() => { ffmpegAvailable().then(setOk).catch(() => setOk(false)); }, []);
  if (ok === false) return <div className="p-6 text-sm">這台電腦找不到 ffmpeg，無法轉檔。裝好並放進 PATH 後再打開這個畫面。</div>;
  return (
    <div className="p-6 max-w-xl space-y-3">
      <h2 className="text-[26px] font-bold">轉檔</h2>
      <p className="text-sm text-[#8a847a]">{song ? song.path : '先選一首歌。'} 轉完才會顯示輸出路徑，沒有進度條。</p>
      <select className="field" value={format} onChange={(e) => setFormat(e.target.value)}>
        <option value="mp3">mp3</option>
        <option value="flac">flac</option>
        <option value="wav">wav</option>
        <option value="ogg">ogg</option>
      </select>
      {format === 'mp3' && (
        <select className="field" style={{ width: 120 }} value={bitrate} onChange={(e) => setBitrate(e.target.value)}>
          <option value="128">128 kbps</option>
          <option value="192">192 kbps</option>
          <option value="320">320 kbps</option>
        </select>
      )}
      <input className="field" placeholder="輸出資料夾，例如 D:\\Out" value={dir} onChange={(e) => setDir(e.target.value)} />
      <button
        className="round w-auto px-4 text-sm" disabled={!song || busy || ok === null}
        onClick={async () => {
          if (!song) return;
          setBusy(true); setMsg('');
          try { setMsg(await convertAudio({ song_id: song.id, format, output_dir: dir, bitrate_kbps: Number(bitrate) })); }
          catch (e) { setMsg(e instanceof Error ? e.message : String(e)); }
          finally { setBusy(false); }
        }}
      >{busy ? '轉檔中' : '開始轉檔'}</button>
      {msg && <p className="text-sm break-all">{msg}</p>}
    </div>
  );
}

export function SettingsView({ onScanned }: { onScanned: () => void }) {
  const [folders, setFolders] = useState<string[]>([]);
  const [draft, setDraft] = useState('');
  const [msg, setMsg] = useState('');
  const reload = async () => {
    const fs = await listMusicFolders();
    setFolders(fs.map((f) => f.path));
  };
  useEffect(() => { reload().catch((e) => setMsg(String(e))); }, []);
  return (
    <div className="p-6 max-w-xl space-y-3">
      <h2 className="text-[26px] font-bold">音樂資料夾</h2>
      <p className="text-sm text-[#8a847a]">選擇資料夾，或直接貼路徑。取消選擇不會改動已加入的資料夾。</p>
      <div className="flex gap-2">
        <input className="field" placeholder="D:\\Music" value={draft} onChange={(e) => setDraft(e.target.value)} />
        <button type="button" className="round w-auto px-3 text-sm" onClick={async () => {
          try {
            const picked = await browseFolder();
            if (!picked) return;
            setDraft(picked);
          } catch (e) {
            setMsg(e instanceof Error ? e.message : String(e));
          }
        }}>選擇</button>
        <button className="round w-auto px-3 text-sm" onClick={async () => {
          if (!draft.trim()) return;
          await addMusicFolder({ path: draft.trim() });
          setDraft('');
          await reload();
        }}>加入</button>
      </div>
      <ul className="text-sm space-y-1">
        {folders.map((f) => (
          <li key={f} className="flex justify-between gap-3 border-b border-[#2a2723] py-1">
            <span className="truncate">{f}</span>
            <button className="text-[#e23b2f]" onClick={async () => { await removeMusicFolder(f); await reload(); }}>移除</button>
          </li>
        ))}
      </ul>
      <button className="round w-auto px-4 text-sm" disabled={folders.length === 0} onClick={async () => {
        setMsg('掃描中');
        try { await scanFolders(folders); setMsg('掃描已送出'); onScanned(); }
        catch (e) { setMsg(e instanceof Error ? e.message : String(e)); }
      }}>掃描</button>
      {msg && <p className="text-sm text-[#8a847a]">{msg}</p>}
    </div>
  );
}
