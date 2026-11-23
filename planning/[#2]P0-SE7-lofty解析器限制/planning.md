<!-- 本文件為規劃文件，不完全是實作依據。 -->

# [#2]P0-SE7-lofty解析器限制

## Description
lofty 讀取標籤時沒有單欄／總標籤大小上限，且仍會將嵌入封面載入記憶體；取樣率與聲道未驗證是否在指定範圍（8000–384000 Hz、1–8 聲道）。Symphonia 單封包沒有 256 KB 配置上限，解碼錯誤計數每次 refill 歸零，不是跨封包累計。

## User Story
作為使用者，開啟異常大或惡意建構的音檔時，播放器不應因讀取過大標籤或嵌入封面而耗盡記憶體；取樣率、聲道等中繼資料應在合理範圍內，異常檔案應拒絕或回退後備路由。

## To Reproduce
不適用（非 bug）。

## Expected behavior
- lofty 讀取標籤前設定單欄 ≤ 4 KB、總標籤 ≤ 16 KB 上限，超長則拒絕該欄位；
- 不載入嵌入封面進記憶體（僅讀取標籤欄位）；
- 取樣率驗證 8000–384000 Hz、聲道 1–8，超出視為無效；
- Symphonia 單封包上限 256 KB，解碼錯誤累計超過 10 次即終止。

## Acceptance Criteria
- [ ] lofty 標籤單欄／總長度上限已實作；
- [ ] 嵌入封面不載入記憶體；
- [ ] 取樣率與聲道範圍驗證已實作；
- [ ] Symphonia 256 KB 封包上限與跨封包錯誤累計已實作；
- [ ] `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過。

## Additional context
- 相關檔案：`src-tauri/src/scanner.rs`、`src-tauri/src/audio_decode.rs`
- 審查來源：Review.md Sec#7（Security 第 3 輪，阻擋）
- 設計參考：planning/P0-daily-blockers/design.md S4 記憶體上限延伸（I1）
