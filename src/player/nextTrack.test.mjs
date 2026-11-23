import assert from 'node:assert/strict';
import { decideNext, PLAY_MODE } from './nextTrack.ts';

const failed = (ids) => new Set(ids);

const base = {
  playlist: ['A', 'B', 'C', 'D'],
  queue: [],
  mode: PLAY_MODE.Sequence,
  playAfterEnd: false,
  currentSong: 'A',
  randomPreselected: null,
  failed: new Set(),
};

function run(overrides) {
  return decideNext({ ...base, ...overrides });
}

// 1. 佇列優先
{
  const r = run({ queue: ['X', 'Y'], currentSong: 'A' });
  assert.equal(r.song, 'X', '佇列優先');
  assert.equal(r.fromQueue, true);
}

// 2. 「結束後不播下一首」：自然播完回傳 null；手動下一首仍前進
{
  const r = run({ reason: 'ended', playAfterEnd: true });
  assert.equal(r.song, null, '自然播完且不播下一首回傳 null');
  const r2 = run({ reason: 'user', playAfterEnd: true, queue: ['X'] });
  assert.equal(r2.song, 'X', '手動下一首不受「不播下一首」影響');
}

// 3. 隨機模式手動切歌後，預選作廢重抽
{
  const r1 = run({ mode: PLAY_MODE.Random, reason: 'user', randomPreselected: 'B' });
  // reason === user 會重抽；因 available 固定，結果可能為 B 或其他，這裡只檢查 randomPreselected 被重設為所挑歌
  assert.ok(r1.randomPreselected != null, '手動切歌應產生新預選');
  assert.ok(base.playlist.includes(r1.randomPreselected), '預選應在清單內');
}

// 4. 佇列 [壞檔 B, 正常 C]：B 失敗後移出佇列，下一首是 C
{
  const r = run({ queue: ['B', 'C'], failed: failed(['B']) });
  assert.equal(r.song, 'C', '佇列失敗歌移出後下一首為 C');
  assert.deepEqual(r.removedFromQueue, ['B'], 'B 從佇列移除');
}

// 5. 播放清單只有 A，佇列 [壞檔 B, 正常 C]：B 失敗移出，下一首 C，A 不受影響
{
  const r = run({ playlist: ['A'], queue: ['B', 'C'], failed: failed(['B']), currentSong: 'A' });
  assert.equal(r.song, 'C');
  assert.equal(r.fromQueue, true);
  assert.deepEqual(r.removedFromQueue, ['B']);
}

// 6. 全部循環，清單 A 加 25 首壞檔：連續失敗 20 次後停止排入
{
  const bad = Array.from({ length: 25 }, (_, i) => `bad${i}`);
  const playlist = ['A', ...bad];
  const r = run({ playlist, mode: PLAY_MODE.All, currentSong: 'A', failed: failed(bad) });
  assert.equal(r.stopEnqueueing, true, '連續 20 首失敗後停止排入');
  assert.equal(r.consecutiveFailures, 20, '連續失敗數 20（達上限即停止）');
  assert.equal(r.song, null);
}

// 7. 隨機模式，清單只有 A 與兩首壞檔，兩首都失敗：下一首是 A
{
  const r = run({
    playlist: ['A', 'bad1', 'bad2'],
    mode: PLAY_MODE.Random,
    currentSong: 'A',
    failed: failed(['bad1', 'bad2']),
    randomPreselected: null,
  });
  assert.equal(r.song, 'A', '隨機清單壞檔全失敗時退回 A');
}

// 8. 順序模式播完就停
{
  const r = run({ reason: 'ended', currentSong: 'D' });
  assert.equal(r.song, null, '順序模式播完回傳 null');
}

// 9. 單曲循環播完回同一首、手動下一首照常前進
{
  const r = run({ reason: 'ended', mode: PLAY_MODE.Single, currentSong: 'A' });
  assert.equal(r.song, 'A', '單曲循環回同一首');
  const r2 = run({ reason: 'user', mode: PLAY_MODE.Single, queue: ['Z'], currentSong: 'A' });
  assert.equal(r2.song, 'Z', '手動下一首前進');
}

// 10. 隨機模式的預選結果跟實際播一致（非手動，預選在清單且未失敗）
{
  const r = run({ mode: PLAY_MODE.Random, reason: 'ended', randomPreselected: 'C' });
  assert.equal(r.song, 'C', '隨機預選即為實際播放');
  assert.equal(r.randomPreselected, 'C');
}

// 11. 全部循環播到最後一首時回到第一首
{
  const r = run({ reason: 'ended', mode: PLAY_MODE.All, currentSong: 'D' });
  assert.equal(r.song, 'A', '全部循環最後一首回第一首');
}

// 12. 佇列改變後輸出跟著改
{
  const r1 = run({ queue: ['X'] });
  assert.equal(r1.song, 'X');
  const r2 = run({ queue: ['Y'] });
  assert.equal(r2.song, 'Y');
}

// 13. 單曲清單全部循環：回到第一首（也就是唯一的那一首，不是 null）
{
  const r = run({ reason: 'ended', mode: PLAY_MODE.All, playlist: ['A'], currentSong: 'A' });
  assert.equal(r.song, 'A', '單曲清單全部循環回到第一首');
}

// 14. 單曲循環且當前歌已在失敗集合：跳過失敗歌曲，沒有其他可播時回傳 null
{
  const r = run({
    reason: 'ended',
    mode: PLAY_MODE.Single,
    playlist: ['A'],
    currentSong: 'A',
    failed: failed(['A']),
  });
  assert.equal(r.song, null, '單曲循環跳過失敗歌曲後回傳 null');
}

// 15. 順序模式播到最後一首不繞回第一首（回傳 null）
{
  const r = run({ reason: 'ended', mode: PLAY_MODE.Sequence, playlist: ['A', 'B'], currentSong: 'B' });
  assert.equal(r.song, null, '順序模式播到尾不繞回');
}

// 16. 單曲循環播完回同一首（這是 Single 模式定義，不受 modulo 繞回影響）
{
  const r = run({ reason: 'ended', mode: PLAY_MODE.Single, playlist: ['A', 'B'], currentSong: 'B' });
  assert.equal(r.song, 'B', '單曲循環回同一首');
}

// 16b. 順序模式且當前歌不在清單內，不應回第一首
{
  const r = run({ reason: 'ended', mode: PLAY_MODE.Sequence, playlist: ['A', 'B'], currentSong: 'Z' });
  assert.equal(r.song, null, '順序模式且當前歌不在清單內不應回第一首');
}

// 17. 全部循環播到最後一首繞回第一首（保留原有行為）
{
  const r = run({ reason: 'ended', mode: PLAY_MODE.All, playlist: ['A', 'B'], currentSong: 'B' });
  assert.equal(r.song, 'A', '全部循環播到尾繞回第一首');
}

console.log('nextTrack tests ok');
