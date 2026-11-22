import type { PlayMode } from '../types';

export type Session = {
  volume: number;
  muted: boolean;
  mode: PlayMode;
  eq: number[];
  preamp: number;
  normalize: boolean;
  crossfade: number;
  songId: number | null;
  positionMs: number;
  queueIds: number[];
};

const MODES: PlayMode[] = ['Sequence', 'Single', 'All', 'Random'];

export function emptySession(): Session {
  return {
    volume: 0.7,
    muted: false,
    mode: 'Sequence',
    eq: Array(10).fill(0),
    preamp: 0,
    normalize: false,
    crossfade: 0,
    songId: null,
    positionMs: 0,
    queueIds: [],
  };
}

function clamp(n: number, lo: number, hi: number) {
  return Math.min(hi, Math.max(lo, n));
}

/** 壞掉的 JSON 當成沒有進度，不要把音量或模式解成 NaN。 */
export function parseSession(raw: string | null | undefined): Session {
  const base = emptySession();
  if (!raw) return base;
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return base;
  }
  if (!value || typeof value !== 'object') return base;
  const row = value as Record<string, unknown>;
  const volume = typeof row.volume === 'number' && Number.isFinite(row.volume) ? clamp(row.volume, 0, 1) : base.volume;
  const mode = MODES.includes(row.mode as PlayMode) ? (row.mode as PlayMode) : base.mode;
  const eq = Array.from({ length: 10 }, (_, i) => {
    const n = Array.isArray(row.eq) ? Number(row.eq[i]) : 0;
    return Number.isFinite(n) ? clamp(n, -12, 12) : 0;
  });
  const preamp = typeof row.preamp === 'number' && Number.isFinite(row.preamp)
    ? clamp(row.preamp, -12, 12) : 0;
  const crossfade = typeof row.crossfade === 'number' && Number.isFinite(row.crossfade)
    ? Math.round(clamp(row.crossfade, 0, 12000))
    : 0;
  const songId = typeof row.songId === 'number' && row.songId > 0 ? row.songId : null;
  const positionMs = typeof row.positionMs === 'number' && row.positionMs > 0 ? Math.round(row.positionMs) : 0;
  const queueIds = Array.isArray(row.queueIds)
    ? row.queueIds.filter((id): id is number => typeof id === 'number' && id > 0).slice(0, 500)
    : [];
  return {
    volume,
    muted: row.muted === true || volume === 0,
    mode,
    eq,
    preamp,
    normalize: row.normalize === true,
    crossfade,
    songId,
    positionMs,
    queueIds,
  };
}
