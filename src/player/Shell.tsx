import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Pause, Play, SkipBack, SkipForward, Volume2, VolumeX, Repeat, Repeat1, Shuffle } from 'lucide-react';
import type { LyricLine, PlayMode, Playlist, Song, EngineSnapshot } from '../types';
import {
  addSongToPlaylist, createPlaylist, deletePlaylist, getConfig, getLyrics, getSong, getSongs, libraryFacets,
  listPlaylistItems, listPlaylists, listRecent, onScanProgress, playerLoad, playerPause, updatePlaylist, updatePlaylistOrder,
  playerPlay, playerPrepare, playerSetNext, playerClearNext, playerSeek, playerSetCrossfade, playerSetEq, playerSetMode, playerSetNormalize, playerSetPreamp, playerSetEqEnabled, playerSetFx, playerSetVolume, playerStatus,
  removeFromPlaylist, revealInExplorer, saveConfig, saveLyrics, scanFolders, searchSongs, setLyricsOffset, songPlayed, songsByField,
  listMusicFolders, browseFolder, addMusicFolder, takeOpenedFile, songByPath, onOpenFile,
  pickM3u, pickM3uSave, importM3u, exportM3u, smtcUpdate,
  playerOutputDevices, playerSetOutput, playerSetDefaultPlayer,
  writeTagsBatch,  listRadio, saveRadio, renameRadio, deleteRadio,
  classifyDrop,
  addSmartRule, listSmartRules, smartPlaylistSongs,
} from '../lib/api';
import { TrackTable, type SortKey } from './TrackTable';
import { ConvertView, EqPanel, RadioView, SettingsView, TagView } from './Tools';
import { SEED_VERSION, flatBands, mergeSeeds, parsePresets, type EqPreset, type PresetBind } from './eq';
import { FX_DEFAULT, parseAuto, pickAutoPreset, sceneFx, type AutoEq, type Fx, type Scene } from './fx';
import { sessionRestoreActions } from './openWithRestore';
import { emptySession, parseSession, type Session } from './session';
import { decideNext, PLAY_MODE, type DecideNextInput, type DecideNextOutput } from './nextTrack';

type View = 'all' | 'artists' | 'albums' | 'genres' | 'folders' | 'playlist' | 'queue' | 'recent' | 'radio' | 'tags' | 'convert' | 'settings';

type OpenTab = { key: string; kind: 'all' | 'recent' | 'queue' | 'playlist'; playlistId?: number; name: string };

const ALL_TAB: OpenTab = { key: 'all', kind: 'all', name: '全部歌曲' };

function persistTabs(next: OpenTab[]) {
  const ids = next.filter((tab) => tab.kind === 'playlist' && tab.playlistId).map((tab) => tab.playlistId);
  saveConfig('playlist_tabs', JSON.stringify(ids)).catch(() => {});
}

