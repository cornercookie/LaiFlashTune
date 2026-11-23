import assert from 'node:assert/strict';
import { EQ_BANDS, migrateBands, parsePresets, mergeSeeds, SEED_VERSION, clampDb } from './eq.ts';
import { parseSession } from './session.ts';

assert.equal(EQ_BANDS, 18);

// 舊版 10 段：端點保留，中間依對數頻率內插。
const old = [4, 3, 2, 1, 0, 0, 0, 0, 0, 6];
const m = migrateBands(old);
assert.equal(m.length, 18);
assert.equal(m[0], 4, '31Hz takes the 32Hz value');
assert.equal(m[3], 2, '125Hz unchanged');
assert.equal(m[17], 6, '16k unchanged');
assert.ok(m[2] > 2 && m[2] < 3, '87Hz lands between 64 and 125');
assert.equal(migrateBands([1, 2, 3]), null, 'unknown length is rejected');
assert.equal(clampDb(40), 15);
assert.equal(clampDb('x'), 0);

// 舊 session 只有 10 段，不能被重置成平直。
const s = parseSession(JSON.stringify({ eq: old, preamp: -14 }));
assert.equal(s.eq.length, 18);
assert.equal(s.eq[0], 4);
assert.equal(s.preamp, -14, 'preamp range is now ±15');
assert.equal(s.eqEnabled, true, 'missing flag means on');
assert.equal(parseSession(JSON.stringify({ eqEnabled: false })).eqEnabled, false);

// 舊預設（10 段）要換算保留，不可被過濾掉。
const presets = parsePresets(JSON.stringify([{ name: '我的', bands: old }, { name: '', bands: old }, { name: '壞', bands: [1] }]));
assert.deepEqual(presets.map((p) => p.name), ['我的']);
assert.equal(presets[0].bands.length, 18);

// 第一次執行：全部種子。
const first = mergeSeeds([], null);
assert.ok(first.presets.length >= 10 && first.changed);
// 從第一版升級：使用者刪掉的「人聲」不補回，同名覆寫的「重低音」不動。
const mine = { name: '重低音', bands: Array(18).fill(1) };
const up = mergeSeeds([mine], null);
assert.ok(!up.presets.some((p) => p.name === '人聲'));
assert.deepEqual(up.presets.find((p) => p.name === '重低音').bands, mine.bands);
assert.ok(up.presets.some((p) => p.name === '搖滾'));
// 已升級過就不再動。
assert.equal(mergeSeeds([mine], SEED_VERSION).changed, false);
console.log('eq tests ok');
