import { EQ_BANDS, flatBands, migrateBands, type EqPreset, type PresetBind } from './eq.ts';
export { parseBind, type PresetBind } from './eq.ts';

/** 與 src-tauri/src/dsp.rs 的 FxParams 對應（camelCase）。 */
export type Fx = {
  speed: number;
  tempo: number;
  pitch: number;
  bassOn: boolean;
  bassDb: number;
  bassHz: number;
  enhancerOn: boolean;
  enhancer: number;
  voiceRemover: boolean;
  chorusOn: boolean;
  chorusDepth: number;
  chorusRate: number;
  chorusMix: number;
  flangerOn: boolean;
  flangerDepth: number;
  flangerRate: number;
  flangerFeedback: number;
  flangerMix: number;
  echoOn: boolean;
  echoMs: number;
  echoFeedback: number;
  echoMix: number;
  reverbOn: boolean;
  reverbRoom: number;
  reverbDamp: number;
  reverbMix: number;
};

export const FX_DEFAULT: Fx = {
  speed: 1, tempo: 1, pitch: 0,
  bassOn: false, bassDb: 6, bassHz: 100,
  enhancerOn: false, enhancer: 0.3,
  voiceRemover: false,
  chorusOn: false, chorusDepth: 0.5, chorusRate: 1, chorusMix: 0.5,
  flangerOn: false, flangerDepth: 0.6, flangerRate: 0.3, flangerFeedback: 0.5, flangerMix: 0.5,
  echoOn: false, echoMs: 300, echoFeedback: 0.35, echoMix: 0.35,
  reverbOn: false, reverbRoom: 0.5, reverbDamp: 0.5, reverbMix: 0.3,
};

/** 範圍與 dsp.rs `sanitized` 相同。 */
const RANGE: Partial<Record<keyof Fx, [number, number]>> = {
  speed: [0.5, 2], tempo: [0.5, 2], pitch: [-12, 12],
  bassDb: [0, 15], bassHz: [40, 250], enhancer: [0, 1],
  chorusDepth: [0, 1], chorusRate: [0.1, 5], chorusMix: [0, 1],
  flangerDepth: [0, 1], flangerRate: [0.05, 2], flangerFeedback: [0, 0.9], flangerMix: [0, 1],
  echoMs: [20, 2000], echoFeedback: [0, 0.9], echoMix: [0, 1],
  reverbRoom: [0, 1], reverbDamp: [0, 1], reverbMix: [0, 1],
};

export function parseFx(raw: unknown): Fx {
  const out: Fx = { ...FX_DEFAULT };
  if (!raw || typeof raw !== 'object') return out;
  const row = raw as Record<string, unknown>;
  for (const key of Object.keys(FX_DEFAULT) as Array<keyof Fx>) {
    const v = row[key];
    if (typeof FX_DEFAULT[key] === 'boolean') {
      (out as Record<string, unknown>)[key] = v === true;
    } else if (typeof v === 'number' && Number.isFinite(v)) {
      const [lo, hi] = RANGE[key] ?? [-Infinity, Infinity];
      (out as Record<string, unknown>)[key] = Math.min(hi, Math.max(lo, v));
    }
  }
  return out;
}

export function fxActive(fx: Fx): string[] {
  const on: string[] = [];
  if (fx.speed !== 1) on.push('速度');
  if (fx.tempo !== 1) on.push('節奏');
  if (fx.pitch !== 0) on.push('音高');
  if (fx.bassOn) on.push('低音');
  if (fx.enhancerOn) on.push('高音增強');
  if (fx.voiceRemover) on.push('人聲消除');
  if (fx.chorusOn) on.push('合唱');
  if (fx.flangerOn) on.push('鑲邊');
  if (fx.echoOn) on.push('回音');
  if (fx.reverbOn) on.push('殘響');
  return on;
}

/* ---------- 依曲風建議 ---------- */

/** 曲風標籤 → 建議的等化器預設名稱。比對不分大小寫、取第一個命中；具體的寫前面（hip-hop 先於 pop）。 */
const GENRE_RULES: Array<[string, string[]]> = [
  ['現場', ['live', '現場']],
  ['重低音', ['hip-hop', 'hip hop', 'hiphop', 'rap', 'r&b', 'rnb', 'trap', 'reggae', 'drum & bass', 'dnb', '嘻哈', '饒舌']],
  ['電子', ['electronic', 'electronica', 'edm', 'dance', 'house', 'techno', 'trance', 'dubstep', 'electro', 'club', '電子', '舞曲']],
  ['搖滾', ['rock', 'metal', 'punk', 'grunge', 'alternative', '搖滾', '金屬', '龐克', '另類']],
  ['古典', ['classical', 'orchestra', 'symphony', 'opera', 'baroque', 'chamber', 'piano', 'soundtrack', 'score', '古典', '交響', '歌劇', '鋼琴', '原聲']],
  ['爵士', ['jazz', 'blues', 'swing', 'soul', 'funk', 'bossa', '爵士', '藍調', '靈魂']],
  ['柔和', ['ambient', 'new age', 'lounge', 'chill', 'easy listening', 'lo-fi', 'lofi', '輕音樂', '新世紀', '放鬆']],
  ['人聲', ['vocal', 'ballad', 'folk', 'acoustic', 'singer-songwriter', 'country', 'a cappella', '民謠', '抒情', '人聲', '清唱']],
  ['流行', ['pop', 'mandopop', 'cantopop', 'j-pop', 'k-pop', 'c-pop', 'anime', '流行', '華語', '國語', '粵語', '動漫']],
];

export function suggestForGenre(genre: string | null | undefined): string | null {
  const g = (genre || '').trim().toLowerCase();
  if (!g) return null;
  for (const [preset, keys] of GENRE_RULES) {
    if (keys.some((k) => g.includes(k))) return preset;
  }
  return null;
}

