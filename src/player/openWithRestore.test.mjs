import assert from 'node:assert/strict';
import { sessionRestoreActions } from './openWithRestore.ts';

// 右鍵開啟時，open() 會在工作階段恢復做完資料庫讀取之前就把 token 加一。
// 若恢復邏輯在那時才取樣，token 前後相同，會誤判成「沒有人搶播放器」，
// 接著 playerPrepare 把剛開始的歌換成上次的歌並暫停。
const claimed = sessionRestoreActions(0, 2, false);
assert.equal(claimed.prepareSavedSong, false, 'open-with already started; do not replace it with the paused session song');
assert.equal(claimed.silenceForMute, false);

const normal = sessionRestoreActions(0, 0, false);
assert.equal(normal.prepareSavedSong, true, 'plain launch still resumes the saved song paused');
assert.equal(normal.silenceForMute, false);

const mutedLaunch = sessionRestoreActions(0, 0, true);
assert.equal(mutedLaunch.silenceForMute, true, 'plain launch still honors a saved mute');

const mutedOpen = sessionRestoreActions(0, 1, true);
assert.equal(mutedOpen.prepareSavedSong, false);
assert.equal(mutedOpen.silenceForMute, false, 'open-with must not be zeroed by a saved mute');

console.log('openWithRestore: ok');
