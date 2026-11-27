# symphonia-format-riff 0.5.5

源碼保留 MPL-2.0，全文見同資料夾的 `LICENSE`。Registry 封包 SHA-256 為 `c2d7c3df0e7d94efb68401d81906eae73c02b40d5ec1a141962c592d0f11a96f`。所有來源由上游 rev `6d533f26150953a882a6a111ebd13f0abf7129d5` 發行。[上游原始碼](https://github.com/pdeljanov/Symphonia/tree/6d533f26150953a882a6a111ebd13f0abf7129d5/symphonia-format-riff)

`src/common.rs` 在配置 WAV 及 AIFF 封包前檢查計算長度、取樣率及聲道。`src/wave/mod.rs` 直接跳過 INFO 文字區塊。 音訊封包上限為 256 KiB。替換新版時重新比對配置、型別化拒絕及標籤路徑，再更新 Cargo path override 與鎖定資料。
