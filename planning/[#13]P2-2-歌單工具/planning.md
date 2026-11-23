<!-- 本文件為規劃文件，不完全是實作依據。 -->

# [#13]P2-2-歌單工具

## Description
需要在右鍵選單與歌單頁籤選單加入歌單工具：移除重複、移除缺檔、鎖定歌單、整單排序、在歌單中尋找。

## User Story
作為使用者，每個動作執行完、關掉重開，結果都還在；鎖定的歌單從任何入口都改不動；智慧歌單沒有這些選項（規則決定內容）。

## To Reproduce
不適用（非 bug）。

## Expected behavior
- 移除重複：同路徑直接移除；「同曲名＋歌手＋長度相差 ±2 秒」先列出來讓使用者確認，不能自動刪；
- 移除缺檔：只從這個歌單移除 missing=1 的項目，曲庫不動；
- 鎖定歌單：playlists 加 locked。鎖定後不能加入、移除、拖曳排序、匯入，頁籤顯示鎖頭；
- 整單排序：依任一欄排序後寫回 update_playlist_order_cmd（傳 playlist_items.id）；另外有打亂、反轉；
- 在歌單中尋找：現有 Ctrl+F 篩選要能只作用於目前歌單（目前是不是已經這樣要先確認）。

## Acceptance Criteria
- [ ] 每個動作執行完、關掉重開，結果都還在；
- [ ] 鎖定的歌單從任何入口都改不動；
- [ ] 智慧歌單沒有這些選項（規則決定內容）；
- [ ] `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過；
- [ ] `npx tauri build` 成功。

## Additional context
- 相關檔案：`src/player/Shell.tsx`、`src/lib/api.ts`、`src-tauri/src/db.rs`
- 設計來源：planning/P2-library-playlists/Design.md 第 2 項
