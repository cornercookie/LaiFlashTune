<!-- 本文件為規劃文件，不完全是實作依據。 -->

# [#4]P0-C7-交接測試全覆蓋

## Description
軌交接測試只要求任一樣本非零，沒有確認兩軌皆輸出、交界時間、或 DSP 狀態延續。滿 completion 通道測試送出一次後立即 Drop，未等待通道滿，也沒有第二筆阻塞發送，不能證明指定情境。

## User Story
作為開發者，需要確認軌交接時兩軌皆有非零音訊輸出、交界處能量連續、DSP 狀態（EQ、正規化等）在交接後延續，且 `set_next` 非阻塞、只保留最新請求。

## To Reproduce
不適用（非 bug）。

## Expected behavior
- 交接測試斷言改為兩軌皆需有非零能量（`&&` 而非 `||`）；
- 第二軌需輸出 ≥ 50 個樣本；
- 新增 `test_playback_core_handoff_full_completion_channel` 驗證 `set_next` 非阻塞、gen 追蹤、latest request 保留；
- `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過。

## Acceptance Criteria
- [ ] 交接測試兩軌皆非零能量斷言已加強；
- [ ] 第二軌樣本數 ≥ 50 的驗收已加入；
- [ ] `test_playback_core_handoff_full_completion_channel` 已新增並通過；
- [ ] `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過。

## Additional context
- 相關檔案：`src-tauri/src/playback_pipeline.rs`（測試區塊）
- 審查來源：Review.md C7（Code 第 7 輪，重要）；Compacted Summary 中「仍待解決」列表
- 設計參考：planning/P0-daily-blockers/design.md v29 C7 章節
