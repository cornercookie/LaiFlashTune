本次為唯讀設計審查。以下 `Design.md` 均指 [本任務設計 v1](<C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/planning/[#6]P1-2-音量曲線與響度補償/Design.md>)。

已執行 Node 測試：**5／5 通過**。未執行會寫入建置產物的 Cargo 測試及 Tauri 建置；目前不能確認 Rust 測試綠燈。

**Spec**

需求均有設計及完成條件對應；控制項是否確實接到現行引擎，存在下列 Code／Security 阻擋。

| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 |
|---|---|---|---|---|---|
| S1 | 建議 | `Design.md:179-182` | 響度補償僅以「可聽見」驗收，重現方式不足；係數為零也不能單獨證明實際輸出不作用。 | `planning.md:24` 要求小音量有差別、100% 不作用；`Design.md:172` 僅列曲線與零補償數值測試。 | 指定同一音源、音量及其他 DSP 設定進行開關比較；利用既定補償公式核對實際輸出，無須另訂主觀聽感門檻。 |

結論：通過

**Code**

| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 |
|---|---|---|---|---|---|
| C1 | 阻擋 | `Design.md:124-132、164` | **只修改 `DspSource`，本機歌曲不會套用響度補償。** 設計引用的幀迴圈存在，但不是目前本機播放的處理路徑。 | `src-tauri/src/player.rs:94-96` 將 `OutputProxy` 接入 Sink；`playback_pipeline.rs:396、664-728` 執行本機 DSP，尾段另走 `:469、576、733`；`player.rs:811-817` 的 `DspSource` 建立點則是電台。 | 修正現況與修改清單，將補償接入本機正常播放及尾段路徑，並保留電台處理；共用補償邏輯，避免兩套行為。 |
| C2 | 阻擋 | `Design.md:63-65、91-104、167` | 新增 `volume_raw／volume_gain` 卻未交代既有 `volume` 的遷移，漏掉初始化及換裝置的音量寫入。照列出的步驟修改，換裝置可能恢復未換算音量，與 dB 顯示不符。 | `src-tauri/src/player.rs:86、112` 初始化為 `0.7`；`:247` 發布舊 `volume`；`:331` 換裝置仍以 `self.volume` 設定 Sink。 | 明定舊欄位移除或保留語義；所有 Sink 建立與更新點統一使用換算後增益，snapshot 的滑桿值及 dB 同步更新。 |
| C3 | 阻擋 | `Design.md:149、155-158、180` | **缺少持久化設定重新套用至引擎的流程。** 增加 `load_config` 欄位不會自動讓重啟後的引擎使用儲存值。 | `src-tauri/src/main.rs:64-68` 開啟資料庫後直接啟動播放器；`player.rs:91` 建立預設 `EqShared`；`src/player/Shell.tsx:423-452` 還原 session，未讀取及套用兩個新設定。 | 指定啟動時的讀取、驗證、引擎套用及 UI 同步位置，並交代其與首次設定音量／播放的順序。 |
| C4 | 阻擋 | `Design.md:107、153、157` | **`volume_db: number` 的傳輸契約不成立。** Rust 現有序列化會輸出 `volumeDb`；靜音的負無限大也不會以 JavaScript 數字傳回。 | `src-tauri/src/models.rs:238-240` 使用 `rename_all = "camelCase"`；`src/lib/api.ts:24-25` 直接回傳 invoke 結果，沒有改名層。[serde_json 原始碼](https://docs.rs/serde_json/latest/src/serde_json/ser.rs.html#155-165) 將非有限浮點數序列化為 `null`。 | 明定一致的傳輸欄名；靜音採明確可序列化契約，例如 Rust `Option<f32>`、TS `number \| null`，前端直接依此顯示「靜音」。 |
| C5 | 重要 | `Design.md:128、194` | 將硬限幅稱為「防削波」「輕微壓縮」不正確，也沒有證據支持失真可接受。低音量的 Sink 衰減發生在限幅之後，無法還原已削平的波形。 | `src-tauri/src/dsp.rs:685` 直接截斷至 ±1；設計圖 `Design.md:52-54` 亦為限幅後才套用音量。 | 改成「限制輸出振幅，但可能產生削波失真」，刪除未經驗證的可接受結論；若要承諾防削波，須另提出增益餘裕處理。 |

結論：不通過

**Security**

依本次要求列出預先檢查結果，不代表已越過 Code 階段的阻擋。

| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 |
|---|---|---|---|---|---|
| SE1 | 阻擋 | `Design.md:134-139、162-173` | 新增兩個 command，卻漏列註冊與授權變更；「無 Tauri capability 變更」與現行專案機制不符。只新增函式與前端呼叫，控制項無法完成接線。 | `src-tauri/src/main.rs:100-127` 明列 invoke handlers；`src-tauri/build.rs:7-47` 已使用 `AppManifest::commands`；`src-tauri/capabilities/default.json:24-38` 逐項授權。[Tauri 官方說明](https://v2.tauri.app/security/capabilities/) 說明此命令權限機制。 | 將 `main.rs`、`build.rs` 及 capability 納入步驟，只新增兩個必要命令的註冊與主視窗權限。 |
| SE2 | 重要 | `Design.md:70-78、138、149` | 新 command 雖寫「驗證值」，但資料庫載入未定義非法字串處理，換算函式又將任意字串默認為 log；不能確認所有入口都只接受兩種值。 | `src-tauri/src/commands.rs:472-478` 既有通用設定入口接受任意 key/value；`db.rs:916-920` 僅做參數化寫入，沒有值域檢查；設計 `:74` 使用 wildcard。 | 在 command 與設定載入邊界明定驗證及非法值處理；內部使用 enum，移除任意字串換算分支。固定兩 key 的新命令不能取代載入驗證。 |
| SE3 | 重要 | `Design.md:115-122` | 補償計算僅註記「`volume_gain > 0` 時」，沒有定義 0% 分支；因此尚不能確認零值不進入 `log10`，或不會沿用前次補償狀態。 | `Design.md:75、95-103` 明確允許零增益並接著更新補償；`:118-121` 沒有零值分支。這是設計缺口，尚非已證實的執行期 NaN。 | 寫明零增益的有限補償值或旁路行為，正增益才計算對數；係數入口拒絕或正規化非有限值。 |

限定本次新增命令，設計使用固定兩 key；`db.rs:918-920` 的參數化 SQL 不將 key 當作檔案路徑，未見新增路徑遍歷途徑。設計亦未提出新增網路、子程序或登錄檔操作；但不能將既有 `save_config_cmd` 說成只允許兩 key。

結論：不通過