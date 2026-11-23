<!-- 本文件為規劃文件，不完全是實作依據。 -->

# [#8]P1-4-CUE整軌

## Description
現況完全不支援 CUE。整軌 rip（一個 flac／ape 加一個 .cue）只會顯示成一首很長的歌。需要支援 CUE 解析、虛擬曲目、分軌播放。

## User Story
作為使用者，有一張整軌 flac 加上 Big5 編碼的 .cue，掃描後出現 N 首、曲名正確；依序播放沒有斷點；拖進度條與歌詞正常；加進歌單、重新掃描後，歌單內容還在。

## To Reproduce
不適用（非 bug）。

## Expected behavior
- 解析 FILE、TRACK、INDEX 01、TITLE、PERFORMER、REM GENRE/DATE；
- 編碼依序試 UTF-8（含 BOM）、Big5、GBK、Shift-JIS；
- `songs` 加 cue_path、start_ms、end_ms；一個實體檔對應多首虛擬曲目，path 用 `實體檔#曲號` 維持 UNIQUE，另存 file_path 指向實體檔；
- 被 CUE 引用的整軌本身不出現在曲庫；
- 資料夾裡有 .cue 就先解析；實體檔不見時，那幾首標成 missing=1；
- CUE 改了要能重新掃描，但不能讓曲目 id 換掉（歌單、播放次數都靠 id）；
- load_at 從 start_ms 開始，播到 end_ms 就算這首結束；同一個檔案的下一軌用無縫排入（P0-3）接上；
- 進度條與歌詞以分軌內的時間顯示。

## Acceptance Criteria
- [ ] 一張整軌 flac＋Big5 編碼的 CUE，掃描後出現 N 首、曲名正確；
- [ ] 依序播放沒有斷點；
- [ ] 拖進度條與歌詞正常；
- [ ] 加進歌單、重新掃描後，歌單內容還在；
- [ ] `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過；
- [ ] `npx tauri build` 成功。

## Additional context
- 相關檔案：`src-tauri/src/db.rs`、`src-tauri/src/scanner.rs`、`src-tauri/src/player.rs`
- 設計來源：planning/P1-playback-quality/Design.md 第 4 項
- 無縫排入依賴 P0-3；轉檔與標籤（多首合併成一檔並產生 CUE）歸 P3
