import assert from 'node:assert/strict';
import { FX_DEFAULT, SCENES, fxActive, parseFx, pickAutoPreset, sceneFx, suggestForGenre, parseAuto } from './fx.ts';
import { parsePresets, seedPresets } from './eq.ts';
import { parseSession } from './session.ts';

// 參數夾在 dsp.rs 相同範圍，壞值回預設。
const fx = parseFx({ speed: 9, tempo: 'x', pitch: -30, reverbOn: true, echoMs: 5 });
assert.equal(fx.speed, 2);
assert.equal(fx.tempo, 1);
assert.equal(fx.pitch, -12);
assert.equal(fx.reverbOn, true);
assert.equal(fx.echoMs, 20);
assert.deepEqual(parseFx(null), FX_DEFAULT);
assert.deepEqual(fxActive(FX_DEFAULT), []);
assert.deepEqual(fxActive({ ...FX_DEFAULT, tempo: 0.8, reverbOn: true }), ['節奏', '殘響']);

// 曲風建議：不分大小寫、中英文都認，hip-hop 不會被當成 pop。
assert.equal(suggestForGenre('Rock'), '搖滾');
assert.equal(suggestForGenre('Pop Rock'), '搖滾');
assert.equal(suggestForGenre('Hip-Hop'), '重低音');
assert.equal(suggestForGenre('K-Pop'), '流行');
assert.equal(suggestForGenre('華語流行'), '流行');
assert.equal(suggestForGenre('Classical'), '古典');
assert.equal(suggestForGenre('民謠'), '人聲');
assert.equal(suggestForGenre(''), null);
assert.equal(suggestForGenre('Polka'), null);

// 自動套用順序：檔案 > 專輯 > 曲風綁定 > 內建建議 > 回退。
const presets = [
  ...seedPresets(),
  { name: '這首', bands: Array(18).fill(1), bind: { kind: 'file', value: 'C:/m/a.flac' } },
  { name: '這張', bands: Array(18).fill(2), bind: { kind: 'album', value: 'Blue' } },
  { name: '我的搖滾', bands: Array(18).fill(3), bind: { kind: 'genre', value: 'rock' } },
];
const on = { enabled: true, fallback: '平直' };
assert.equal(pickAutoPreset({ path: 'c:/M/A.flac', album: 'Blue', genre: 'Rock' }, presets, on).preset.name, '這首');
assert.equal(pickAutoPreset({ path: 'x', album: 'blue', genre: 'Rock' }, presets, on).preset.name, '這張');
assert.equal(pickAutoPreset({ path: 'x', album: 'y', genre: 'ROCK' }, presets, on).preset.name, '我的搖滾');
assert.equal(pickAutoPreset({ path: 'x', album: 'y', genre: 'Jazz' }, presets, on).preset.name, '爵士');
const fb = pickAutoPreset({ path: 'x', album: 'y', genre: 'Polka' }, presets, on);
assert.equal(fb.preset.name, '平直');
assert.equal(pickAutoPreset({ path: 'x', genre: 'Polka' }, presets, { enabled: true, fallback: '' }), null, 'no fallback keeps current curve');
assert.equal(pickAutoPreset({ path: 'c:/m/a.flac' }, presets, { enabled: false, fallback: '' }), null, 'off means off');
assert.deepEqual(parseAuto('garbage'), { enabled: false, fallback: '' });

// 綁定隨預設存讀。
const saved = parsePresets(JSON.stringify(presets));
assert.deepEqual(saved.find((p) => p.name === '這張').bind, { kind: 'album', value: 'Blue' });

// 情境：每個都是 18 段，音效從預設值開始覆寫（不殘留上一個情境的殘響）。
for (const s of SCENES) {
  assert.equal(s.bands.length, 18, s.name);
  assert.ok(s.why.length > 10, s.name);
}
assert.equal(sceneFx(SCENES.find((s) => s.id === 'practice')).tempo, 0.8);
assert.equal(sceneFx(SCENES.find((s) => s.id === 'practice')).reverbOn, false);
assert.deepEqual(sceneFx(SCENES.find((s) => s.id === 'reset')), FX_DEFAULT);

// session 帶 fx。
assert.equal(parseSession(JSON.stringify({ fx: { tempo: 0.8 } })).fx.tempo, 0.8);
assert.deepEqual(parseSession('{}').fx, FX_DEFAULT);
console.log('fx tests ok');
