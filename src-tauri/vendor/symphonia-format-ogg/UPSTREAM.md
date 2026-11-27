# symphonia-format-ogg 0.5.5

源碼保留 MPL-2.0，全文見同資料夾的 `LICENSE`。Registry 封包 SHA-256 為 `2b4955c67c1ed3aa8ae8428d04ca8397fbef6a19b2b051e73b5da8b1435639cb`。所有來源由上游 rev `6d533f26150953a882a6a111ebd13f0abf7129d5` 發行。[上游原始碼](https://github.com/pdeljanov/Symphonia/tree/6d533f26150953a882a6a111ebd13f0abf7129d5/symphonia-format-ogg)

`src/logical.rs` 在組合原生音訊封包前限制片段合計及最終配置。Vorbis、Opus 與 Ogg FLAC 的標籤或圖片在辨認封包開頭後跳過，解碼設定與音訊封包沿用原流程。 音訊封包上限為 256 KiB。替換新版時重新比對配置、型別化拒絕及標籤路徑，再更新 Cargo path override 與鎖定資料。