function fmt(ms: number) {
  if (!isFinite(ms) || ms < 0) return '0:00';
  const s = Math.floor(ms / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${String(sec).padStart(2, '0')}` : `${m}:${String(sec).padStart(2, '0')}`;
}

function techLine(song: Song | null) {
  if (!song) return '';
  const format = song.format || '未知';
  const br = song.bitrate && song.bitrate > 0 ? `${song.bitrate} kbps` : '未知';
  const sr = song.sample_rate && song.sample_rate > 0 ? `${song.sample_rate} Hz` : '未知';
  return `${format}　${br}　${sr}`;
}

function catalog(id: number | null) {
  return id ? `SL-${String(id).padStart(3, '0')}` : 'SL----';
}

function parseLrc(raw: string): LyricLine[] {
  const lines: LyricLine[] = [];
  for (const rawLine of raw.split(/\r?\n/)) {
    const times: number[] = [];
    const re = /\[(\d+):(\d+(?:[.:]\d+)?)\]/g;
    let rest = rawLine;
    let match: RegExpExecArray | null;
    while ((match = re.exec(rawLine))) {
      const min = Number(match[1]);
      const secPart = match[2];
      const [secText, frac = ''] = secPart.split(/[.:]/);
      const fracMs = frac.length >= 3 ? Number(frac.slice(0, 3)) : frac.length === 2 ? Number(frac) * 10 : frac.length === 1 ? Number(frac) * 100 : 0;
      times.push(min * 60000 + Number(secText) * 1000 + fracMs);
      rest = rest.replace(match[0], '');
    }
    if (!times.length) continue;
    const text = rest.trim();
    for (const time_ms of times) lines.push({ time_ms, text });
  }
  return lines.sort((a, b) => a.time_ms - b.time_ms);
}

function modeCopy(mode: PlayMode) {
  if (mode === 'All') return { short: '全部', title: '重複全部。再按切到隨機' };
  if (mode === 'Random') return { short: '隨機', title: '隨機播放。再按切到單曲重複' };
  if (mode === 'Single') return { short: '單曲', title: '單曲重複。再按回到順序播放' };
  return { short: '順序', title: '這首播完就停，不會自動接下一首。再按切到重複全部' };
}

export default function Shell() {
  const [view, setView] = useState<View>('all');
  const [facet, setFacet] = useState<string | null>(null);
  const [facets, setFacets] = useState<{ key: string; count: number }[]>([]);
  const [songs, setSongs] = useState<Song[]>([]);
  const [itemIds, setItemIds] = useState<Record<number, number>>({});
  const [itemOrder, setItemOrder] = useState<number[]>([]);
  const [scrollNonce, setScrollNonce] = useState(0);
  const [renameDraft, setRenameDraft] = useState('');
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [searchNote, setSearchNote] = useState('');
  const [playlists, setPlaylists] = useState<Playlist[]>([]);
  const [playlistId, setPlaylistId] = useState<number | null>(null);
  const [query, setQuery] = useState('');
  const [sortKey, setSortKey] = useState<SortKey>('title');
  const [sortDir, setSortDir] = useState<'asc' | 'desc'>('asc');
  const [activeId, setActiveId] = useState<number | null>(null);
  const [playing, setPlaying] = useState(false);
  const [position, setPosition] = useState(0);
  const [duration, setDuration] = useState(0);
  const [volume, setVolume] = useState(0.7);
  const [muted, setMuted] = useState(false);
  const [mode, setMode] = useState<PlayMode>('Sequence');
  const [queue, setQueue] = useState<Song[]>([]);
  const [order, setOrder] = useState<Song[]>([]);
  const [lines, setLines] = useState<LyricLine[]>([]);
  const [lyricOffset, setLyricOffset] = useState(0);
  const [lyricDraft, setLyricDraft] = useState('');
  const [m3uMissing, setM3uMissing] = useState<string[]>([]);
  const [tabs, setTabs] = useState<OpenTab[]>([ALL_TAB]);
  const [lyricsOpen, setLyricsOpen] = useState(true);
  const [queueName, setQueueName] = useState('');
  const [smartList, setSmartList] = useState(false);
  const [smartName, setSmartName] = useState('');
  const [smartKind, setSmartKind] = useState('played');
  const [smartGenre, setSmartGenre] = useState('');
  const [mini, setMini] = useState(false);
  const [eq, setEq] = useState<number[]>(flatBands());
  const [eqEnabled, setEqEnabled] = useState(true);
  const [eqOpen, setEqOpen] = useState(false);
  const [fx, setFx] = useState<Fx>({ ...FX_DEFAULT });
  const [autoEq, setAutoEq] = useState<AutoEq>({ enabled: false, fallback: '' });
  const [autoNote, setAutoNote] = useState('');
  const [preamp, setPreamp] = useState(0);
  const [normalize, setNormalize] = useState(false);
  const [crossfade, setCrossfade] = useState(0);
  const [gain, setGain] = useState(1);
  const [eqPresets, setEqPresets] = useState<EqPreset[]>([]);
  const [presetDraft, setPresetDraft] = useState('');
  const [selectedPreset, setSelectedPreset] = useState<string | null>(null);
  const [selected, setSelected] = useState<number[]>([]);
  const [menu, setMenu] = useState<{ x: number; y: number; song: Song } | null>(null);
  const [plMenu, setPlMenu] = useState<{ x: number; y: number; id: number; name: string } | null>(null);
  const [newName, setNewName] = useState('');
  const [scan, setScan] = useState('');
  const [scanErrors, setScanErrors] = useState<{ path: string; reason: string }[]>([]);
  const [engineError, setEngineError] = useState('');
  const [radioTitle, setRadioTitle] = useState('');
  const [batchDraft, setBatchDraft] = useState<{ artist: string; album: string; genre: string; year: string }>({ artist: '', album: '', genre: '', year: '' });
  const [sleepSeconds, setSleepSeconds] = useState(0);
  const [stopAfterCurrent, setStopAfterCurrent] = useState(false);
  const sleepRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const [outputDevices, setOutputDevices] = useState<string[]>([]);
  const [outputDevice, setOutputDevice] = useState('');
  const [defaultPlayer, setDefaultPlayer] = useState(false);
  const [offset, setOffset] = useState(0);
  const [hasMore, setHasMore] = useState(true);
  const activeRef = useRef<number | null>(null);
  const orderRef = useRef<Song[]>([]);
  const modeRef = useRef<PlayMode>('Sequence');
  // types.PlayMode（大寫）↔ nextTrack PlayMode（小寫）的轉換，decideNext 輸入用小寫
  const MODE_TO_NEXT: Record<PlayMode, import('./nextTrack').PlayMode> = {
    Sequence: 'sequence',
    Single: 'single',
    All: 'all',
    Random: 'random',
  };
  const endedRef = useRef(false);
  const volRef = useRef(0.7);
  const mutedRef = useRef(false);
  const eqRef = useRef<number[]>(flatBands());
  const eqOnRef = useRef(true);
  const fxRef = useRef<Fx>({ ...FX_DEFAULT });
  const autoSongRef = useRef<number | null>(null);
  const preampRef = useRef(0);
  const normRef = useRef(false);
  const fadeRef = useRef(0);
  const positionRef = useRef(0);
  const queueRef = useRef<Song[]>([]);
  const sessionRef = useRef<Session>(emptySession());
  const hydratedRef = useRef(false);
  const resumeToken = useRef(0);
  const lastFlushRef = useRef(0);
  const flushNowRef = useRef<() => Promise<void>>(async () => {});
  const viewRef = useRef<View>('all');
  const miniRef = useRef(false);
  const selectedRef = useRef<number[]>([]);
  const playlistIdRef = useRef<number | null>(null);
  const itemIdsRef = useRef<Record<number, number>>({});
  const shownRef = useRef<Song[]>([]);
  const playSongRef = useRef<(song: Song, list?: Song[]) => Promise<void>>(async () => {});
  const openPlaylistRef = useRef<(id: number, name?: string) => Promise<void>>(async () => {});

  // SP3：新流程的狀態
  const nextRequestedRef = useRef<{ songId: number; token: number; fromQueue: boolean } | null>(null);
  const requestedTokenRef = useRef<number | null>(null);      // 上一次送出 set_next 的 token（不論引擎是否準備好）
  const nextClearEpochRef = useRef(0);                         // 引擎自行清除下一首的 epoch
  const tokenSongRef = useRef<Map<number, { songId: number; fromQueue: boolean }>>(new Map());
  const failedRef = useRef<Set<number>>(new Set());            // 這階段解碼/播放失敗的歌曲 id
  const consecutiveFailuresRef = useRef(0);
  const stopEnqueueingRef = useRef(false);
  const playSeqRef = useRef<number | null>(null);              // 上一次處理過的 play_seq（避免重複副作用）

  const snapshot = (): Session => ({
    volume: volRef.current,
    muted: mutedRef.current,
    mode: modeRef.current,
    eq: eqRef.current.slice(),
    eqEnabled: eqOnRef.current,
    fx: { ...fxRef.current },
    preamp: preampRef.current,
    normalize: normRef.current,
    crossfade: fadeRef.current,
    songId: activeRef.current && activeRef.current > 0 ? activeRef.current : null,
    positionMs: Math.max(0, Math.round(positionRef.current)),
    queueIds: queueRef.current.map((s) => s.id).filter((id) => id > 0).slice(0, 500),
  });
  flushNowRef.current = async () => {
    if (!hydratedRef.current) return;
    const body = snapshot();
    sessionRef.current = body;
    lastFlushRef.current = Date.now();
    await saveConfig('session', JSON.stringify(body));
  };

  const active = songs.find((s) => s.id === activeId) ?? order.find((s) => s.id === activeId) ?? null;
  const fileName = active?.path?.split(/[/\\]/).pop() || '';
  const playingName = radioTitle || fileName || active?.title || '未選擇';
  const shown = useMemo(() => {
    const list = view === 'queue' ? queue : songs;
    const q = query.trim().toLowerCase();
    const filtered = q && view !== 'all' ? list.filter((s) => `${s.title} ${s.artist} ${s.album} ${s.path}`.toLowerCase().includes(q)) : list;
    if (view === 'recent' || view === 'queue' || view === 'playlist' || (view === 'albums' && facet)) return filtered;
    const dir = sortDir === 'asc' ? 1 : -1;
    return [...filtered].sort((a, b) => {
      const numeric = sortKey === 'duration_ms' || sortKey === 'play_count' || sortKey === 'file_mtime';
      const av = numeric ? (a[sortKey] || 0) : (a[sortKey] || '');
      const bv = numeric ? (b[sortKey] || 0) : (b[sortKey] || '');
      return av < bv ? -dir : av > bv ? dir : 0;
    });
  }, [songs, queue, view, query, sortKey, sortDir]);
  const totalMs = shown.reduce((n, s) => n + (s.duration_ms || 0), 0);

  const loadAll = useCallback(async (reset = true) => {
    const off = reset ? 0 : offset;
    const page = await getSongs(400, off, sortKey, sortDir);
    setSongs((prev) => reset ? page : [...prev, ...page]);
    setHasMore(page.length === 400);
    setOffset(off + page.length);
  }, [offset, sortKey, sortDir]);

  const openRecent = async () => {
    setQuery('');
    setFacet(null);
    setView('recent');
    try {
      const rows = await listRecent(200);
      setSongs(rows);
      setHasMore(false);
    } catch (e) {
      setSongs([]);
      setEngineError(e instanceof Error ? e.message : String(e));
    }
  };

  const openFacet = async (field: 'artist' | 'album' | 'genre' | 'folder') => {
    setView(field === 'artist' ? 'artists' : field === 'album' ? 'albums' : field === 'genre' ? 'genres' : 'folders');
    setFacet(null);
    setFacets(await libraryFacets(field));
  };

  const openSongs = async (field: string, value: string) => {
    setFacet(value);
    setSongs(await songsByField(field, value, 2000, 0, sortKey, sortDir));
    setHasMore(false);
  };

  const openPlaylist = async (id: number, name?: string) => {
    setTabs((cur) => {
      const key = `pl-${id}`;
      if (cur.some((tab) => tab.key === key)) return cur;
      const label = name || playlists.find((row) => row.id === id)?.name || '歌單';
      const next = [...cur, { key, kind: 'playlist' as const, playlistId: id, name: label }];
      persistTabs(next);
      return next;
    });
    setView('playlist');
    setPlaylistId(id);
    setFacet(null);
    const rules = await listSmartRules(id).catch(() => []);
    if (rules.length) {
      setSmartList(true);
      setItemIds({});
      setItemOrder([]);
      setSongs(await smartPlaylistSongs(id, 500, 0));
      setHasMore(false);
      return;
    }
    setSmartList(false);
    const items = await listPlaylistItems(id);
    const map: Record<number, number> = {};
    const ids: number[] = [];
    const list = items.flatMap((i) => {
      if (i.song) {
        map[i.song.id] = i.id;
        ids.push(i.id);
        return [i.song];
      }
      const flat = i as unknown as Song & { song_id?: number };
      if (!flat.path) return [];
      const song = { ...flat, id: flat.song_id || i.song_id || flat.id };
      map[song.id] = i.id;
      ids.push(i.id);
      return [song];
    });
    setItemIds(map);
    setItemOrder(ids);
    setSongs(list);
    setHasMore(false);
  };
  openPlaylistRef.current = openPlaylist;

  const pinTab = (tab: OpenTab) => {
    setTabs((cur) => {
      if (cur.some((row) => row.key === tab.key)) return cur;
      const next = [...cur, tab];
      persistTabs(next);
      return next;
    });
  };

  const openTab = async (tab: OpenTab) => {
    setQuery('');
    setFacet(null);
    if (tab.kind === 'all') {
      setView('all');
      setPlaylistId(null);
      setSmartList(false);
      await loadAll(true);
      return;
    }
    if (tab.kind === 'recent') {
      pinTab(tab);
      setSmartList(false);
      await openRecent();
      return;
    }
    if (tab.kind === 'queue') {
      pinTab(tab);
      setView('queue');
      setPlaylistId(null);
      setSmartList(false);
      return;
    }
    if (tab.playlistId) await openPlaylist(tab.playlistId, tab.name);
  };

  const closeTab = (key: string) => {
    if (key === 'all') return;
    const idx = tabs.findIndex((tab) => tab.key === key);
    const next = tabs.filter((tab) => tab.key !== key);
    const kept = next.length ? next : [ALL_TAB];
    setTabs(kept);
    persistTabs(kept);
    const active = view === 'playlist' && playlistId ? `pl-${playlistId}` : view === 'recent' ? 'recent' : view === 'queue' ? 'queue' : 'all';
    if (key === active) {
      const neighbor = kept[Math.max(0, Math.min(idx - 1, kept.length - 1))];
      void openTab(neighbor);
    }
  };

  useEffect(() => {
    const tokenAtStart = resumeToken.current;
    (async () => {
      try {
        const pls = await listPlaylists();
        setPlaylists(pls);
        try {
          const rawTabs = await getConfig('playlist_tabs');
          const ids = JSON.parse(rawTabs || '[]') as number[];
          const restored = (Array.isArray(ids) ? ids : []).flatMap((id) => {
            const row = pls.find((pl) => pl.id === id);
            return row ? [{ key: `pl-${row.id}`, kind: 'playlist' as const, playlistId: row.id, name: row.name }] : [];
          });
          if (restored.length) {
            setTabs((cur) => {
              const extras = cur.filter((tab) => tab.key !== 'all' && !restored.some((row) => row.key === tab.key));
              return [ALL_TAB, ...restored, ...extras];
            });
          }
          const lyricsFlag = await getConfig('lyrics_open');
          if (lyricsFlag === '0') setLyricsOpen(false);
        } catch { /* 沒有存過頁籤就維持全部歌曲 */ }
        try {
          // 舊版 10 段預設換算成 18 段保留；種子只補沒有同名的，使用者覆寫或刪掉的不動。
          const raw = await getConfig('eq_presets');
          const seeded = await getConfig('eq_presets_seed').catch(() => null);
          const parsed = parsePresets(raw);
          const merged = mergeSeeds(parsed, seeded ?? null);
          setEqPresets(merged.presets);
          const migrated = !!raw && JSON.stringify(parsed) !== raw;
          if (merged.changed || migrated) {
            await saveConfig('eq_presets', JSON.stringify(merged.presets)).catch(() => {});
            await saveConfig('eq_presets_seed', SEED_VERSION).catch(() => {});
          }
          setAutoEq(parseAuto(await getConfig('eq_auto').catch(() => null)));
        } catch { /* 讀不到預設就維持空清單，不擋開啟 */ }
        const folders = await listMusicFolders();
        const first = await getSongs(1, 0, 'title', 'asc');
        if (folders.length > 0 && first.length === 0) {
          setScan(`正在掃描 ${folders.map((f) => f.path).join('、')}`);
          const done = await scanFolders(folders.map((f) => f.path));
          setScan(done.current_file || '掃描完成');
        }
        await loadAll(true);
      } catch (e) {
        setEngineError(String(e));
      }
      try {
        const raw = await getConfig('session');
        const saved = parseSession(raw);
        volRef.current = saved.volume;
        mutedRef.current = saved.muted;
        modeRef.current = saved.mode;
        eqRef.current = saved.eq;
        eqOnRef.current = saved.eqEnabled;
        fxRef.current = saved.fx;
        preampRef.current = saved.preamp;
        normRef.current = saved.normalize;
        fadeRef.current = saved.crossfade;
        positionRef.current = saved.positionMs;
        setVolume(saved.volume);
        setMuted(saved.muted);
        setMode(saved.mode);
        setEq(saved.eq);
        setEqEnabled(saved.eqEnabled);
        setFx(saved.fx);
        setPreamp(saved.preamp);
        setNormalize(saved.normalize);
        setCrossfade(saved.crossfade);
        const restore = sessionRestoreActions(tokenAtStart, resumeToken.current, saved.muted);
        await playerSetVolume(restore.silenceForMute ? 0 : (saved.volume || 0.7));
        await playerSetMode(saved.mode);
        await playerSetEq(saved.eq);
        await playerSetEqEnabled(saved.eqEnabled);
        await playerSetFx(saved.fx);
        await playerSetPreamp(saved.preamp);
        await playerSetNormalize(saved.normalize);
        await playerSetCrossfade(saved.crossfade);
        const queued: Song[] = [];
        for (const id of saved.queueIds) {
          const row = await getSong(id).catch(() => null);
          if (row) queued.push(row);
        }
        queueRef.current = queued;
        setQueue(queued);
        const token = tokenAtStart;
        const claimed = () => !sessionRestoreActions(token, resumeToken.current, saved.muted).prepareSavedSong;
        if (saved.songId && !claimed()) {
          const song = await getSong(saved.songId).catch(() => null);
          if (song && token === resumeToken.current) {
            activeRef.current = song.id;
            setActiveId(song.id);
            setOrder([song]);
            orderRef.current = [song];
            setPosition(saved.positionMs);
            setDuration(song.duration_ms || 0);
            setPlaying(false);
            setRadioTitle('');
            hydratedRef.current = true;
            const lyric = await getLyrics(song.id).catch(() => null);
            if (token === resumeToken.current) {
              setLines(lyric?.lines ?? []);
              setLyricOffset(lyric?.offset_ms ?? 0);
            }
            if (token === resumeToken.current) {
              await playerPrepare(song.path, song.id, saved.positionMs, resumeToken.current);
            }
          }
        }
        hydratedRef.current = true;
      } catch (e) {
        setEngineError(e instanceof Error ? e.message : String(e));
      }
    })();
    const un = onScanProgress((p) => {
      setScan(`${p.stage} ${p.scanned}/${p.total} ${p.current_file}`);
      setScanErrors(p.errors);
    });
    return () => { un.then((fn) => fn()); };
  }, []);

  useEffect(() => {
    if (!hydratedRef.current) return;
    eqRef.current = eq;
    eqOnRef.current = eqEnabled;
    fxRef.current = fx;
    preampRef.current = preamp;
    normRef.current = normalize;
    fadeRef.current = crossfade;
    queueRef.current = queue;
    volRef.current = volume;
    mutedRef.current = muted;
    modeRef.current = mode;
    const timer = window.setTimeout(() => { void flushNowRef.current().catch((e) => setEngineError(String(e))); }, 400);
    return () => window.clearTimeout(timer);
  }, [volume, muted, mode, eq, eqEnabled, fx, preamp, normalize, crossfade, queue]);

  useEffect(() => {
    if (sleepRef.current) { clearInterval(sleepRef.current); sleepRef.current = null; }
    let remaining = sleepSeconds;
    if (remaining > 0) {
      sleepRef.current = setInterval(() => {
        remaining -= 1;
        setSleepSeconds(remaining);
        if (remaining <= 0) {
          playerPause().catch(() => {});
          setStopAfterCurrent(false);
          clearInterval(sleepRef.current!);
          sleepRef.current = null;
        }
      }, 1000);
    }
    return () => { if (sleepRef.current) clearInterval(sleepRef.current); sleepRef.current = null; };
  }, [sleepSeconds]);

  useEffect(() => {
    playerOutputDevices().then((ds) => { setOutputDevices(ds); if (ds.length && !outputDevice) setOutputDevice(ds[0]); }).catch(() => {});
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let gone = false;
    import('@tauri-apps/api/window').then(({ getCurrentWindow }) => {
      const win = getCurrentWindow();
      win.onCloseRequested(async (event) => {
        event.preventDefault();
        try {
          await Promise.race([
            flushNowRef.current(),
            new Promise((resolve) => window.setTimeout(resolve, 1500)),
          ]);
          if (miniRef.current) {
            const { LogicalSize } = await import('@tauri-apps/api/dpi');
            await Promise.race([
              (async () => {
                await win.setAlwaysOnTop(false);
                await win.setMinSize(new LogicalSize(900, 600));
                await win.setSize(new LogicalSize(1200, 800));
              })(),
              new Promise((resolve) => window.setTimeout(resolve, 800)),
            ]);
          }
        } catch { /* 寫不進去也要能關 */ }
        if (!gone) {
          try {
            await win.destroy();
          } catch { /* 權限被拒時，Rust 的關閉後備仍會結束程序 */ }
        }
      }).then((fn) => {
        if (gone) fn();
        else unlisten = fn;
      }).catch(() => {});
    }).catch(() => {});
    return () => {
      gone = true;
      unlisten?.();
    };
  }, []);

  const playSong = async (song: Song, list = shown) => {
    if (song.missing) {
      setEngineError(`缺檔，無法播放：${song.path}`);
      setPlaying(false);
      return;
    }
    resumeToken.current += 1;
    setOrder(list);
    orderRef.current = list;
    activeRef.current = song.id;
    setActiveId(song.id);
    setPlaying(true);
    setEngineError('');
    setRadioTitle('');
    endedRef.current = false;
    // SP3：不提前記歷史/消耗佇列；副作用改在新 play_seq 出現時統一處理
    const token = resumeToken.current;
    const songId = song.id > 0 ? song.id : 0;
    requestedTokenRef.current = token;
    tokenSongRef.current.set(token, { songId, fromQueue: false });
    try {
      await playerLoad(song.path, songId, token); // C1：帶 token
      await playerPlay().catch(() => {}); // 保險：確保啟動音訊
      await playerSetVolume(volRef.current || 0.7);
      positionRef.current = 0;
      void flushNowRef.current();
    } catch (e) {
      setEngineError(e instanceof Error ? e.message : String(e));
      setPlaying(false);
    }
    playSeqRef.current = null; // 重新播放，下一次 play_seq 變動才做副作用
  };
  playSongRef.current = playSong;

  useEffect(() => {
    const open = async (path: string) => {
      resumeToken.current += 1;
      try {
        let found: Song | null = null;
        try {
          found = await songByPath(path);
        } catch {
          found = null;
        }
        if (found) {
          await playSong(found, [found]);
          return;
        }
        const title = path.split(/[/\\]/).pop() || path;
        const ghost: Song = { id: 0, path, title, duration_ms: 0, file_mtime: 0, play_count: 0 };
        setOrder([ghost]);
        orderRef.current = [ghost];
        activeRef.current = 0;
        setActiveId(0);
        setPlaying(true);
        setRadioTitle('');
        setEngineError('');
        await playerLoad(path, 0, resumeToken.current);
        await playerPlay().catch(() => {});
        await playerSetVolume(volRef.current || 0.7);
      } catch (e) {
        setEngineError(e instanceof Error ? e.message : String(e));
        setPlaying(false);
      }
    };
    takeOpenedFile().then((p) => { if (p) open(p); }).catch((e) => setEngineError(String(e)));
    const poll = window.setInterval(() => {
      takeOpenedFile().then((p) => { if (p) open(p); }).catch(() => {});
    }, 400);
    const un = onOpenFile((path) => { open(path); });
    return () => {
      window.clearInterval(poll);
      un.then((fn) => fn());
    };
  }, []);

  const stepRef = useRef<(dir: 1 | -1, reason?: 'user' | 'ended') => void>(() => {});
  const step = async (dir: 1 | -1, reason: 'user' | 'ended' = 'user') => {
    if (dir === 1 && queue.length) {
      const [head, ...rest] = queue;
      queueRef.current = rest;
      setQueue(rest);
      await playSong(head, [head, ...rest]);
      return;
    }
    if (reason === 'ended' && stopAfterCurrent) {
      setStopAfterCurrent(false);
      setPlaying(false);
      return;
    }
    if (reason === 'ended' && modeRef.current === 'Sequence') {
      setPlaying(false);
      return;
    }
    const list = orderRef.current.length ? orderRef.current : shown;
    const i = list.findIndex((s) => s.id === activeRef.current);
    if (reason === 'ended' && modeRef.current === 'Single' && list[i]) {
      await playSong(list[i], list);
      return;
    }
    if (modeRef.current === 'Random' && dir === 1) {
      const pick = list[Math.floor(Math.random() * list.length)];
      if (pick) await playSong(pick, list);
      return;
    }
    const n = list[i + dir] ?? (modeRef.current === 'All' ? list[dir === 1 ? 0 : list.length - 1] : null);
    if (n) await playSong(n, list);
    else setPlaying(false);
  };
  stepRef.current = step;
  const playingRef = useRef(playing);
  playingRef.current = playing;

  useEffect(() => {
    const un = import('@tauri-apps/api/event').then(({ listen }) => listen<string>('transport', (event) => {
      const kind = event.payload;
      if (kind === 'next') { stepRef.current(1); return; }
      if (kind === 'prev') { stepRef.current(-1); return; }
      if (kind === 'pause' || (kind === 'toggle' && playingRef.current)) {
        playerPause().catch((err) => setEngineError(String(err)));
        void flushNowRef.current().catch((err) => setEngineError(String(err)));
        return;
      }
      if (kind === 'play' || kind === 'toggle') playerPlay().catch((err) => setEngineError(String(err)));
    }));
    return () => { un.then((fn) => fn()); };
  }, []);

  useEffect(() => {
    const enqueueNext = async (current: Song, status: EngineSnapshot) => {
      const input: DecideNextInput = {
        reason: status.playback_state === 'Finished' ? 'ended' : 'user',
        playlist: orderRef.current.map((s) => String(s.id)),
        queue: queueRef.current.map((s) => String(s.id)),
        mode: MODE_TO_NEXT[modeRef.current],
        playAfterEnd: stopAfterCurrent,
        currentSong: String(current.id),
        randomPreselected: null,
        failed: new Set([...failedRef.current].map(String)),
      };
      const result = decideNext(input);
      consecutiveFailuresRef.current = result.consecutiveFailures;
      stopEnqueueingRef.current = result.stopEnqueueing;
      for (const id of result.removedFromQueue) {
        const nid = Number(id);
        queueRef.current = queueRef.current.filter((s) => s.id !== nid);
      }
      setQueue(queueRef.current);

      if (result.song == null || stopEnqueueingRef.current) {
        await playerClearNext().catch(() => {});
        requestedTokenRef.current = 0;
        nextClearEpochRef.current = status.next_clear_epoch;
        nextRequestedRef.current = null;
        return;
      }

      const songId = Number(result.song);
      const fromQueue = Boolean(result.fromQueue);
      const nextSong =
        orderRef.current.find((x) => x.id === songId) ?? queueRef.current.find((x) => x.id === songId);
      if (!nextSong) return;

      const nextToken = (requestedTokenRef.current ?? 0) + 1;
      requestedTokenRef.current = nextToken;
      tokenSongRef.current.set(nextToken, { songId, fromQueue });
      await playerSetNext(nextSong.path, songId, nextToken).catch(() => {});
      nextRequestedRef.current = { songId, token: nextToken, fromQueue };
    };

    const t = setInterval(async () => {
      try {
        const s = await playerStatus();
        setPosition(s.position_ms);
        if (s.kind !== 'radio') {
          positionRef.current = s.position_ms;
          setDuration(s.duration_ms);
        }
        setPlaying(s.is_playing);
        setGain(s.applied_gain);
        if (s.error) setEngineError(s.error);
        if (s.kind === 'radio') setRadioTitle(s.stream_title);
        if (s.kind !== 'radio' && s.is_playing && hydratedRef.current && Date.now() - lastFlushRef.current >= 5000) {
          void flushNowRef.current().catch(() => {});
        }

        // SP3：以 play_seq 變動為唯一副作用觸發點（歷史、佇列消耗、下一首一次到位）；不再輪詢 ended 重載
        if (
          s.kind !== 'radio' &&
          s.song_id &&
          s.song_id !== activeRef.current &&
          s.play_seq != null &&
          s.play_seq !== playSeqRef.current
        ) {
          playSeqRef.current = s.play_seq;
          const found =
            orderRef.current.find((x) => x.id === s.song_id) ??
            queueRef.current.find((x) => x.id === s.song_id) ??
            songs.find((x) => x.id === s.song_id);
          if (found) {
            activeRef.current = s.song_id;
            setActiveId(s.song_id);
            setOrder([found, ...orderRef.current.filter((x) => x.id !== s.song_id)]);
            setDuration(found.duration_ms || 0);
            // 記播放（只一次）；歌詞若已過期則丟棄
            await songPlayed(s.song_id).catch(() => {});
            const lyric = await getLyrics(s.song_id);
            setLines(lyric?.lines ?? []);
            setLyricOffset(lyric?.offset_ms ?? 0);
            // 佇列來的歌：此時才消耗佇列
            if (nextRequestedRef.current?.songId === s.song_id) {
              const head = queueRef.current[0];
              if (head && head.id === s.song_id) {
                queueRef.current = queueRef.current.slice(1);
                setQueue(queueRef.current);
              }
              nextRequestedRef.current = null;
            }
            await enqueueNext(found, s);

            // SP3：引擎自行清除了下一首（seek/換裝置/player_load）會遞增 next_clear_epoch。
            // 若前端仍有「已要求的 token」且尚未成歌，就重新送出一次 set_next；
            // 比對對象是「已要求的 token」，不是快照的 next_token（可能還在準備中）。
            if (
              s.next_clear_epoch !== nextClearEpochRef.current &&
              requestedTokenRef.current !== null
            ) {
              const pending = nextRequestedRef.current;
              if (pending) {
                const nextSong =
                  orderRef.current.find((x) => x.id === pending.songId) ??
                  queueRef.current.find((x) => x.id === pending.songId);
                if (nextSong) {
                  const nextToken = (requestedTokenRef.current ?? 0) + 1;
                  requestedTokenRef.current = nextToken;
                  tokenSongRef.current.set(nextToken, {
                    songId: pending.songId,
                    fromQueue: pending.fromQueue,
                  });
                  await playerSetNext(nextSong.path, pending.songId, nextToken).catch(() => {});
                  nextRequestedRef.current = {
                    songId: pending.songId,
                    token: nextToken,
                    fromQueue: pending.fromQueue,
                  };
                }
              }
              nextClearEpochRef.current = s.next_clear_epoch;
            }
          }
        }
        if (!s.ended) endedRef.current = false;
      } catch { /* 視窗還沒接上後端時不刷錯誤 */ }
    }, 250);
    return () => clearInterval(t);
  }, [queue]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement)?.tagName;
      if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return;
      if (e.code === 'Space' && tag !== 'BUTTON') {
        e.preventDefault();
        if (playingRef.current) {
          playerPause().catch((err) => setEngineError(String(err)));
          void flushNowRef.current().catch((err) => setEngineError(String(err)));
        } else playerPlay().catch((err) => setEngineError(String(err)));
      }
      if (e.code === 'Enter' && tag !== 'BUTTON') {
        const id = selectedRef.current[0];
        const song = shownRef.current.find((row) => row.id === id);
        if (song) {
          e.preventDefault();
          void playSongRef.current(song, shownRef.current);
        }
      }
      if (e.code === 'Delete' && viewRef.current === 'playlist' && playlistIdRef.current) {
        e.preventDefault();
        const pid = playlistIdRef.current;
        const ids = selectedRef.current;
        const map = itemIdsRef.current;
        void (async () => {
          let removed = 0;
          for (const id of ids) {
            const itemId = map[id];
            if (!itemId) continue;
            await removeFromPlaylist(itemId);
            removed += 1;
          }
          if (removed) {
            setEngineError('');
            setScan(`已從歌單移除 ${removed} 首`);
            await openPlaylistRef.current(pid);
          }
        })().catch((err) => setEngineError(err instanceof Error ? err.message : String(err)));
      }
      if (e.code === 'ArrowRight' && e.altKey) step(1);
      if (e.code === 'ArrowLeft' && e.altKey) step(-1);
      if (e.code === 'KeyF' && e.ctrlKey) { e.preventDefault(); document.getElementById('filter')?.focus(); }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [playing, shown]);

  const applyPreset = async (name: string) => {
    const preset = eqPresets.find((p) => p.name === name);
    if (!preset) return;
    setSelectedPreset(name);
    setEq(preset.bands);
    eqRef.current = preset.bands.slice();
    await playerSetEq(preset.bands).catch((e) => setEngineError(String(e)));
    if (typeof preset.preamp === 'number') {
      setPreamp(preset.preamp);
      preampRef.current = preset.preamp;
      await playerSetPreamp(preset.preamp).catch((e) => setEngineError(String(e)));
    }
    void flushNowRef.current().catch(() => {});
  };

  // 參考播放器「依曲目自動調整等化器預設」：每首歌只在換到它時套一次，之後使用者手動改不會被蓋回去。
  useEffect(() => {
    if (!autoEq.enabled || !activeId || autoSongRef.current === activeId) return;
    autoSongRef.current = activeId;
    const pick = pickAutoPreset(active, eqPresets, autoEq);
    if (!pick) { setAutoNote('沒有符合的預設，維持目前曲線'); return; }
    setAutoNote(`自動套用「${pick.preset.name}」：${pick.reason}`);
    if (selectedPreset !== pick.preset.name) void applyPreset(pick.preset.name);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeId, autoEq, eqPresets, active]);

  const savePreset = async (bind?: PresetBind) => {
    const name = presetDraft.trim();
    if (!name) {
      setEngineError('預設名稱不能是空的');
      return;
    }
    const next = eqPresets.filter((p) => p.name !== name);
    next.push(bind ? { name, bands: eq.slice(), preamp, bind } : { name, bands: eq.slice(), preamp });
    setEqPresets(next);
    setPresetDraft('');
    setSelectedPreset(name);
    await saveConfig('eq_presets', JSON.stringify(next)).catch((e) => setEngineError(String(e)));
  };

  const deletePreset = async (name: string) => {
    const next = eqPresets.filter((p) => p.name !== name);
    setEqPresets(next);
    setSelectedPreset(null);
    await saveConfig('eq_presets', JSON.stringify(next)).catch((e) => setEngineError(String(e)));
  };

  // 等化器每個變更直接打到音訊執行緒；後端即時換係數，不重開音源。失敗顯示在狀態列。
  const changeBands = (bands: number[]) => {
    setEq(bands);
    setSelectedPreset(null);
    playerSetEq(bands).catch((e) => setEngineError(String(e)));
  };
  const changePreamp = (db: number) => {
    setPreamp(db);
    playerSetPreamp(db).catch((e) => setEngineError(String(e)));
  };
  const changeEqEnabled = (on: boolean) => {
    setEqEnabled(on);
    playerSetEqEnabled(on).catch((e) => setEngineError(String(e)));
  };
  const changeNormalize = (on: boolean) => {
    setNormalize(on);
    playerSetNormalize(on).catch((e) => setEngineError(String(e)));
  };
  const changeCrossfade = (ms: number) => {
    setCrossfade(ms);
    playerSetCrossfade(ms).catch((e) => setEngineError(String(e)));
  };
  const closeEq = useCallback(() => setEqOpen(false), []);
  const changeFx = (next: Fx) => {
    setFx(next);
    fxRef.current = next;
    playerSetFx(next).catch((e) => setEngineError(String(e)));
  };
  // 情境建議：等化器、前級、音效一起換；音效從預設值開始覆寫，不殘留上一個情境。
  const applyScene = (scene: Scene) => {
    setSelectedPreset(null);
    changeBands(scene.bands.slice());
    changePreamp(scene.preamp);
    if (!eqEnabled) changeEqEnabled(true);
    changeFx(sceneFx(scene));
    setScan(`已套用情境「${scene.name}」`);
  };
  const changeAuto = (next: AutoEq) => {
    setAutoEq(next);
    if (next.enabled && !autoEq.enabled) autoSongRef.current = null;
    saveConfig('eq_auto', JSON.stringify(next)).catch((e) => setEngineError(String(e)));
  };

  const cycleMode = () => {
    const next: PlayMode = mode === 'Sequence' ? 'All' : mode === 'All' ? 'Random' : mode === 'Random' ? 'Single' : 'Sequence';
    setMode(next);
    modeRef.current = next;
    playerSetMode(next).catch((e) => setEngineError(String(e)));
  };

  const actedSongs = (song: Song) => {
    if (!selected.includes(song.id)) return [song];
    const picked = new Set(selected);
    const ordered = shown.filter((row) => picked.has(row.id));
    return ordered.length ? ordered : [song];
  };

  const putQueue = (targets: Song[], where: 'next' | 'end') => {
    const ids = new Set(targets.map((row) => row.id));
    const rest = queue.filter((row) => !ids.has(row.id));
    const next = where === 'next' ? [...targets, ...rest] : [...rest, ...targets];
    queueRef.current = next;
    setQueue(next);
  };

  const reorderPlaylist = async (from: number, to: number) => {
    if (view !== 'playlist' || playlistId == null || query.trim() || from === to) return;
    if (itemOrder.length !== songs.length || from < 0 || to < 0 || from >= songs.length || to >= songs.length) {
      setEngineError('歌單順序對不上，請重新打開這個歌單');
      return;
    }
    const nextSongs = songs.slice();
    const nextIds = itemOrder.slice();
    const [movedSong] = nextSongs.splice(from, 1);
    const [movedId] = nextIds.splice(from, 1);
    nextSongs.splice(to, 0, movedSong);
    nextIds.splice(to, 0, movedId);
    setSongs(nextSongs);
    setItemOrder(nextIds);
    try {
      await updatePlaylistOrder(playlistId, nextIds);
    } catch (e) {
      setEngineError(e instanceof Error ? e.message : String(e));
      await openPlaylist(playlistId);
    }
  };

  const reorderQueue = (from: number, to: number) => {
    if (from === to || from < 0 || to < 0) return;
    setQueue((cur) => {
      if (from >= cur.length || to >= cur.length) return cur;
      const next = cur.slice();
      const [moved] = next.splice(from, 1);
      next.splice(to, 0, moved);
      queueRef.current = next;
      return next;
    });
  };

  const clearQueue = () => {
    queueRef.current = [];
    setQueue([]);
  };

  const saveQueueAsPlaylist = async () => {
    const name = queueName.trim();
    if (!name) {
      setEngineError('歌單名稱不能是空的');
      return;
    }
    const rows = queue.filter((song) => song.id > 0);
    if (!rows.length) {
      setEngineError('佇列裡沒有曲庫裡的歌');
      return;
    }
    try {
      const id = await createPlaylist({ name, sort_order: playlists.length, is_smart: 0 });
      for (let i = 0; i < rows.length; i++) {
        await addSongToPlaylist({ playlist_id: id, song_id: rows[i].id, position: i });
      }
      setQueueName('');
      setPlaylists(await listPlaylists());
      setEngineError('');
      await openPlaylist(id, name);
    } catch (e) {
      setEngineError(e instanceof Error ? e.message : String(e));
    }
  };

  const handleDrop = async (e: React.DragEvent<HTMLDivElement>) => {
    e.preventDefault();
    const dt = e.dataTransfer;
    if (!dt) return;
    const text = dt.getData('text/plain');
    const paths = text.split(/\r?\n/).filter((p) => p.trim().length > 0);
    if (!paths.length) return;
    try {
      const result = await classifyDrop(paths);
      for (const dir of result.dirs) {
        await addMusicFolder({ path: dir });
        setScan(`正在掃描 ${dir}`);
        await scanFolders([dir]);
      }
      const filePaths = result.files;
      if (filePaths.length === 0) return;
      if (view === 'playlist' && playlistId != null) {
        let position = 0;
        const existing = await listPlaylistItems(playlistId);
        position = existing.length;
        for (const p of filePaths) {
          const found = await songByPath(p);
          if (found) {
            await addSongToPlaylist({ playlist_id: playlistId, song_id: found.id, position });
            position += 1;
          }
        }
        if (playlistId) await openPlaylist(playlistId);
        setScan(`已將 ${filePaths.length} 個檔案加入歌單`);
        return;
      }
      const first = filePaths[0];
      const found = await songByPath(first);
      if (found) {
        await playSong(found, [found]);
      } else {
        const title = first.split(/[/\\]/).pop() || first;
        const ghost: Song = { id: 0, path: first, title, duration_ms: 0, file_mtime: 0, play_count: 0 };
        setOrder([ghost]);
        orderRef.current = [ghost];
        activeRef.current = 0;
        setActiveId(0);
        setPlaying(true);
        setRadioTitle('');
        setEngineError('');
        await playerLoad(first, 0, resumeToken.current);
        await playerSetVolume(mutedRef.current ? 0 : volRef.current);
      }
      for (let i = 1; i < filePaths.length; i++) {
        const f = await songByPath(filePaths[i]);
        if (f) putQueue([f], 'end');
      }
      setScan('');
    } catch (err) {
      setEngineError(err instanceof Error ? err.message : String(err));
    }
  };

  const jumpToPlaying = () => {
    if (activeId == null || !shown.some((row) => row.id === activeId)) {
      setEngineError('正在播放的歌不在這個清單');
      return;
    }
    setEngineError('');
    setScrollNonce((n) => n + 1);
  };

  const userPlaylists = playlists.filter((pl) => pl.name !== '全部歌曲' && pl.name !== '最近播放');

  const pushVolume = (slider: number, mute: boolean) => {
    playerSetVolume(mute ? 0 : slider).catch((e) => setEngineError(String(e)));
  };

  const setSliderVolume = (raw: number) => {
    const v = Math.min(1, Math.max(0, Math.round(raw * 100) / 100));
    setVolume(v);
    volRef.current = v;
    const mute = v === 0;
    setMuted(mute);
    mutedRef.current = mute;
    pushVolume(v, mute);
  };

  const toggleMute = () => {
    if (!muted && volume > 0) {
      setMuted(true);
      mutedRef.current = true;
      pushVolume(volRef.current, true);
      return;
    }
    const restore = volRef.current > 0 ? volRef.current : 0.7;
    volRef.current = restore;
    setVolume(restore);
    setMuted(false);
    mutedRef.current = false;
    pushVolume(restore, false);
  };

  const modeInfo = modeCopy(mode);
  const volumePct = Math.round((muted ? 0 : volume) * 100);

  const lyricAt = (line: LyricLine) => line.time_ms + lyricOffset;
  const lyricIndex = (() => {
    const pos = position;
    let idx = -1;
    for (let i = 0; i < lines.length; i++) if (lyricAt(lines[i]) <= pos) idx = i;
    return idx;
  })();

  const tool = view === 'radio' || view === 'tags' || view === 'convert' || view === 'settings';
  const facetField = view === 'artists' ? 'artist' : view === 'albums' ? 'album' : view === 'genres' ? 'genre' : view === 'folders' ? 'folder' : null;

  useEffect(() => {
    const title = playingName && playingName !== '未選擇' ? `SonicLedger — ${playingName}` : 'SonicLedger / 音樂隨身聽';
    import('@tauri-apps/api/window').then(({ getCurrentWindow }) => {
      getCurrentWindow().setTitle(title).catch(() => {});
    }).catch(() => {});
  }, [playingName]);

  useEffect(() => {
    if (playingName === '未選擇') return;
    smtcUpdate(playingName, active?.artist || radioTitle || '', playing).catch(() => {});
  }, [playingName, playing, active?.artist, radioTitle]);

  useEffect(() => {
    document.querySelector('[data-lyric-now="1"]')?.scrollIntoView({ block: 'nearest' });
  }, [lyricIndex, activeId]);

  viewRef.current = view;
  selectedRef.current = selected;
  playlistIdRef.current = playlistId;
  itemIdsRef.current = itemIds;
  shownRef.current = shown;
  const activeTabKey = view === 'playlist' && playlistId ? `pl-${playlistId}` : view === 'recent' ? 'recent' : view === 'queue' ? 'queue' : view === 'all' && !facet ? 'all' : '';

  const toggleMini = async () => {
    const { getCurrentWindow } = await import('@tauri-apps/api/window');
    const { LogicalSize } = await import('@tauri-apps/api/dpi');
    const win = getCurrentWindow();
    if (!mini) {
      miniRef.current = true;
      setMini(true);
      // Wait for React to render mini layout before resizing
      await new Promise(r => setTimeout(r, 50));
      await win.setMinSize(new LogicalSize(360, 140));
      await win.setSize(new LogicalSize(480, 200));
      await win.setAlwaysOnTop(true);
      return;
    }
    await win.setAlwaysOnTop(false);
    miniRef.current = false;
    setMini(false);
    // Wait for React to render normal layout before resizing
    await new Promise(r => setTimeout(r, 50));
    await win.setMinSize(new LogicalSize(900, 600));
    await win.setSize(new LogicalSize(1200, 800));
  };

  if (mini) {
    return (
      <div className="h-screen flex flex-col justify-center gap-2 bg-[#12110f] text-[#f4f0e8] px-3 py-2" style={playing ? { boxShadow: 'inset 0 0 0 1px #e23b2f' } : undefined}>
        <div className="flex items-center gap-2 min-w-0">
          <span className="text-[11px] tracking-widest text-[#e23b2f] shrink-0">{radioTitle ? 'LIVE' : catalog(activeId)}</span>
          <div className={`now-title text-[15px] leading-snug font-bold min-w-0 flex-1 ${playing ? 'is-on' : ''}`} title={playingName}>
            <span className="ink-pass" aria-hidden="true">{playingName}</span>
            <span className="face">{playingName}</span>
          </div>
          <button type="button" className="round w-auto px-2 text-[11px] shrink-0" onClick={() => { toggleMini().catch((e) => setEngineError(String(e))); }}>還原</button>
        </div>
        <div className="flex items-center gap-2 min-w-0">
          <button className="round" aria-label="上一首" onClick={() => step(-1)}><SkipBack size={14} /></button>
          <button className="round" aria-label={playing ? '暫停' : '播放'} onClick={() => { if (playing) { playerPause().catch((e) => setEngineError(String(e))); } else playerPlay().catch((e) => setEngineError(String(e))); }}>{playing ? <Pause size={14} /> : <Play size={14} />}</button>
          <button className="round" aria-label="下一首" onClick={() => step(1)}><SkipForward size={14} /></button>
          <span className="num text-[11px] text-[#8a847a] w-10 shrink-0">{radioTitle ? '直播' : fmt(position)}</span>
          <input aria-label="播放位置" type="range" min={0} max={Math.max(duration, 1)} value={Math.min(position, duration || 1)} disabled={!!radioTitle} style={{ ['--pct' as string]: `${duration > 0 ? Math.min(100, (position / duration) * 100) : 0}%` }} onChange={(e) => { const ms = Number(e.target.value); setPosition(ms); positionRef.current = ms; playerSeek(ms); }} className="line flex-1" />
          <button type="button" className="round" aria-label={muted ? '取消靜音' : '靜音'} onClick={toggleMute}>{muted || volume === 0 ? <VolumeX size={14} /> : <Volume2 size={14} />}</button>
        </div>
      </div>
    );
  }

  return (
    <div className="h-screen flex flex-col bg-[#12110f] text-[#f4f0e8]" onClick={() => { setMenu(null); setPlMenu(null); }} onDragOver={(e) => e.preventDefault()} onDrop={handleDrop}>
      <header className="shrink-0 border-b border-[#2a2723] px-5 py-2">
        <div className="flex items-center gap-4 min-w-0">
          <div className="min-w-0 flex-1">
            <div className="flex items-baseline gap-3 min-w-0">
              <span className="text-[12px] tracking-widest text-[#e23b2f] shrink-0">{radioTitle ? 'LIVE' : catalog(activeId)}</span>
              <div className={`now-title text-[18px] leading-snug font-bold min-w-0 flex-1 ${playing ? 'is-on' : ''}`} title={playingName}>
                <span className="ink-pass" aria-hidden="true">{playingName}</span>
                <span className="face">{playingName}</span>
              </div>
            </div>
            <div className="text-[12px] text-[#8a847a] truncate" title={active?.artist || ''}>
              {radioTitle || [active?.artist, active?.album].filter(Boolean).join(' · ') || '尚未播放'}
            </div>
          </div>
          <div className="text-right shrink-0">
            <b className="block text-[16px] text-[#e23b2f] num leading-none">{fmt(totalMs)}</b>
            <span className="text-[12px] text-[#8a847a]">{shown.length} 首</span>
            <button type="button" className="round w-auto px-2 text-[11px] mt-1" onClick={() => { toggleMini().catch((e) => setEngineError(String(e))); }}>縮小</button>
          </div>
        </div>
      </header>
      {engineError && <div className="px-5 py-1 text-xs text-[#e23b2f] border-b border-[#2a2723]">{engineError}</div>}
      {m3uMissing.length > 0 && (
        <div className="px-5 py-2 text-xs text-[#e23b2f] border-b border-[#2a2723] max-h-24 overflow-y-auto">
          {m3uMissing.map((line) => <div key={line} className="break-all">{line}</div>)}
        </div>
      )}
      {scan && <div className="px-5 py-1 text-xs text-[#8a847a] border-b border-[#2a2723]">{scan}</div>}
      {scanErrors.length > 0 && (
        <div className="px-5 py-2 text-xs text-[#e23b2f] border-b border-[#2a2723] max-h-24 overflow-y-auto">
          <div className="mb-1 font-bold text-[#e23b2f]">掃描失敗：</div>
          {scanErrors.map((e, i) => (
            <div key={i} className="break-all text-[#f4f0e8]">
              {e.path} — {e.reason}
            </div>
          ))}
        </div>
      )}
      <div className="flex flex-1 min-h-0">
        <nav className="w-[208px] shrink-0 border-r border-[#2a2723] py-3 px-2 text-sm overflow-y-auto overflow-x-hidden">
          {([
            ['all', '全部歌曲'], ['artists', '演出者'], ['albums', '專輯'], ['genres', '曲風'], ['folders', '資料夾'],
            ['queue', '播放佇列'], ['recent', '最近播放'],
          ] as [View, string][]).map(([id, label]) => (
            <button key={id} className={`flex items-center w-full min-h-9 px-3 text-left leading-5 rounded-sm ${view === id && !facet ? 'bg-[#1c1b18] text-[#f4f0e8]' : 'text-[#8a847a]'}`} onClick={() => {
              setQuery('');
              if (id === 'all') void openTab(ALL_TAB);
              else if (id === 'artists') openFacet('artist');
              else if (id === 'albums') openFacet('album');
              else if (id === 'genres') openFacet('genre');
              else if (id === 'folders') openFacet('folder');
              else if (id === 'recent') void openTab({ key: 'recent', kind: 'recent', name: '最近播放' });
              else if (id === 'queue') void openTab({ key: 'queue', kind: 'queue', name: '播放佇列' });
              else setView(id);
            }}>{label}</button>
          ))}
          <div className="mt-4 mb-1 px-3 text-[11px] tracking-widest text-[#8a847a]">歌單</div>
          {userPlaylists.map((pl) => (
            <button key={pl.id} className={`flex items-center w-full min-h-9 px-3 text-left leading-5 rounded-sm ${playlistId === pl.id && view === 'playlist' ? 'bg-[#1c1b18] text-[#f4f0e8]' : 'text-[#8a847a]'}`} onClick={() => openTab({ key: `pl-${pl.id}`, kind: 'playlist', playlistId: pl.id, name: pl.name })} onContextMenu={(e) => { e.preventDefault(); e.stopPropagation(); setRenameDraft(pl.name); setConfirmDelete(false); setPlMenu({ x: e.clientX, y: e.clientY, id: pl.id, name: pl.name }); }}>
              <span className="truncate">{pl.is_smart ? `${pl.name} · 智慧` : pl.name}</span>
            </button>
          ))}
          <form className="px-3 mt-2" onSubmit={async (e) => {
            e.preventDefault();
            const name = newName.trim();
            if (!name) return;
            const id = await createPlaylist({ name, sort_order: playlists.length, is_smart: 0 });
            setNewName('');
            setPlaylists(await listPlaylists());
            await openPlaylist(id, name);
          }}>
            <input className="field" placeholder="新歌單" value={newName} onChange={(e) => setNewName(e.target.value)} />
          </form>
          <form className="px-3 pb-2 space-y-1" onSubmit={async (e) => {
            e.preventDefault();
            const name = smartName.trim();
            if (!name) return;
            if (smartKind === 'genre' && !smartGenre.trim()) {
              setEngineError('要填曲風');
              return;
            }
            try {
              const id = await createPlaylist({ name, sort_order: playlists.length, is_smart: 1 });
              if (smartKind === 'played') await addSmartRule(id, 'play_count', '>', '0');
              else if (smartKind === 'month') {
                const since = new Date(Date.now() - 30 * 86400000).toISOString();
                await addSmartRule(id, 'added_at', '>', since);
              } else if (smartKind === 'genre') await addSmartRule(id, 'genre', '=', smartGenre.trim());
              else await addSmartRule(id, 'added_at', '>', '');
              setSmartName('');
              setSmartGenre('');
              setPlaylists(await listPlaylists());
              await openPlaylist(id, name);
            } catch (err) {
              setEngineError(err instanceof Error ? err.message : String(err));
            }
          }}>
            <input className="field" placeholder="智慧歌單" value={smartName} onChange={(e) => setSmartName(e.target.value)} />
            <select className="field" value={smartKind} onChange={(e) => setSmartKind(e.target.value)}>
              <option value="played">播放次數最多</option>
              <option value="added">最近加入</option>
              <option value="month">最近 30 天加入</option>
              <option value="genre">指定曲風</option>
            </select>
            {smartKind === 'genre' && <input className="field" placeholder="曲風" value={smartGenre} onChange={(e) => setSmartGenre(e.target.value)} />}
            <button type="submit" className="round w-auto px-3 text-xs">建立智慧歌單</button>
          </form>
          <button type="button" className="flex items-center w-full min-h-9 px-3 text-left leading-5 text-[#8a847a]" onClick={async () => {
            try {
              const picked = await pickM3u();
              if (!picked) return;
              const result = await importM3u(picked);
              setM3uMissing(result.missing);
              setPlaylists(await listPlaylists());
              if (result.playlist_id) {
                await openPlaylist(result.playlist_id, result.name);
                setEngineError(result.missing.length ? `已匯入 ${result.matched} 首。下面這些路徑對不到。` : '');
              } else {
                setEngineError('沒有對到曲庫的歌，沒有建立歌單');
              }
            } catch (e) {
              setEngineError(e instanceof Error ? e.message : String(e));
            }
          }}>匯入 m3u</button>
          <div className="mt-4 mb-1 px-3 text-[11px] tracking-widest text-[#8a847a]">工具</div>
          {(['radio', 'tags', 'convert', 'settings'] as View[]).map((id) => (
            <button key={id} className={`flex items-center w-full min-h-9 px-3 text-left leading-5 rounded-sm ${view === id ? 'bg-[#1c1b18] text-[#f4f0e8]' : 'text-[#8a847a]'}`} onClick={() => setView(id)}>
              {id === 'radio' ? '電台' : id === 'tags' ? '標籤' : id === 'convert' ? '轉檔' : '資料夾設定'}
            </button>
          ))}
          <button className={`flex items-center w-full min-h-9 px-3 text-left leading-5 rounded-sm ${eqOpen ? 'text-[#e23b2f]' : 'text-[#8a847a]'}`} aria-pressed={eqOpen} onClick={() => setEqOpen((v) => !v)}>等化器{eqEnabled ? '' : '（關）'}</button>
        </nav>
        <main className="flex-1 min-w-0 flex flex-col">
          {!tool && (
            <div className="flex items-end gap-1 px-3 border-b border-[#2a2723] overflow-x-auto shrink-0">
              {tabs.map((tab) => {
                const label = tab.kind === 'playlist' ? (playlists.find((row) => row.id === tab.playlistId)?.name || tab.name) : tab.name;
                return (
                  <div key={tab.key} className="flex items-center shrink-0">
                    <button type="button" className={`tab ${tab.key === activeTabKey ? 'is-on' : ''}`} onClick={() => { void openTab(tab); }}>{label}</button>
                    {tab.kind !== 'all' && <button type="button" className="tab-x" aria-label={`關閉${label}`} title="關閉頁籤，不刪歌單" onClick={() => closeTab(tab.key)}>×</button>}
                  </div>
                );
              })}
            </div>
          )}
          {!tool && facetField && !facet && (
            <div className="overflow-y-auto">
              {facets.map((f) => (
                <button key={f.key} className="w-full text-left h-8 px-[18px] text-sm border-b border-[#221f1c] flex justify-between" onClick={() => openSongs(facetField, f.key)}>
                  <span>{f.key}</span><span className="num text-[#e23b2f]">{f.count}</span>
                </button>
              ))}
              {facets.length === 0 && <div className="p-6 text-sm text-[#8a847a]">還沒有可分的資料。先到資料夾設定掃描。</div>}
            </div>
          )}
          {!tool && !(facetField && !facet) && (
            <>
              <div className="px-5 py-3 border-b border-[#2a2723] flex flex-wrap gap-x-3 gap-y-2 items-center">
                {view === 'recent' && <span className="shrink-0 text-xs text-[#e23b2f]">最近播放，新的在上</span>}
                {view === 'playlist' && <span className="shrink-0 text-xs text-[#8a847a]">{smartList ? '智慧歌單，開頁籤時重算，不能拖曳' : query.trim() ? '篩選時不能排序' : '拖曳列可調整順序'}</span>}
                {view === 'queue' && <span className="shrink-0 text-xs text-[#8a847a]">拖曳列可調整佇列。下一首先播佇列</span>}
                {view === 'all' && searchNote && <span className="shrink-0 text-xs text-[#e23b2f]">{searchNote}</span>}
                <input id="filter" className="field flex-[1_1_180px] min-w-0" placeholder="篩選歌名、歌手、專輯，或搜尋曲庫" value={query} onChange={async (e) => {
                  const v = e.target.value;
                  setQuery(v);
                  if (view === 'all' && v.trim()) {
                    const { songs: found, total } = await searchSongs(v.trim(), 80);
                    setSongs(found);
                    setHasMore(false);
                    setSearchNote(total > found.length ? `搜尋到 ${total} 首，先顯示 ${found.length} 首` : '');
                  } else {
                    setSearchNote('');
                    if (view === 'all' && !v.trim()) loadAll(true);
                  }
                }} />
                <button type="button" className="round w-auto px-3 text-xs shrink-0" onClick={jumpToPlaying}>回到正在播放</button>
                {view === 'queue' && (
                  <>
                    <input className="field shrink-0" style={{ width: 112 }} aria-label="佇列歌單名稱" placeholder="歌單名稱" value={queueName} onChange={(e) => setQueueName(e.target.value)} />
                    <button type="button" className="round w-auto px-3 text-xs shrink-0" onClick={() => { void saveQueueAsPlaylist(); }}>存成歌單</button>
                    <button type="button" className="round w-auto px-3 text-xs shrink-0" onClick={clearQueue}>清空</button>
                  </>
                )}
                {view === 'playlist' && playlistId && (
                  <button type="button" className="round w-auto px-3 text-xs shrink-0" onClick={async () => {
                    try {
                      const pl = playlists.find((row) => row.id === playlistId);
                      const dest = await pickM3uSave(pl?.name || '歌單');
                      if (!dest) return;
                      await exportM3u(playlistId, dest);
                      setEngineError('');
                      setScan(`已匯出 ${dest}`);
                    } catch (e) {
                      setEngineError(e instanceof Error ? e.message : String(e));
                    }
                  }}>匯出 m3u</button>
                )}
                <button className="round w-auto px-3 text-xs shrink-0" onClick={async () => {
                  try {
                    const picked = await browseFolder();
                    if (!picked) return;
                    await addMusicFolder({ path: picked });
                    setScan(`正在掃描 ${picked}`);
                    const done = await scanFolders([picked]);
                    setScan(done.current_file || '掃描完成');
                    setView('all');
                    await loadAll(true);
                  } catch (e) {
                    setEngineError(String(e));
                  }
                }}>掃描資料夾</button>
              </div>
              <TrackTable
                songs={shown} activeId={activeId} playing={playing} selected={selected} sortKey={sortKey} sortDir={sortDir}
                onSort={(k) => {
                  const nextDir = sortKey !== k
                    ? (k === 'file_mtime' || k === 'play_count' ? 'desc' : 'asc')
                    : (sortDir === 'asc' ? 'desc' : 'asc');
                  setSortKey(k);
                  setSortDir(nextDir);
                  if (view === 'all') {
                    getSongs(400, 0, k, nextDir).then((page) => {
                      setSongs(page);
                      setOffset(page.length);
                      setHasMore(page.length === 400);
                    }).catch((e) => setEngineError(String(e)));
                  }
                }}
                onSelect={(song, e) => {
                  if (e.shiftKey && selected.length) {
                    const ids = shown.map((s) => s.id);
                    const a = ids.indexOf(selected[0]);
                    const b = ids.indexOf(song.id);
                    const [lo, hi] = a < b ? [a, b] : [b, a];
                    setSelected(ids.slice(lo, hi + 1));
                  } else if (e.ctrlKey) setSelected((cur) => cur.includes(song.id) ? cur.filter((id) => id !== song.id) : [...cur, song.id]);
                  else setSelected([song.id]);
                }}
                onPlay={(song) => playSong(song, shown)}
                onContext={(song, x, y) => setMenu({ song, x, y })}
                onNearEnd={() => { if (view === 'all' && hasMore && !query) loadAll(false); }}
                scrollNonce={scrollNonce}
                onReorder={smartList ? undefined : view === 'playlist' && !query.trim() ? reorderPlaylist : view === 'queue' && !query.trim() ? reorderQueue : undefined}
              />
            </>
          )}
          {view === 'radio' && <RadioView onPlay={setRadioTitle} />}
          {view === 'tags' && <TagView song={active} />}
          {view === 'convert' && <ConvertView song={active} />}
          {view === 'settings' && <SettingsView onScanned={() => loadAll(true)} />}
        </main>
        {lyricsOpen && (
        <aside className="w-[260px] shrink-0 border-l border-[#2a2723] overflow-y-auto overflow-x-hidden">
          {(
            <div className="px-5 py-4">
              <div className="text-[12px] tracking-widest text-[#e23b2f]">{catalog(activeId)}</div>
              <h2 className="text-[16px] leading-snug font-bold mt-2 mb-2 break-words">{playingName}</h2>
              {active?.path && <p className="text-[11px] leading-5 text-[#8a847a] break-all mb-4">{active.path}</p>}
              {lines.length > 0 && (
                <div className="flex flex-wrap gap-1 mb-3">
                  {[[-1000, '−1 秒'], [-100, '−0.1 秒'], [100, '+0.1 秒'], [1000, '+1 秒']].map(([delta, label]) => (
                    <button key={label} type="button" className="round w-auto px-2 text-[11px]" onClick={async () => {
                      if (!activeId) return;
                      const next = lyricOffset + Number(delta);
                      try {
                        await setLyricsOffset(activeId, next);
                        setLyricOffset(next);
                      } catch (e) {
                        setEngineError(e instanceof Error ? e.message : String(e));
                      }
                    }}>{label}</button>
                  ))}
                  <span className="num text-[11px] text-[#8a847a] self-center">{lyricOffset === 0 ? '未偏移' : `${lyricOffset > 0 ? '+' : ''}${(lyricOffset / 1000).toFixed(1)} 秒`}</span>
                </div>
              )}
              {lines.length === 0 && active && (
                <form className="mb-4 space-y-2" onSubmit={async (e) => {
                  e.preventDefault();
                  const parsed = parseLrc(lyricDraft);
                  if (!parsed.length) {
                    setEngineError('貼上的內容沒有時間標籤');
                    return;
                  }
                  try {
                    await saveLyrics(active.id, { song_id: active.id, source: 'paste', lines: parsed, offset_ms: lyricOffset });
                    setLines(parsed);
                    setEngineError('');
                  } catch (err) {
                    setEngineError(err instanceof Error ? err.message : String(err));
                  }
                }}>
                  <p className="text-sm text-[#8a847a]">這首沒有歌詞。可貼上 LRC。</p>
                  <textarea className="field min-h-24" aria-label="貼上歌詞" value={lyricDraft} onChange={(e) => setLyricDraft(e.target.value)} />
                  <button className="round w-auto px-3 text-xs" type="submit">存入歌詞</button>
                </form>
              )}
              {lines.length === 0 && !active && <p className="text-sm text-[#8a847a]">這首沒有歌詞。</p>}
              {lines.map((line, i) => (
                <button key={`${line.time_ms}-${i}`} data-lyric-now={i === lyricIndex ? '1' : undefined} className={`block text-left w-full py-0.5 text-sm ${i === lyricIndex ? 'lyric-now' : 'text-[#8a847a]'}`} onClick={() => { const ms = lyricAt(line); positionRef.current = ms; setPosition(ms); playerSeek(ms); }}>
                  {line.text}
                </button>
              ))}
            </div>
          )}
        </aside>
        )}
      </div>
      <footer className="shrink-0 border-t border-[#2a2723] px-4 py-2">
        <div className="flex items-center gap-3 min-w-0">
          <button className="round" aria-label="上一首" onClick={() => step(-1)}><SkipBack size={16} /></button>
          <button className="round" aria-label={playing ? '暫停' : '播放'} onClick={() => { if (playing) { playerPause().catch((e) => setEngineError(String(e))); void flushNowRef.current().catch((e) => setEngineError(String(e))); } else playerPlay().catch((e) => setEngineError(String(e))); }}>{playing ? <Pause size={16} /> : <Play size={16} />}</button>
          <button className="round" aria-label="下一首" onClick={() => step(1)}><SkipForward size={16} /></button>
          <div className="flex-1 flex items-center gap-3 min-w-0">
            <span className="num text-xs text-[#8a847a] w-12 shrink-0">{radioTitle ? '直播' : fmt(position)}</span>
            <input aria-label="播放位置" type="range" min={0} max={Math.max(duration, 1)} value={Math.min(position, duration || 1)} disabled={!!radioTitle} style={{ ['--pct' as string]: `${duration > 0 ? Math.min(100, (position / duration) * 100) : 0}%` }} onChange={(e) => { const ms = Number(e.target.value); setPosition(ms); positionRef.current = ms; playerSeek(ms); }} className="line flex-1 min-w-0 w-0" />
            <span className="num text-xs text-[#8a847a] w-12 shrink-0">{radioTitle ? '' : fmt(duration)}</span>
          </div>
          <div className="flex items-center gap-1.5 shrink-0">
            <button type="button" className="round" aria-label={modeInfo.title} title={modeInfo.title} onClick={cycleMode} style={mode === 'Sequence' ? { color: '#8a847a' } : { color: '#e23b2f', borderColor: '#e23b2f' }}>
              {mode === 'Random' ? <Shuffle size={15} /> : mode === 'Single' ? <Repeat1 size={15} /> : <Repeat size={15} />}
            </button>
            <span className="text-[11px] w-8 shrink-0" style={{ color: mode === 'Sequence' ? '#8a847a' : '#e23b2f' }}>{modeInfo.short}</span>
          </div>
          <div className="flex items-center gap-1.5 shrink-0" onWheel={(e) => setSliderVolume((muted ? 0 : volume) + (e.deltaY < 0 ? 0.05 : -0.05))}>
            <button type="button" className="round" aria-label={muted ? '取消靜音' : '靜音'} aria-pressed={muted} title={muted ? '取消靜音' : '靜音'} onClick={toggleMute} style={muted || volume === 0 ? { color: '#e23b2f', borderColor: '#e23b2f' } : undefined}>
              {muted || volume === 0 ? <VolumeX size={15} /> : <Volume2 size={15} />}
            </button>
            <input aria-label="音量" aria-valuemin={0} aria-valuemax={100} aria-valuenow={volumePct} aria-valuetext={muted || volume === 0 ? '靜音' : `${volumePct}%`} type="range" min={0} max={1} step={0.01} value={muted ? 0 : volume} className="line w-28" style={{ ['--pct' as string]: `${volumePct}%` }} onChange={(e) => setSliderVolume(Number(e.target.value))} />
            <span className="num text-[11px] w-9 shrink-0 text-right text-[#8a847a]">{muted || volume === 0 ? '靜音' : `${volumePct}%`}</span>
          </div>
        </div>
        <div className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1.5 min-w-0">
        <div className="flex-[1_1_320px] min-w-0 text-[12px] leading-5 text-[#c8c2b8] break-all" title={active?.path || radioTitle || ''}>
          {radioTitle ? `直播　${radioTitle}` : (active ? `${techLine(active)}　${active.path}` : '尚未選擇檔案')}
        </div>
        <div className="flex items-center gap-2 min-w-0 ml-auto">
          <select className="field text-[11px] min-w-[140px]" style={{ width: 260, flex: '0 1 260px' }} value={outputDevice} onChange={(e) => { void playerSetOutput(e.target.value).catch((err) => setEngineError(String(err))); setOutputDevice(e.target.value); }} disabled={outputDevices.length === 0}>
            {outputDevices.map((d) => (<option key={d} value={d}>{d}</option>))}
          </select>
          <button type="button" className="round w-auto px-3 text-xs shrink-0" onClick={() => { playerOutputDevices().then((ds) => { setOutputDevices(ds); if (ds.length && !outputDevice) setOutputDevice(ds[0]); }).catch(() => {}); }}>重整裝置</button>
          <button type="button" className="round w-auto px-3 text-xs shrink-0" onClick={() => { void playerSetDefaultPlayer(!defaultPlayer).then(() => setDefaultPlayer(!defaultPlayer)).catch((err) => setEngineError(String(err))); }}>{defaultPlayer ? '取消預設播放器' : '設為預設播放器'}</button>
          <button type="button" className="round w-auto px-3 text-xs shrink-0" onClick={() => {
            if (!lyricsOpen) {
              setLyricsOpen(true);
              saveConfig('lyrics_open', '1').catch(() => {});
            } else {
              setLyricsOpen(false);
              saveConfig('lyrics_open', '0').catch(() => {});
            }
          }}>{lyricsOpen ? '收歌詞' : '歌詞'}</button>
          <button type="button" className={`round w-auto px-3 text-xs shrink-0 ${sleepSeconds > 0 ? 'text-[#e23b2f]' : ''}`} onClick={() => {
            const m = window.prompt('睡眠計時：幾分鐘後暫停（輸入 0 取消）', sleepSeconds > 0 ? String(Math.round(sleepSeconds / 60)) : '');
            const n = Number(m);
            if (!Number.isFinite(n)) return;
            setStopAfterCurrent(false);
            setSleepSeconds(n > 0 ? n * 60 : 0);
          }}>{sleepSeconds > 0 ? `睡眠 ${Math.ceil(sleepSeconds / 60)} 分鐘` : '睡眠計時'}</button>
          <button type="button" className={`round w-auto px-3 text-xs shrink-0 ${stopAfterCurrent ? 'text-[#e23b2f]' : ''}`} onClick={() => {
            setStopAfterCurrent(!stopAfterCurrent);
            if (!stopAfterCurrent) setSleepSeconds(0);
          }} title="這首播完不再播下一首">{stopAfterCurrent ? '結束後不播下一首 ✔' : '結束後不播下一首'}</button>
        </div>
        </div>
      </footer>
      {menu && (() => {
        const targets = actedSongs(menu.song);
        const many = targets.length > 1 ? `（${targets.length} 首）` : '';
        return (
        <div className="fixed z-20 bg-[#1c1b18] border border-[#3a3632] text-sm min-w-[200px]" style={{ left: menu.x, top: menu.y }} onClick={(e) => e.stopPropagation()}>
          {targets.length > 1 && (
            <form className="px-3 py-2 space-y-2" onSubmit={async (e) => {
              e.preventDefault();
              const artist = batchDraft.artist.trim() || undefined;
              const album = batchDraft.album.trim() || undefined;
              const genre = batchDraft.genre.trim() || undefined;
              const year = batchDraft.year.trim() ? Number(batchDraft.year) : undefined;
              if (!artist && !album && !genre && year === undefined) {
                setEngineError('沒有要改的欄位');
                return;
              }
              try {
                const result = await writeTagsBatch({ song_ids: targets.map((s) => s.id), artist, album, genre, year });
                setEngineError('');
                setScan(`批次標籤：${result.updated} 首已改，${result.failed.length} 首失敗`);
                if (result.failed.length) setEngineError(result.failed.join('；'));
                setMenu(null);
              } catch (e) {
                setEngineError(e instanceof Error ? e.message : String(e));
              }
            }}>
              <div className="text-xs text-[#8a847a] mb-1">批次標籤（${targets.length} 首）</div>
              <input className="field" placeholder="歌手（空白不改）" value={batchDraft.artist} onChange={(e) => setBatchDraft({ ...batchDraft, artist: e.target.value })} />
              <input className="field" placeholder="專輯（空白不改）" value={batchDraft.album} onChange={(e) => setBatchDraft({ ...batchDraft, album: e.target.value })} />
              <input className="field" placeholder="曲風（空白不改）" value={batchDraft.genre} onChange={(e) => setBatchDraft({ ...batchDraft, genre: e.target.value })} />
              <input className="field" placeholder="年份（空白不改）" value={batchDraft.year} onChange={(e) => setBatchDraft({ ...batchDraft, year: e.target.value })} />
              <button className="block w-full text-left text-[#e23b2f]" type="submit">套用批次標籤</button>
            </form>
          )}
          {targets.length === 1 && (
            <button className="block w-full text-left px-3 py-2 hover:text-[#e23b2f]" onClick={() => { playSong(targets[0], targets); setMenu(null); }}>播放{many}</button>
          )}
          <button className="block w-full text-left px-3 py-2 hover:text-[#e23b2f]" onClick={() => { putQueue(targets, 'next'); setMenu(null); }}>下一首播放{many}</button>
          <button className="block w-full text-left px-3 py-2 hover:text-[#e23b2f]" onClick={() => { putQueue(targets, 'end'); setMenu(null); }}>加入佇列{many}</button>
          {userPlaylists.map((pl) => (
            <button key={pl.id} className="block w-full text-left px-3 py-2 hover:text-[#e23b2f]" onClick={async () => {
              try {
                for (const song of targets) await addSongToPlaylist({ playlist_id: pl.id, song_id: song.id, position: 0 });
                if (playlistId === pl.id) await openPlaylist(pl.id);
              } catch (e) {
                setEngineError(e instanceof Error ? e.message : String(e));
              }
              setMenu(null);
            }}>加入「{pl.name}」{many}</button>
          ))}
          {view === 'playlist' && targets.some((song) => itemIds[song.id]) && (
            <button className="block w-full text-left px-3 py-2 hover:text-[#e23b2f]" onClick={async () => {
              try {
                for (const song of targets) {
                  const itemId = itemIds[song.id];
                  if (itemId) await removeFromPlaylist(itemId);
                }
                if (playlistId) await openPlaylist(playlistId);
              } catch (e) {
                setEngineError(e instanceof Error ? e.message : String(e));
              }
              setMenu(null);
            }}>從歌單移除{many}</button>
          )}
          <button className="block w-full text-left px-3 py-2 hover:text-[#e23b2f]" onClick={() => { revealInExplorer(menu.song.path).catch((e) => setEngineError(String(e))); setMenu(null); }}>在檔案總管顯示</button>
          <button className="block w-full text-left px-3 py-2 hover:text-[#e23b2f]" onClick={() => { setActiveId(menu.song.id); setView('tags'); setMenu(null); }}>編輯標籤</button>
        </div>
        );
      })()}
      {plMenu && (
        <div className="fixed z-20 bg-[#1c1b18] border border-[#3a3632] text-sm min-w-[200px]" style={{ left: plMenu.x, top: plMenu.y }} onClick={(e) => e.stopPropagation()}>
          <form className="px-3 py-2 space-y-2" onSubmit={async (e) => {
            e.preventDefault();
            const name = renameDraft.trim();
            if (!name) {
              setEngineError('歌單名稱不能是空的');
              return;
            }
            try {
              await updatePlaylist(plMenu.id, name);
              setPlaylists(await listPlaylists());
              setPlMenu(null);
            } catch (err) {
              setEngineError(err instanceof Error ? err.message : String(err));
            }
          }}>
            <input className="field" aria-label="歌單名稱" value={renameDraft} onChange={(e) => setRenameDraft(e.target.value)} />
            <button className="block w-full text-left text-[#e23b2f]" type="submit">改名</button>
          </form>
          <button className="block w-full text-left px-3 py-2 hover:text-[#e23b2f]" onClick={async () => {
            if (!confirmDelete) { setConfirmDelete(true); return; }
            try {
              await deletePlaylist(plMenu.id);
              const key = `pl-${plMenu.id}`;
              setTabs((cur) => {
                const next = cur.filter((tab) => tab.key !== key);
                const kept = next.length ? next : [ALL_TAB];
                persistTabs(kept);
                return kept;
              });
              setPlMenu(null);
              setPlaylists(await listPlaylists());
              setView('all');
              loadAll(true);
            } catch (err) {
              setEngineError(err instanceof Error ? err.message : String(err));
            }
          }}>刪除歌單「{plMenu.name}」{confirmDelete ? '：再按一次才刪' : ''}</button>
        </div>
      )}
      {eqOpen && !mini && (
        <EqPanel
          bands={eq} preamp={preamp} enabled={eqEnabled} normalize={normalize} crossfade={crossfade} gain={gain}
          presets={eqPresets} selectedPreset={selectedPreset} presetDraft={presetDraft}
          onBands={changeBands} onPreamp={changePreamp} onEnabled={changeEqEnabled}
          onNormalize={changeNormalize} onCrossfade={changeCrossfade}
          onApplyPreset={applyPreset} onSavePreset={savePreset} onDeletePreset={deletePreset}
          onPresetNameChange={setPresetDraft} onClose={closeEq}
          fx={fx} onFx={changeFx} onScene={applyScene} isRadio={!!radioTitle}
          song={active} auto={autoEq} onAuto={changeAuto} autoNote={autoNote}
        />
      )}
    </div>
  );
}
