本次為唯讀設計複查，未修改檔案、未執行產品測試或建置。**C4 尚未關閉，不能進入實作。** 下列 `Design.md` 均指本任務的 [Design v2](C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/planning/%5B%236%5DP1-2-音量曲線與響度補償/Design.md)。

**Spec**

| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 |
|---|---|---|---|---|---|
| S1 | 建議 | `Design.md:184、193` | 已加入固定輸入的開關比較，但「100% 不作用」仍只明列係數歸零，未明列輸出比較的預期結果。 | `planning.md:24` 要求小音量有差別、100% 不作用；`Design.md:184` 寫「開關前後固定輸入輸出差異」。 | 明列同一音源與其他 DSP 設定下，小音量開關輸出有差異、100% 開關輸出一致，涵蓋本機與電台。 |

結論：通過

**Code**

| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 |
|---|---|---|---|---|---|
| C4 | 阻擋 | `Design.md:22、111、161、165` | **可空型別已補，但傳輸欄名仍不一致。** 後端輸出 `volumeDb`，前端卻讀 `volume_db`；讀到 `undefined` 後不會進入 `=== null` 分支，呼叫 `toFixed()` 會失敗。R4 也仍殘留 `volume_db: f32`。 | `models.rs:239` 使用 camelCase；`src/lib/api.ts:24-25、92` 直接傳回 invoke 結果，沒有轉名。[Serde 官方說明](https://serde.rs/attr-rename.html)確認此命名行為；本次以 Node 重現該顯示分支得到 `TypeError`。 | Rust 保留 `volume_db: Option<f32>`；JSON、TS 與前端讀取統一為 `volumeDb: number \| null`，同步修正 R4。 |
| C2 | 重要 | `Design.md:64-67、175` | 初始化與換裝置已補，但既有 `AudioEngine.volume` 的去留及 snapshot 對應仍未明寫。 | `player.rs:53` 保留舊欄位；`:247` 以 `self.volume` 發布 `snapshot.volume`；設計僅寫新增 raw／gain。 | 明列以 `volume_raw` 取代舊欄位，`snapshot.volume = volume_raw`；Sink 一律使用 `volume_gain`。 |

其餘 v1 修正核對：

- **C1：設計缺口已關閉。** `Design.md:134-138、176` 涵蓋本機正常與尾段；對應 `playback_pipeline.rs:396、469、576、664、733`。電台於 `player.rs:811-814` 傳入同一 `core.eq_shared`，可支援共用參數與版本更新。現行程式尚未加入響度擱架，不能稱為「已實際套用」。
- **C3：設計缺口已關閉。** `Design.md:157` 已規定讀取、驗證、套用引擎、還原 session 的順序；現行啟動點為 `main.rs:64-68`。
- **C5：已修正。** `Design.md:208` 明確承認限幅可能造成削波失真。
- **C4 的零值表示已修正：** `Design.md:111` 指定靜音為 `None/null`，不傳送負無限大；剩餘阻擋為上表的欄名契約。

結論：不通過

**Security**

| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 |
|---|---|---|---|---|---|
| — | — | 本次修正範圍 | 未發現剩餘資安設計阻擋。 | SE1–SE3 核對如下。 | 實作後核對實際授權與拒絕路徑。 |

- **SE1：設計缺口已關閉。** `Design.md:144-145、178-180` 已列三處註冊與權限。**現行** `main.rs`、`src-tauri/build.rs`、`capabilities/default.json` 尚未加入兩新命令；這是待實作事項，不是新的設計阻擋。其做法符合現有 `build.rs:47` 與 [Tauri 權限機制](https://v2.tauri.app/security/capabilities/)。
- **SE2：設計缺口已關閉。** `Design.md:84、144、157、217` 已規定 command 拒絕非法字串、載入驗證及非法值回退預設。
- **SE3：設計缺口已關閉。** `Design.md:122-131` 明列零增益將兩個補償值歸零，不進入 `log10`；`:217` 另規定係數入口拒絕非有限值。

本次未發現新的阻擋；原 C4 仍須修正後複查。以上通過僅指設計層面。

結論：通過