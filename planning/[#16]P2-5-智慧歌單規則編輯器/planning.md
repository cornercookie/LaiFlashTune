<!-- 本文件為規劃文件，不完全是實作依據。 -->

# [#16]P2-5-智慧歌單規則編輯器

## Description
現況後端 query_smart_playlist_songs 已經支援欄位白名單與運算子，畫面只給四個固定規則。需要規則編輯畫面：多條規則、AND／OR、欄位白名單、依欄位型別給運算子、結果上限與排序、以資料夾為來源、結果套用分組。

## User Story
作為使用者，建一個「曲風包含 Rock、而且年份大於 2000、依播放次數取前 50 首」的規則，結果正確，關掉重開後重算結果一致。

## To Reproduce
不適用（非 bug）。

## Expected behavior
- 規則編輯畫面：多條規則，AND／OR，欄位只能從白名單選，運算子依欄位型別給（文字：包含／等於／不等於；數字與日期：大於／小於／介於）；
- 結果的上限筆數與排序方式（例如播放次數前 100 首）；
- 以資料夾為來源：路徑前綴規則（一個或多個資料夾）；
- 結果可以套用第 1 項的分組；
- 仍然不能寫自由 SQL，只能用白名單組合。

## Acceptance Criteria
- [ ] 建一個「曲風包含 Rock、而且年份大於 2000、依播放次數取前 50 首」的規則，結果正確；
- [ ] 關掉重開後重算結果一致；
- [ ] `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過；
- [ ] `npx tauri build` 成功。

## Additional context
- 相關檔案：`src-tauri/src/db.rs`、`src-tauri/src/commands.rs`、`src/player/Shell.tsx`
- 設計來源：planning/P2-library-playlists/Design.md 第 5 項
