// 決定下一首播放什麼（純函式，可獨立以 node 測試）。
//
// 輸出的 `song` 是下一首的歌曲 id；`fromQueue` 表示是否來自佇列；
// `stopEnqueueing` 為 true 表示連續失敗上限已達，不再排入；
// `randomPreselected` 為隨機模式下的預選結果（供比對實際播放是否一致）。
//
// 判斷順序（僅處理「往後」；上一首由呼叫端另行處理）：
//  1. 佇列第一首（手動與自然播完都一樣）
//  2. 自然播完且勾「結束後不播下一首」→ null
//  3. 自然播完且順序模式 → null
//  4. 自然播完且單曲循環 → 同一首
//  5. 隨機 → 抽一首（預選結果快取，手動切歌/改模式/改佇列/改播放清單時作廢重抽）
//  6. 播放清單下一首；全部循環時回到第一首
//
// 失敗集合裡的歌會被跳過；來自佇列的失敗歌會被移出佇列；連續失敗達 20 次時停止排入。

export const PLAY_MODE = {
  Sequence: 'sequence',
  Single: 'single',
  All: 'all',
  Random: 'random',
} as const;

export type PlayMode = (typeof PLAY_MODE)[keyof typeof PLAY_MODE];

export interface DecideNextInput {
  reason: 'ended' | 'user';
  playlist: string[];       // 播放清單歌曲 id 依序
  queue: string[];          // 佇列歌曲 id（第一個為下一首）
  mode: PlayMode;
  playAfterEnd: boolean;    // true = 「結束後不播下一首」
  currentSong: string | null;
  randomPreselected: string | null;
  failed: Set<string>;      // 這階段解碼/播放失敗的歌曲 id
}

export interface DecideNextOutput {
  song: string | null;
  fromQueue: boolean;
  randomPreselected: string | null;
  stopEnqueueing: boolean;
  consecutiveFailures: number;
  removedFromQueue: string[];
}

const MAX_CONSECUTIVE_FAILURES = 20;

function isFailed(id: string | null, failed: Set<string>): boolean {
  return id != null && failed.has(id);
}

