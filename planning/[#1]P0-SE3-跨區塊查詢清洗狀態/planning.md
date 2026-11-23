<!-- 本文件為規劃文件，不完全是實作依據。 -->

# [#1]P0-SE3-跨區塊查詢清洗狀態

## Description
含憑證 URL 在跨讀取區塊時，查詢參數會殘留。遮蔽 `@` 後會把 `in_url` 設為 false，導致 `https://user:pass@host/path?token=secret` 的 query 不再被清除；此外，清洗狀態不跨呼叫保存，FfmpegStream 的 stderr 各區塊獨立清洗，跨區塊憑證可能殘留。

## User Story
作為使用者，當 ffmpeg/ffprobe 等子程序 stderr 含憑證 URL 時，日誌與前端顯示不應洩漏 query 權杖，即使 URL 被切成多個讀取區塊傳入清洗函式。

## To Reproduce
不適用（非 bug）。

## Expected behavior
`sanitize_log`（或取代它的有狀態清洗器）必須：
- 移除 URL 查詢參數（`?` 之後直到空白為止）；
- 跨讀取區塊保留 `in_url` / `in_query` 狀態，避免憑證跨區塊殘留；
- 清洗完成後再截斷 ≤ 1 KB，避免截斷切掉前綴導致憑證殘留。

## Acceptance Criteria
- [ ] URL 查詢參數在單一區塊內被移除；
- [ ] URL 查詢參數在跨區塊（例如第一區塊結束於 `?token=`，第二區塊是 `secret\n`）時也被移除；
- [ ] 一般 URL 不含 query 時正常保留；
- [ ] `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過。

## Additional context
- 相關檔案：`src-tauri/src/open_with.rs`、`src-tauri/src/audio_decode.rs`（FfmpegStream stderr 讀取執行緒）
- 審查來源：Review.md Sec#3（Security 第 3 輪，重要）
- 影響範圍：stderr 清洗管線，可能涉及 `sanitize_log` 呼叫點全部重構