/* ---------- 參考播放器 式自動套用 ---------- */

export type AutoEq = { enabled: boolean; fallback: string };
export type SongKey = { path?: string | null; album?: string | null; genre?: string | null };
export type AutoPick = { preset: EqPreset; reason: string } | null;

const same = (a: string | null | undefined, b: string | null | undefined) =>
  !!a && !!b && a.trim().toLowerCase() === b.trim().toLowerCase();

/** 順序與 參考播放器 相同：綁這首檔案 → 綁這張專輯 → 綁這個曲風 → 依曲風的內建建議 → 預設回退。 */
export function pickAutoPreset(song: SongKey | null, presets: EqPreset[], auto: AutoEq): AutoPick {
  if (!auto.enabled || !song) return null;
  const by = (kind: PresetBind['kind'], value: string | null | undefined) =>
    presets.find((p) => p.bind?.kind === kind && same(p.bind.value, value));
  const file = by('file', song.path);
  if (file) return { preset: file, reason: '綁定這首' };
  const album = by('album', song.album);
  if (album) return { preset: album, reason: `綁定專輯「${song.album}」` };
  const genre = by('genre', song.genre);
  if (genre) return { preset: genre, reason: `綁定曲風「${song.genre}」` };
  const name = suggestForGenre(song.genre);
  const built = name ? presets.find((p) => p.name === name) : undefined;
  if (built) return { preset: built, reason: `依曲風「${song.genre}」建議` };
  const fb = auto.fallback ? presets.find((p) => p.name === auto.fallback) : undefined;
  if (fb) return { preset: fb, reason: '沒有符合的，用預設回退' };
  return null;
}

export function parseAuto(raw: string | null | undefined): AutoEq {
  try {
    const v = JSON.parse(raw || '{}') as Record<string, unknown>;
    return { enabled: v.enabled === true, fallback: typeof v.fallback === 'string' ? v.fallback : '' };
  } catch {
    return { enabled: false, fallback: '' };
  }
}

/* ---------- 情境建議（等化器＋音效一起套） ---------- */

export type Scene = { id: string; name: string; why: string; bands: number[]; preamp: number; fx: Partial<Fx> };

/** 情境用 10 段寫，與舊資料同一套換算成 18 段。 */
const b18 = (ten: number[]) => migrateBands(ten) ?? flatBands();

export const SCENES: Scene[] = [
  {
    id: 'night', name: '夜間小音量',
    why: '音量小時耳朵對低頻和高頻比較不敏感，補回兩端，人聲不變。前級降 3 dB 避免削波。',
    bands: b18([5, 4, 1, 0, -1, -1, 0, 2, 4, 5]), preamp: -3, fx: {},
  },
  {
    id: 'headphone', name: '耳機長時間聆聽',
    why: '稍降 2.8k–5.6k 容易刺耳的區段，加一點點小房間殘響，減少「聲音在腦袋裡」的悶感。',
    bands: b18([1, 1, 0, 0, 0, 0, -1, -2, -1, 0]), preamp: 0,
    fx: { reverbOn: true, reverbRoom: 0.3, reverbDamp: 0.6, reverbMix: 0.12 },
  },
  {
    id: 'concert', name: '演唱會現場',
    why: '中頻往前、加大廳殘響，模擬在場館裡聽。',
    bands: b18([-2, 0, 2, 3, 3, 3, 2, 1, 1, 1]), preamp: -3,
    fx: { reverbOn: true, reverbRoom: 0.85, reverbDamp: 0.3, reverbMix: 0.35 },
  },
  {
    id: 'karaoke', name: 'KTV 伴唱',
    why: '消掉錄在正中央的主唱（低音保留），加一點殘響。立體聲混音才有效，現場錄音效果有限。',
    bands: flatBands(), preamp: 0,
    fx: { voiceRemover: true, reverbOn: true, reverbRoom: 0.5, reverbMix: 0.25 },
  },
  {
    id: 'speech', name: '人聲清晰（Podcast／有聲書）',
    why: '收掉 125Hz 以下的轟聲，把 1k–4k 的咬字區往前推，稍微加高音增強。',
    bands: b18([-6, -4, -2, 0, 1, 3, 4, 3, 1, 0]), preamp: -2,
    fx: { enhancerOn: true, enhancer: 0.2 },
  },
  {
    id: 'party', name: '重低音派對',
    why: '18 段的低頻加上 80Hz 低音增強。前級降 6 dB，否則鼓點會削波。',
    bands: b18([6, 5, 3, 1, 0, 0, 0, 1, 2, 2]), preamp: -6,
    fx: { bassOn: true, bassDb: 6, bassHz: 80 },
  },
  {
    id: 'vintage', name: '老錄音修飾',
    why: '切掉 63Hz 以下的嗡聲與唱盤轟聲，用高音增強補回被壓縮掉的空氣感。',
    bands: b18([-8, -4, 0, 0, 0, 0, 1, 1, 1, 0]), preamp: 0,
    fx: { enhancerOn: true, enhancer: 0.45 },
  },
  {
    id: 'practice', name: '慢速練習',
    why: '節奏 0.8 倍、音高不變，跟著練樂器或聽寫用。電台不適用。',
    bands: flatBands(), preamp: 0, fx: { tempo: 0.8 },
  },
  {
    id: 'reset', name: '全部還原',
    why: '等化器歸零、所有音效關閉、速度／節奏／音高回原值。',
    bands: flatBands(), preamp: 0, fx: {},
  },
];

export function sceneFx(scene: Scene): Fx {
  return { ...FX_DEFAULT, ...scene.fx };
}

export { EQ_BANDS };
