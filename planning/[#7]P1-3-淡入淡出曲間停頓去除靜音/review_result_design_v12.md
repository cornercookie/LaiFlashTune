本輪唯讀 Spec 複查，未修改 repo、未執行測試或建置。Design 雜湊前後一致；指定 `git grep` 及未追蹤 Design 的補充搜尋均無命中。

v11 #3 已關閉：D:87、178、245 已統一為「命令執行緒收到準備結果 → 背景開流 → 交付 → 曲終安裝」。#1、#2 仍未完整關閉。

位置簡稱：**D**＝[Design.md](</C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md>)；**P**＝[playback_pipeline.rs](/C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/src-tauri/src/playback_pipeline.rs)。

| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論 |
|---|---|---|---|---|---|---|
| 1 | 阻擋 | D:179–184、243 | **串流 EOF → Memory 的共同入口仍未完整定義，實作步驟也未接通串流 gap。** | D:179 新增等待分支，但 Memory 仍指定「走既有 `prepared_to_trackchain_item → TrackChain::next()`」。同段已要求串流安裝時清除舊 TrackChain；原碼 P:1040–1073 只有存在目前軌且播放完畢才會取得協調器下一軌，空鏈直接回 `None`，不能自然完成上述交接。D:243 仍只安排 `TrackChain::next()` 設定 gap，未安排串流 EOF 的一次性啟動及 Memory 安裝、離開等待狀態的步驟。影響 R4 的曲間停頓與接續播放。 | 明定等待分支直接從協調器取得 Memory、轉換並建立／安裝 TrackChain、恢復 `Playing`；將串流 EOF 的一次性 gap 啟動與這個 Memory 分支同步寫入實作步驟，避免再依賴空鏈的既有 `next()`。 | v11 #1 部分修正，未關閉。 |
| 2 | 阻擋 | D:88–106、141、178–183 | **`install_gen` 尚不能拒收取消後晚到的開流結果，且 `StreamMeta.gen` 定義缺失。** | D:88–96 的 `StreamMeta` 沒有 `gen`，但 D:178–179 使用它。更關鍵的是：交付時直接設 `install_gen = meta.gen`，安裝時再比較兩者，未與目前有效世代核對。依此文字流程，B 開流中 → Load C／ClearNext 清空並重設 0 → B 晚到重新寫入來源與 `install_gen`，仍可通過安裝檢查。原碼 P:1891–1904 的世代檢查只保護原準備結果，不能涵蓋其後新增的背景開流。 | 補回 `StreamMeta.gen`，明定開流啟動時攜帶的世代及其權威來源；交付與安裝前均核對目前有效世代，過期結果不得回填來源或改寫 `install_gen`。成功安裝後才以該世代建立完成通知。 | v11 #2 部分修正，未關閉。 |

上述影響為文件與原始碼的靜態流程推論，未經實際播放驗證；不代表 Code／Security 階段結論。

結論：不通過