export function decideNext(input: DecideNextInput): DecideNextOutput {
  const {
    reason,
    playlist,
    queue,
    mode,
    playAfterEnd,
    currentSong,
    randomPreselected,
    failed,
  } = input;

  let nextQueue = [...queue];
  let removedFromQueue: string[] = [];
  let consecutiveFailures = 0;

  // 計算連續失敗數：從佇列開頭開始，遇到失敗歌就遞增，遇到好歌歸零（只用於判斷停止排入）
  for (const id of nextQueue) {
    if (failed.has(id)) consecutiveFailures++;
    else break;
  }

  // 1. 佇列第一首（移除佇列開頭的失敗歌）
  while (nextQueue.length > 0 && isFailed(nextQueue[0], failed)) {
    removedFromQueue.push(nextQueue.shift()!);
  }
  if (nextQueue.length > 0) {
    return {
      song: nextQueue[0],
      fromQueue: true,
      randomPreselected,
      stopEnqueueing: false,
      consecutiveFailures: 0,
      removedFromQueue,
    };
  }

  // 佇列沒有可用歌曲時，重新計算連續失敗（全部失敗）
  consecutiveFailures = nextQueue.length === 0 && queue.length > 0 ? queue.length : consecutiveFailures;

  // 2. 自然播完且「結束後不播下一首」→ null（手動下一首不受影響）
  if (reason === 'ended' && playAfterEnd) {
    return { song: null, fromQueue: false, randomPreselected: null, stopEnqueueing: false, consecutiveFailures: 0, removedFromQueue };
  }

  // 3. 自然播完且順序模式 → null
  if (reason === 'ended' && mode === PLAY_MODE.Sequence) {
    return { song: null, fromQueue: false, randomPreselected: null, stopEnqueueing: false, consecutiveFailures: 0, removedFromQueue };
  }

  // 4. 自然播完且單曲循環 → 同一首（失敗的歌會被跳過，不會自我重複）
  if (reason === 'ended' && mode === PLAY_MODE.Single && currentSong != null && !failed.has(currentSong)) {
    return { song: currentSong, fromQueue: false, randomPreselected, stopEnqueueing: false, consecutiveFailures: 0, removedFromQueue };
  }

  // 候選清單：播放清單中的可用歌曲
  const available = playlist.filter((id) => !failed.has(id));

  // 5. 隨機模式
  if (mode === PLAY_MODE.Random) {
    let pick = randomPreselected;
    // 手動切歌、改模式、改佇列、改播放清單時作廢重抽（reason === 'user' 視為手動操作）
    if (reason === 'user' || pick == null || failed.has(pick) || !available.includes(pick)) {
      pick = available.length > 0 ? available[Math.floor(Math.random() * available.length)] : null;
    }
    if (pick != null) {
      return { song: pick, fromQueue: false, randomPreselected: pick, stopEnqueueing: false, consecutiveFailures: 0, removedFromQueue };
    }
    // 隨機清單全失敗：退回第一首（若未失敗）
    const fallback = playlist[0];
    if (fallback != null && !failed.has(fallback)) {
      return { song: fallback, fromQueue: false, randomPreselected: fallback, stopEnqueueing: false, consecutiveFailures: 0, removedFromQueue };
    }
    return { song: null, fromQueue: false, randomPreselected: null, stopEnqueueing: false, consecutiveFailures: 0, removedFromQueue };
  }

  // 6. 播放清單下一首；全部循環時回到第一首（只有全部循環才繞回，順序／單曲播到頭就停）
  let searchedFailures = 0;
  if (currentSong != null && playlist.includes(currentSong)) {
    const idx = playlist.indexOf(currentSong);
    const n = playlist.length;
    // 從下一首開始，最多掃 n-1 首，避免繞回當前歌曲；
    // 非全部循環模式不得用 modulo 繞回，只搜尋當前位置之後的歌曲
    const maxStep = mode === PLAY_MODE.All ? n : n - idx;
    for (let step = 1; step < maxStep; step++) {
      const id = playlist[(idx + step) % n];
      if (failed.has(id)) {
        searchedFailures++;
        if (searchedFailures >= MAX_CONSECUTIVE_FAILURES) break;
      } else {
        return {
          song: id,
          fromQueue: false,
          randomPreselected: null,
          stopEnqueueing: false,
          consecutiveFailures: searchedFailures,
          removedFromQueue,
        };
      }
    }
    // 繞了一圈都是失敗：只有全部循環才回到第一首
    if (mode === PLAY_MODE.All && searchedFailures < MAX_CONSECUTIVE_FAILURES) {
      const first = playlist[0];
      if (first != null && !failed.has(first)) {
        return {
          song: first,
          fromQueue: false,
          randomPreselected: null,
          stopEnqueueing: false,
          consecutiveFailures: searchedFailures,
          removedFromQueue,
        };
      }
    }
    // 繞了一圈都是失敗，或非全部循環模式下沒有下一首
    consecutiveFailures = Math.max(consecutiveFailures, searchedFailures);
  } else if (playlist.length > 0) {
    const first = playlist[0];
    if (first != null && !failed.has(first)) {
      // 非全部循環且當前歌曲不在清單內，只在全部循環時回第一首，否則停
      if (mode === PLAY_MODE.All) {
        return {
          song: first,
          fromQueue: false,
          randomPreselected: null,
          stopEnqueueing: false,
          consecutiveFailures: 0,
          removedFromQueue,
        };
      }
    } else {
      searchedFailures = playlist.length;
    }
  }

  // 全部失敗：連續失敗上限
  const stopEnqueueing =
    consecutiveFailures >= MAX_CONSECUTIVE_FAILURES || searchedFailures >= MAX_CONSECUTIVE_FAILURES;
  return {
    song: null,
    fromQueue: false,
    randomPreselected: null,
    stopEnqueueing,
    consecutiveFailures: Math.max(consecutiveFailures, searchedFailures),
    removedFromQueue,
  };
}
