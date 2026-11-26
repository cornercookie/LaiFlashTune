本輪為唯讀 Spec 複查，未修改 repo、未執行測試或建置。Design 雜湊前後一致；指定 `git grep` 與未追蹤 Design 的補充搜尋均無命中。

v11 已補 `StreamMeta.gen`、Load／Seek／ClearNext 清除規則及恢復 `Playing` 的敘述，但 v10 #1–#3 尚未完整關閉。

位置簡稱：**D**＝[Design.md](</C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md>)；**P**＝[playback_pipeline.rs](/C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/src-tauri/src/playback_pipeline.rs)。

| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論 |
|---|---|---|---|---|---|---|
| 1 | 阻擋 | D:179–186、245 | **串流 EOF → 記憶體軌仍缺可到達的交接入口，gap 啟動也未接通。** | D:186 新增「共用同一曲終交接入口」，但仍指定由 `TrackChain::next()` 安裝記憶體軌；D:180 已清除舊 TrackChain，D:245 仍只在 TrackChain 曲終設定 gap。原碼 P:361 的 `WaitingForNext` 直接回靜音；P:1040–1073 必須有目前軌且播放完畢，才會取協調器下一軌。因此新增敘述尚未交代等待狀態如何到達安裝入口。 | 明定 `PlaybackCore::next()` 在 `WaitingForNext` 的共同交接分支：串流 EOF 啟動一次 gap、耗盡後取得 Memory、建立／安裝 TrackChain 並恢復 `Playing`；同步修正實作步驟。 | v10 #1 部分修正，未關閉。 |
| 2 | 阻擋 | D:96–106、120–123、139–141、178–185 | **安裝前世代比對依賴尚未明確建立的完成通知。** | `done_target` 是可空的完成通知；D:123 在淡段完成後設定，D:180 則在串流安裝後才設 `StreamInstalled`，但安裝前已要求 `meta.gen == done_target.1`。自然交接未必有淡出完成通知，文件未定義此時比較值的來源。D:139 又引用 `seg.gen`，但完成時 `active` 已清空。新增清除規則也未明定背景開流較晚返回時如何拒收過期結果。 | 將待安裝來源綁定明確的有效世代，於開流結果交付及安裝前核對；安裝成功後以該來源世代建立 `Some((StreamInstalled, gen))`，tick 使用通知攜帶的世代。 | v10 #2 部分修正，未關閉。 |
| 3 | 重要 | D:87、178、247、302 | **開流執行緒與時機仍有三種描述，修正摘要與正文不一致。** | D:87 仍寫「已由命令執行緒開流完成」；D:178 寫 `poll_completion` 收到結果後由背景執行緒開流；D:247 仍寫背景執行緒「於準備時完成」。D:302 宣稱已統一，但上述舊描述仍存在。 | 統一為已選定的流程：命令執行緒收到準備結果 → 背景開流 → 交付來源與中繼資料 → 曲終／gap 耗盡安裝；同步修正註解與實作步驟。 | v10 #3 未關閉。 |

上述影響為文件與原始碼的靜態流程推論，未經實際播放驗證；不代表 Code／Security 階段結論。

結論：不通過