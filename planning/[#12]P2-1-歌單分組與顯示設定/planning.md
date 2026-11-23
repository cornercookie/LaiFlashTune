<!-- 本文件為規劃文件，不完全是實作依據。 -->

# [#12]P2-1-歌單分組與顯示設定

## Description
現況 TrackTable.tsx 是平面清單，固定欄位，依量到的寬度自動藏欄（日期、軌／次數、專輯）。需要支援歌單分組與顯示設定，各歌單獨立設定。

## User Story
作為使用者，1 萬首的歌單依專輯分組後捲動順暢；收合一組之後其他組位置正確；欄位設定關掉重開還在；900px 寬時不會出現水平捲軸。

## To Reproduce
不適用（非 bug）。

## Expected behavior
- 分組範本：無／專輯／專輯歌手＋專輯／資料夾／年份／曲風；
- 組標題列顯示封面縮圖、專輯、歌手、年份、首數、總長度，可以收合；
- 虛擬捲動：列分成「組標題」與「曲目」兩種，各有固定高度，@tanstack/react-virtual 用 estimateSize 依種類回傳；
- 單行／雙行：雙行模式第二行顯示歌手 · 專輯 · 格式；
- 欄位：可選欄位（曲名、歌手、專輯、專輯歌手、曲風、年份、時間、格式、位元率、取樣率、播放次數、評分、最後播放、檔名），可拖曳欄寬；
- 各歌單獨立設定：存在 app_config 的 playlist_view:<id>；沒有設定的用全域預設；
- 多選、拖曳排序、右鍵選單在分組模式下照常可用；拖曳排序只在不分組、或依專輯分組的組內可用。

## Acceptance Criteria
- [ ] 1 萬首的歌單依專輯分組後捲動順暢；
- [ ] 收合一組之後其他組位置正確；
- [ ] 欄位設定關掉重開還在；
- [ ] 900px 寬時不會出現水平捲軸；
- [ ] `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過；
- [ ] `npx tauri build` 成功。

## Additional context
- 相關檔案：`src/player/TrackTable.tsx`、`src/lib/api.ts`
- 設計來源：planning/P2-library-playlists/Design.md 第 1 項
- 第 4 項評分的「略過次數」靠 P0-3 無縫播放裡的換曲事件；第 1 項分組要先確認 TrackTable 虛擬捲動能放兩種高度的列
