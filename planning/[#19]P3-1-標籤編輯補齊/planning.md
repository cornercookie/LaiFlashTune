<!-- 本文件為規劃文件，不完全是實作依據。 -->

# [#19]P3-1-標籤編輯補齊

## Description
現況單首可改曲名、歌手、專輯、曲風、年份、音軌、碟號；批次只改歌手、專輯、曲風、年份。沒有封面。需要補齊欄位、封面、標籤格式、批次工具、ReplayGain 計算、BPM 偵測、章節與 CUE 編輯。

## User Story
作為使用者，換一張封面，清單與正在播放區的封面立刻更新；用範本改 20 個檔名，歌單與播放次數仍然對得上；算出的 ReplayGain 值跟其他通用的 ReplayGain 計算工具相差 0.1 dB 以內。

## To Reproduce
不適用（非 bug）。

## Expected behavior
- 欄位：專輯歌手、作曲、註解、總音軌數、總碟數、內嵌歌詞（ID3 USLT、Vorbis LYRICS）、BPM；
- 封面：加入、取代、移除、匯出成圖檔（lofty Picture）。改完要更新 data/covers 的縮圖與 cover_path；
- 標籤格式：ID3v1、ID3v2、APE、Vorbis、MP4 用 lofty 讀寫。WMA（ASF）lofty 不支援：先做成唯讀，並寫明原因；之後再評估能不能用 ffmpeg remux（-metadata，不重新編碼）；
- 批次工具（每個都要先預覽、再執行，失敗的檔案逐一列出）：
  - 依範本改檔名或搬資料夾，例如 %albumartist%\%album%\%track% - %title%。不合法的字元要替換，檔名撞到時要列出來，不能覆蓋。改完同步更新 songs.path，歌單、播放次數、書籤都不能斷；
  - 自動編號：依目前順序填音軌號與總音軌數；
  - 從檔名填標籤，例如 %artist% - %title%；
  - 大小寫轉換、簡繁轉換（中文使用者最需要的是簡繁轉換）；
- ReplayGain 計算：EBU R128（ebur128 crate），可以算單首、整張專輯、多張專輯，寫入 P1-1 讀取的那組標籤。解碼走跟播放相同的路由（symphonia／ffmpeg）；
- BPM 偵測：onset 包絡加自相關，範圍 60–200，寫入 BPM 欄。準確度測試要附樣本與誤差；
- 章節與 CUE 編輯：長檔可以在目前播放位置加分軌點，存成 .cue（P1-4 的格式），存完重新掃描就會出現分軌。

## Acceptance Criteria
- [ ] 換一張封面，清單與正在播放區的封面立刻更新；
- [ ] 用範本改 20 個檔名，歌單與播放次數仍然對得上；
- [ ] 算出的 ReplayGain 值跟其他通用的 ReplayGain 計算工具相差 0.1 dB 以內；
- [ ] `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過；
- [ ] `npx tauri build` 成功。

## Additional context
- 相關檔案：`src-tauri/src/db.rs`、`src-tauri/src/extras.rs`、`src-tauri/src/commands.rs`、`src/player/Tools.tsx`
- 設計來源：planning/P3-tools/Design.md 第 1 項
- ReplayGain 計算結果寫入 P1-1 讀取的標籤；章節與 CUE 編輯依賴 P1-4
