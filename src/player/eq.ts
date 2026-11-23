/** 與 src-tauri/src/eq.rs 的 EQ_HZ 相同。改段數時兩邊一起改。 */
export const EQ_HZ = [31, 63, 87, 125, 175, 250, 350, 500, 700, 1000, 1400, 2000, 2800, 4000, 5600, 8000, 11200, 16000];
export const EQ_LABELS = ['31', '63', '87', '125', '175', '250', '350', '500', '700', '1K', '1.4K', '2K', '2.8K', '4K', '5.6K', '8K', '11K', '16K'];
export const EQ_BANDS = EQ_HZ.length;
export const EQ_LIMIT = 15;

/** 舊版 10 段的中心頻率，用來把舊工作階段與舊預設換算成 18 段。 */
const OLD_HZ = [32, 64, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];

/** 參考播放器 式綁定：播到這個檔案／專輯／曲風時自動套用這個預設。 */
export type PresetBind = { kind: 'file' | 'album' | 'genre'; value: string };
export type EqPreset = { name: string; bands: number[]; preamp?: number; bind?: PresetBind };

export function parseBind(raw: unknown): PresetBind | undefined {
  if (!raw || typeof raw !== 'object') return undefined;
  const r = raw as Record<string, unknown>;
  if ((r.kind === 'file' || r.kind === 'album' || r.kind === 'genre') && typeof r.value === 'string' && r.value.trim()) {
    return { kind: r.kind, value: r.value };
  }
  return undefined;
}

export function clampDb(n: unknown): number {
  const v = Number(n);
  if (!Number.isFinite(v)) return 0;
  return Math.round(Math.min(EQ_LIMIT, Math.max(-EQ_LIMIT, v)) * 10) / 10;
}

export function flatBands(): number[] {
  return Array(EQ_BANDS).fill(0);
}

/** 18 段直接夾範圍；10 段依對數頻率內插成 18 段；其他長度視為壞資料。 */
export function migrateBands(raw: unknown): number[] | null {
  if (!Array.isArray(raw)) return null;
  if (raw.length === EQ_BANDS) return raw.map(clampDb);
  if (raw.length !== OLD_HZ.length) return null;
  const old = raw.map(clampDb);
  return EQ_HZ.map((f) => {
    if (f <= OLD_HZ[0]) return old[0];
    if (f >= OLD_HZ[OLD_HZ.length - 1]) return old[old.length - 1];
    let i = 0;
    while (OLD_HZ[i + 1] < f) i += 1;
    const t = Math.log(f / OLD_HZ[i]) / Math.log(OLD_HZ[i + 1] / OLD_HZ[i]);
    return clampDb(old[i] + (old[i + 1] - old[i]) * t);
  });
}

export function parsePresets(raw: string | null | undefined): EqPreset[] {
  let value: unknown;
  try {
    value = JSON.parse(raw || '[]');
  } catch {
    return [];
  }
  if (!Array.isArray(value)) return [];
  const out: EqPreset[] = [];
  for (const row of value) {
    if (!row || typeof row !== 'object') continue;
    const r = row as Record<string, unknown>;
    const name = typeof r.name === 'string' ? r.name.trim() : '';
    const bands = migrateBands(r.bands);
    if (!name || !bands || out.some((p) => p.name === name)) continue;
    const preset: EqPreset = { name, bands };
    if (typeof r.preamp === 'number' && Number.isFinite(r.preamp)) preset.preamp = clampDb(r.preamp);
    const bind = parseBind(r.bind);
    if (bind) preset.bind = bind;
    out.push(preset);
  }
  return out;
}

/** 種子值是這個播放器自己調的曲線，不是 參考播放器 內建預設。以 10 段寫，載入時換算成 18 段。 */
const SEED_10: Array<[string, number[]]> = [
  ['平直', [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]],
  ['人聲', [-2, -1, 0, 2, 3, 3, 2, 1, 0, -1]],
  ['重低音', [6, 5, 4, 2, 0, 0, 0, 0, 0, 0]],
  ['古典', [0, 0, 0, 0, 0, 0, -2, -3, -3, -4]],
  ['流行', [-1, 1, 3, 4, 3, 0, -1, -1, -1, -1]],
  ['搖滾', [5, 3, -2, -4, -2, 1, 3, 5, 6, 6]],
  ['爵士', [3, 2, 1, 2, -1, -1, 0, 1, 2, 3]],
  ['電子', [5, 4, 1, 0, -2, 1, 0, 1, 4, 5]],
  ['現場', [-3, 0, 2, 3, 3, 3, 2, 1, 1, 1]],
  ['柔和', [2, 1, 0, 0, -1, -1, -2, -3, -4, -4]],
  ['高音增強', [0, 0, 0, 0, 0, 1, 3, 5, 7, 8]],
  ['低音與高音', [6, 4, 0, -2, -2, 0, 1, 4, 6, 7]],
];

export const SEED_VERSION = '2';

export function seedPresets(): EqPreset[] {
  return SEED_10.map(([name, bands]) => ({ name, bands: migrateBands(bands) ?? flatBands() }));
}

/** 升級種子時只補「沒有同名」的種子；使用者覆寫過的同名預設不動，刪掉的舊種子名稱也不會被補回（第一版只有三個）。 */
export function mergeSeeds(current: EqPreset[], seededVersion: string | null): { presets: EqPreset[]; changed: boolean } {
  if (seededVersion === SEED_VERSION) return { presets: current, changed: false };
  const firstRun = current.length === 0 && seededVersion === null;
  const legacy = new Set(['平直', '人聲', '重低音']);
  const add = seedPresets().filter((s) => !current.some((p) => p.name === s.name) && (firstRun || !legacy.has(s.name)));
  return { presets: [...current, ...add], changed: true };
}
