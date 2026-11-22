export type RestoreActions = {
  prepareSavedSong: boolean;
  silenceForMute: boolean;
};

/** tokenAtStart 必須在工作階段恢復的任何 await 之前取樣。
 *  右鍵開啟會在恢復讀完資料庫之前把 token 加一；若那時才取樣，
 *  前後相同，playerPrepare 會把剛開始的歌換成上次的歌並暫停。 */
export function sessionRestoreActions(tokenAtStart: number, tokenNow: number, muted: boolean): RestoreActions {
  const claimed = tokenNow !== tokenAtStart;
  return {
    prepareSavedSong: !claimed,
    silenceForMute: muted && !claimed,
  };
}
