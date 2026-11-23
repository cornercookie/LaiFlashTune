import type { PlayMode } from '../types';
import { EQ_LIMIT, clampDb, flatBands, migrateBands } from './eq.ts';
import { FX_DEFAULT, parseFx, type Fx } from './fx.ts';

export type Session = {
  volume: number;
  muted: boolean;
  mode: PlayMode;
  eq: number[];
  eqEnabled: boolean;
  fx: Fx;
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
    eq: flatBands(),
    eqEnabled: true,
    fx: { ...FX_DEFAULT },
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
  // 舊版存 10 段，換算成 18 段，不要把使用者的曲線歸零。
  const eq = migrateBands(row.eq) ?? flatBands();
  const preamp = typeof row.preamp === 'number' && Number.isFinite(row.preamp)
    ? clampDb(clamp(row.preamp, -EQ_LIMIT, EQ_LIMIT)) : 0;
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
    eqEnabled: row.eqEnabled !== false,
    fx: parseFx(row.fx),
    preamp,
    normalize: row.normalize === true,
    crossfade,
    songId,
    positionMs,
    queueIds,
  };
}
