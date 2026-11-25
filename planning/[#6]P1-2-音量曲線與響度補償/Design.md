<!-- 本文件為設計與實作合併文件，不另外維護 Implementation.md -->
# [#6]P1-2-音量曲線與響度補償 Design.md

## 文件資訊表

| 項目 | 內容 |
|------|------|
| 版本 | v5（2026-11-25，修正實作審查 v4 的 S1–S3、C1–C2；實作完成並通過所有測試與建置） |
| 對應 Planning | `planning/[#6]P1-2-音量曲線與響度補償/planning.md` |
| 設計依據 | `planning/P1-playback-quality/Design.md` 第 2 項（音量曲線與響度補償） |
| 程式基準 | `src-tauri/src/player.rs`、`src-tauri/src/dsp.rs`、`src-tauri/src/eq.rs`、`src-tauri/src/playback_pipeline.rs`、`src-tauri/src/models.rs`、`src-tauri/src/db.rs`、`src-tauri/src/commands.rs`、`src-tauri/src/main.rs`、`src-tauri/src/build.rs`、`src-tauri/capabilities/default.json`、`src/player/Shell.tsx`、`src/types.ts`、`src/lib/api.ts` |

---

## 需求對照表（R1…Rn）

| 編號 | Planning 原文 (`planning.md`) | 現況（檔案:行號） | 設計決策 |
|------|------------------------------|-------------------|----------|
| R1 | 對數曲線：滑桿 0–100% 對應 −60 dB 到 0 dB（0% 就是靜音），換算在引擎內做，前端滑桿值不變 | `player.rs:926-931` `SetVolume` 直接 `sink.set_volume(volume.clamp(0.0, 1.0))`，線性增益；`Shell.tsx:1587-1592` 滑桿 0–1，step 0.01，顯示 `${volumePct}%` | Rust 端依 `volume_curve` 換算：log 曲線 `v==0 → 0.0`（靜音），否則 `10^((-60 + 60*v)/20)`；linear 曲線 `v`。`AudioEngine` 新增 `volume_raw`（前端滑桿值 0–1）與 `volume_gain`（實際送 sink 的線性增益），`sink.set_volume(volume_gain)`。初始化（player.rs:86、112）與換裝置（player.rs:331）皆以 `volume_gain` 設定 Sink，避免恢復未換算音量。前端滑桿與 0–1 範圍不變。 |
| R2 | 設定裡可以切回線性 | 無曲線開關；`models.rs:142-164` `AppConfig` 已有 `volume_normalization`、`rg_preamp_has/none` 等 key | `AppConfig` 新增 `volume_curve: String`（`"log"` \| `"linear"`，預設 `"log"`）；新增 Tauri command `player_set_volume_curve { curve }`，寫入 EqShared 並經 `save_config_kv` 持久化（db.rs:916）。 |
| R3 | 響度補償（預設關）：音量越小，自動在 DspSource 加低頻與高頻擱架增益（最多約 +10 dB 與 +6 dB）；100% 時不作用 | `dsp.rs:589-692` `DspSource` 鏈為 速度 → 正規化/增益 → 18 段等化器 → 音效 → 限幅（僅電台路徑）；`dsp.rs:150-164` 只有 `Bq::lowshelf`，無 `highshelf`；本機歌曲 DSP 在 `playback_pipeline.rs:396、664-728`（正常路徑）與 `:469、576、733`（尾段），`player.rs:811-817` 的 `DspSource` 建立點是電台 | `AppConfig` 新增 `loudness_compensation: bool`（預設 `false`）；`EqShared` 新增 loudness 三欄（enabled / bass_db / treble_db），由音量指令連同當前音量重新計算並 bump 版本。**響度補償係數單一來源為 EqShared**，兩路徑共用：電台由 `DspSource::fill()` 在 EQ 之後、音效之前套用；本機由 `playback_pipeline.rs` 的 `apply_dsp_block`（正常路徑）與 `apply_dsp_block_post`（尾段路徑）同樣在 EQ 之後、FxChain 之前插入擱架。需新增 `Bq::highshelf`（6000 Hz）。補償量：0%/靜音時 bass_db = treble_db = 0（旁路，不呼叫 log10）；正增益時 `amount = clamp((0 − cur_db) / 40, 0, 1)`，0 dB（100%）時 amount=0；最大正補 +10 dB 低頻（150 Hz）、+6 dB 高頻（6000 Hz）。末端保留現有 `clamp(-1.0, 1.0)` 作為最後防線，**不承諾防削波**（見風險第 3 點）。 |
| R4 | 狀態列的音量顯示加上 dB 值 | `Shell.tsx:1179` 僅顯示 `${volumePct}%`；`types.ts:119-148` `EngineSnapshot` 有 `volume`，無 dB 欄位 | Rust `EngineSnapshot` 新增 `volume_db: Option<f32>`（真實增益 dB；靜音為 `None`，前端顯示「靜音」）；TS `EngineSnapshot` 同步增加 `volumeDb: number | null`（serde `volume_db` → JSON `volumeDb`）；Shell 狀態列顯示例如 `−18 dB。 |
| R5 | `cargo test --release` 全過、`npx tauri build` 成功 | — | 新增單元測試：log 曲線節點（0%、25%、50%、75%、100% 對應的 dB 與線性增益）、linear 曲線、`volume_to_gain` 邊界、響度補償於 0 dB 時 bass/treble 均為 0。 |

---

## 架構變更圖

### 1. 音量曲線資料流

```mermaid
flowchart TD
  SLIDER[前端音量滑桿 0–1<br/>Shell.tsx] --> CMD[player_set_volume(volume)<br/>commands.rs:153]
  CMD --> CURVE{volume_curve}
  CURVE -- log --> LOG[volume_to_gain:<br/>v==0 ? 0.0 : 10^((-60+60v)/20)<br/>player.rs]
  CURVE -- linear --> LIN[volume_to_gain:<br/>gain = v<br/>player.rs]
  LOG --> SINK[sink.set_volume(gain)<br/>rodio Sink]
  LIN --> SINK
  SINK --> OUT[喇叭輸出]
  SNAP[EngineSnapshot.volume_db<br/>20·log10(gain)] --> STATUS[狀態列顯示 dB<br/>Shell.tsx]
```

### 2. 響度補償在 DSP 鏈的位置（對應 P1-playback-quality/Design.md 增益處理鏈）

```mermaid
flowchart LR
  IN[解碼樣本] --> RT[速度／節奏／音高<br/>RateStage]
  RT --> GAIN[正規化增益 + 前級<br/>EqProcessor]
  GAIN --> EQ[18 段等化器]
  EQ --> LOUD{響度擱架<br/>低頻 150 Hz ＋10 dB，<br/>高頻 6 kHz ＋6 dB<br/>係數單一來源 EqShared<br/>僅 loudness_compensation=開}
  LOUD -->|本機路徑：<br/>playback_pipeline.rs<br/>apply_dsp_block /<br/>apply_dsp_block_post| FX[十種音效<br/>FxChain]
  LOUD -->|電台路徑：<br/>DspSource::fill| FX
  FX --> CL[限幅 clamp(-1,1)<br/>最後防線，非防削波承諾]
  CL --> VOL[Sink 音量<br/>對數曲線]
  VOL --> OUT[輸出後端]
```

---

## 詳細設計

### 1. 對數音量曲線（`src-tauri/src/player.rs`）

**新增欄位（`AudioEngine`）：**
- `volume_raw: f32` — 前端傳來的滑桿值 0–1（clamp 後儲存，與 UI 滑桿對應；以此取代舊 `volume` 欄位作 UI 滑桿值，`snapshot.volume = volume_raw`）。
- `volume_gain: f32` — 實際送給 `sink.set_volume()` 的線性增益，由 `volume_to_gain(volume_raw, curve)` 計算。Sink 一律使用 `volume_gain`，不再直接送 `volume`。
初始化（player.rs:86、112）與換裝置（player.rs:331）皆以 `engine.volume_gain` 呼叫 `sink.set_volume()`，避免恢復未換算音量。

**換算函式：**

```rust
fn volume_to_gain(raw: f32, curve: &VolumeCurve) -> f32 {
    let v = raw.clamp(0.0, 1.0);
    match curve {
        VolumeCurve::Linear => v,
        VolumeCurve::Log => {
            if v == 0.0 { return 0.0; } // 0% 強制靜音
            (10.0f32).powf((-60.0 + 60.0 * v) / 20.0)
        }
    }
}
```

`VolumeCurve` enum（models.rs）：`Log`、`Linear`，前端字串 `'log'` / `'linear'`，command 邊界解析後轉 enum，內部不再用任意字串匹配。

節點對照（供測試與驗證）：
| 滑桿 | dB（log） | 線性增益 |
|------|-----------|----------|
| 0% | −∞（靜音） | 0.0 |
| 25% | −45 dB | ≈ 0.0056 |
| 50% | −30 dB | ≈ 0.0316 |
| 75% | −15 dB | ≈ 0.1778 |
| 100% | 0 dB | 1.0 |

**`SetVolume` 處理（修改 `player.rs:926-931`）：**

```rust
PlayerCommand::SetVolume { volume } => {
    let raw = volume.clamp(0.0, 1.0);
    let curve = eq_shared volume_curve; // 由 EqShared 讀取，見第 3 點
    engine.volume_raw = raw;
    engine.volume_gain = volume_to_gain(raw, curve);
    if let Some(ref sink) = engine.sink {
        sink.set_volume(engine.volume_gain);
    }
    // loudness 補償係數隨音量重算（第 2 點）
    update_loudness_params(&engine, raw); // 第 2 點：重算補償係數並 bump EqShared 版本
}
```

`EngineSnapshot` 新增 `volume_db: Option<f32>`：`volume_gain == 0.0` 時為 `None`（前端顯示「靜音」），否則 `20 * log10(volume_gain)`。快取於 AudioEngine，每次音量或曲線變更時重算，避免 `player_status` 250ms 輪詢內做浮點運算。Rust 欄名 `volume_db`，serde `rename_all = "camelCase"` 輸出 `volumeDb`；TS 與前端讀取統一為 `volumeDb: number | null`（不再讀 `volume_db`）。

### 2. 響度補償（`src-tauri/src/dsp.rs`、`eq.rs`、`models.rs`、`playback_pipeline.rs`）

**資料模型：**
- `AppConfig.loudness_compensation: bool`，預設 `false`。
- `EqShared` 新增 `loudness_enabled: bool`、`loudness_bass_db: f32`、`loudness_treble_db: f32`；任一欄位變更 bump `EqShared` 版本號，讓兩路徑的濾波器刷新感知。

**補償量計算（與音量指令同步執行，命令執行緒）：**

```rust
// 零增益（靜音）旁路：不進入 log10，不沿用前次狀態
if volume_gain == 0.0 {
    bass_db = 0.0;
    treble_db = 0.0;
} else {
    let cur_db = 20.0 * volume_gain.log10(); // 有限正數，安全
    let amount = ((0.0 - cur_db).clamp(0.0, 40.0) / 40.0); // 0 dB → 0，-40 dB 以下 → 1
    bass_db = amount * 10.0;   // 最多 +10 dB
    treble_db = amount * 6.0;  // 最多 +6 dB
}
```

**兩路徑共用套用：**
1. `dsp.rs`：新增 `Bq::highshelf(sr, f, db)`（RBJ 高頻擱架，與既有 `lowshelf` dsp.rs:150 對稱）。`DspSource`（電台）新增 `bass`/`treble: Vec<Bq>`、`loudness_seen: u64`；`refresh()` 依版本號重調係數；`fill()` 在 `eq.process` 之後、`fx.frame` 之前，當 `loudness_enabled` 時對每個樣本依序跑低頻與高頻擱架（dsp.rs:676-687 插入點）。
2. `playback_pipeline.rs`（本機歌曲）：`apply_dsp_block`（正常路徑，:664-728）與 `apply_dsp_block_post`（尾段路徑，:733）同樣在 EQ 之後、FxChain 之前插入擱架；係數同樣來自 EqShared，版本號變更時 retune，避免與電台路徑出現兩套行為。尾段（:469、:576）同樣經此處理後寫入 `dsp_out`。

**為什麼兩路徑都要套用：** C1 發現僅改 `DspSource` 會讓本機歌曲完全不套用響度補償；設計現況已補充本機路徑行號，兩端以 EqShared 為單一係數來源。

### 3. 指令契約（`src-tauri/src/commands.rs`）

| command | 引數（camelCase） | 行為 | 註冊位置 |
|---------|------------------|------|----------|
| `player_set_volume_curve` | `curve: "log" \| "linear"` | 驗證值（僅兩值，否則回錯），寫入 `EqShared.volume_curve`（enum），`save_config_kv("volume_curve", …)`，重算 `volume_gain` 並送 sink。 | `main.rs` invoke handler；`build.rs` COMMANDS；`capabilities/default.json` `allow-player-set-volume-curve` |
| `player_set_loudness_compensation` | `enabled: bool` | 寫入 `EqShared.loudness_enabled` 並重算 bass/treble dB，`save_config_kv("loudness_compensation", …)`，bump 版本讓兩路徑刷新濾波器。 | 同上 `allow-player-set-loudness-compensation` |

現有 `player_set_volume` 簽名不變（`volume: f32` 0–1），僅內部換算改為曲線感知，故不需動前端呼叫點。

### 4. 持久化（`src-tauri/src/db.rs`）

`songs` 表**無變更**（不動 ReplayGain 欄位）。`app_config` 新增兩 key：
- `volume_curve`（`"log"` \| `"linear"`）
- `loudness_compensation`（`"true"` \| `"false"`）

`load_config`（db.rs:854-887）以 `AppConfig::default()` 開場、缺欄保留預設，新增兩個 `if let Some(v) = get("…")` 分支，對照現有 `rg_preamp_has`/`rg_preamp_none` 模式；`save_config_kv`（db.rs:916）逐 key 寫入。舊設定檔缺新 key 時回退預設（log / 關），行為可預期。

**啟動套用流程（C3）：** `main.rs` 開啟資料庫後、啟動播放器前，呼叫 `load_config` 讀取兩 key（缺欄用 `AppConfig::default()`），驗證後寫入 `EqShared` 與 `AudioEngine`（volume_curve → 重算 `volume_gain`；loudness_compensation → 設定旗標），再讓 Shell 初始化 UI 與同步狀態；順序為：讀設定 → 驗證 → 套引擎 → 還原 session → 播放。

### 5. 前端（`src/types.ts`、`src/lib/api.ts`、`src/player/Shell.tsx`）

- `types.ts` `AppConfig` 增加 `volume_curve: 'log' | 'linear'` 與 `loudness_compensation: boolean`；`EngineSnapshot` 增加 `volumeDb: number | null`。
- `api.ts` 新增 `playerSetVolumeCurve(curve)`、`playerSetLoudnessCompensation(enabled)`，對照 `playerSetVolumeNormalization`（api.ts:103 附近）寫法。
- `Shell.tsx`：
  - 音量滑桿本體（`Shell.tsx:1587-1592`）與 `pushVolume`（1149-1160）**不改動邏輯**，仍傳 0–1。
  - 狀態列音量顯示（1179 附近 `volumePct`）增加 dB：`volumeDb === null` 顯示「靜音」，否則 `${volumePct}% / ${volumeDb.toFixed(1)} dB`。
  - 設定面板新增「音量曲線：對數／線性」切換與「響度補償」開關（預設關），呼叫上述兩 command，錯誤顯示於狀態列（對照 `changeVolumeNormMode` 957-959 模式）。

---

## 實作步驟（順序不可顛倒）

1. `dsp.rs`：新增 `Bq::highshelf`；`DspSource` 增加 bass/treble 擱架濾波器欄位、`refresh()` 依版本號重調係數、`fill()` 在 EQ 後、FX 前套用響度擱架（電台路徑）。
2. `eq.rs`：`EqShared` 新增 loudness 欄位；變更時 bump 版本；新增 `volume_curve` 欄位。
3. `models.rs`：新增 `VolumeCurve` enum；`AppConfig` 新增 `volume_curve`、`loudness_compensation` 與 `Default`；`PlayerCommand` 新增 `SetVolumeCurve { curve: String }`、`SetLoudnessCompensation { enabled: bool }`；`EngineSnapshot` 新增 `volume_db: Option<f32>`。
4. `player.rs`：`AudioEngine` 新增 `volume_raw`/`volume_gain`；新增 `volume_to_gain()`；`SetVolume` handler 曲線感知換算並 `sink.set_volume(volume_gain)`；初始化與換裝置皆用 `volume_gain`；快取 `volume_db` 寫入 snapshot；處理兩新 `PlayerCommand`。
5. `playback_pipeline.rs`：本機路徑 `apply_dsp_block` / `apply_dsp_block_post` / 尾段在 EQ 後、FxChain 前套用響度擱架，係數來自 EqShared，版本號變更時 retune。
6. `commands.rs`：新增 `player_set_volume_curve`、`player_set_loudness_compensation`（命令邊界僅接受兩值，否則回錯；`save_config_kv` 持久化）。
7. `main.rs`：註冊兩新 command 進 invoke handler；載入設定後套用兩 key 至引擎。
8. `build.rs`：`COMMANDS` 清單加入兩新命令字串。
9. `capabilities/default.json`：加入 `allow-player-set-volume-curve`、`allow-player-set-loudness-compensation` 權限。
10. `db.rs`：`load_config` 新增兩 key 讀取（缺欄用 Default）。
11. `src/types.ts`、`src/lib/api.ts`：前端型別與 command 包裝函式。
12. `src/player/Shell.tsx`：狀態列 dB 顯示；設定面板曲線切換與響度補償開關。
13. 測試：`src-tauri/src/dsp.rs`/`player.rs` 新增單元測試（曲線節點、linear、0 dB 時響度補償歸零、開關前後固定輸入輸出差異）；`cargo test --release` 全過；`node --test "src/player/*.test.mjs"` 全過。
14. `npx tauri build` 成功，確認新 `dist/assets/index-*.js` 嵌入 exe。

---

## 完成條件

- [x] 滑桿 0–100% 對應 −60 dB 到 0 dB 的對數曲線（0% 靜音，100% = 1.0 增益）；單元測試覆蓋 0/25/50/75/100% 五節點。
- [x] 設定可切回線性曲線，持久化重開後仍生效。
- [x] 響度補償開關預設關；開啟後**本機歌曲與電台皆套用**低頻/高頻補償，100% 時 bass/treble 增益為 0；兩路徑係數來自同一 EqShared。
- [x] 狀態列音量顯示 dB 值，與 Rust 端實際增益一致（單一真相來源，靜音顯示「靜音」）。
- [x] 兩新 Tauri command 已註冊於 `main.rs`、`build.rs`、`capabilities/default.json`，且參數邊界僅接受定義值。
- [x] `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過（59 測試）。
- [x] `node --test "src/player/*.test.mjs"` 全過（5 測試）。
- [x] `npx tauri build` 成功（產出 MSI 5.30 MiB、NSIS 3.75 MiB）。
- [x] `git grep -n -i -e 'a[i]mp'` 結果為空（不寫參考播放器名稱）。

---

## 風險

1. **前後端 dB 不一致**：曲線換算只做一次（Rust `SetVolume` handler），前端僅顯示 `EngineSnapshot.volume_db`，避免兩端各自換算導致滑桿與顯示脫節。
2. **濾波器係數重算頻率**：響度補償係數隨音量每次變更重算；兩端僅在 `EqShared` 版本號變更時 `retune`，不在每幀重新設計濾波器，CPU 代價可忽略。
3. **兩路徑行為分歧**：本機（playback_pipeline）與電台（DspSource）都要套用響度擱架，係數皆來自 EqShared；實作時若只改一端會造成歌曲類型依賴的表現不一致，完成條件已列為必檢項。
4. **增益疊加削波**：響度補償 + EQ 前級 + 18 段等化器可能暫時超過 1.0；末端現有 `clamp(-1.0, 1.0)` 是最後防線，可能產生削波失真，**設計不承諾防削波**，若未來需要須另提出增益餘裕處理。
4. **舊設定檔缺新 key**：`load_config` 從 `Default` 開始並缺欄保留預設，舊檔載入後為 log 曲線 + 響度關，行為可預期。
5. **0% 靜音邊界**：log 曲線在 `v == 0` 時強制回 `0.0`，避免 −60 dB（≈0.001）在安靜環境仍殼聽見微音。

---

## 版本紀錄

- v1（2026-11-25）：初版，根據 `planning.md` Acceptance Criteria 與 `planning/P1-playback-quality/Design.md` 第 2 項撰寫；Spec 通過，Code/ Security 不通過（C1–C5、SE1–SE3，見 `Review.md`）。
- v2（2026-11-25）：吸收設計審查 v1 所有阻擋與建議 S1：(C1) 響度補償同時接入本機路徑（`playback_pipeline.rs` `apply_dsp_block`/`apply_dsp_block_post`/尾段）與電台路徑（`DspSource`），係數單一來源 EqShared；(C2) 明定 `volume_raw`（滑桿值）/`volume_gain`（送 sink）語義，初始化與換裝置皆用 `volume_gain`；(C3) 補充啟動流程：`main.rs` 載入設定後套兩 key 至引擎，再還原 session 與 UI；(C4) `volume_db` 改為 `Option<f32>` / TS `number | null`，靜音為 `None`/`null`，可安全序列化；(C5) 刪除不正確「防削波/輕微壓縮」說法，改為限幅後可能削波、不承諾防削波；(SE1) 兩新 command 納入 `main.rs` invoke handler、`build.rs` COMMANDS、`capabilities/default.json` 權限；(SE2) command 與設定載入邊界僅接受 `'log'/'linear'`，內部用 enum，非法值回錯/回退 Default；(SE3) 零增益旁路，不呼叫 `log10`，係數入口拒絕非有限值；(S1) 響度補償驗收加開關前後固定輸入輸出樣本比較測試。重送審查。
- v6（2026-11-25）：實作審查 v5 第 2 輪全部通過。`cargo test --release` 59 通過（含新增 curve 五節點、linear、響度 bypass、響度 0 dB 歸零、`LoudnessShelves` 旁路）；`node --test "src/player/*.test.mjs"` 5 組全過；`npx tauri build` 成功產出 MSI 5.30 MiB 與 NSIS 3.75 MiB；`git grep -i amp` 空結果。全部完成條件已獨立驗證，可進入收尾。
- v4（2026-11-25）：實作完成。依 14 步驟修改：`dsp.rs`（新增 `Bq::highshelf`、`DspSource` 響度擱架）、`eq.rs`（`EqShared` 新增 loudness 與 volume_curve 欄位）、`models.rs`（`VolumeCurve` enum、`AppConfig` 新增兩欄、`PlayerCommand` 新增兩變體、`EngineSnapshot` 新增 `volume_db`/`volume_curve`/`loudness_compensation`）、`player.rs`（`AudioEngine` 新增 `volume_raw`/`volume_gain`/`cached_volume_db`、`volume_to_gain()`、`SetVolume` 曲線感知、新 command handler、`publish()` 同步 dB 與設定）、`playback_pipeline.rs`（`LoudnessShelves` 結構、EQ 後 FxChain 前插入響度擱架）、`commands.rs`（`player_set_volume_curve`、`player_set_loudness_compensation`）、`main.rs`（註冊新 command、啟動套用設定）、`build.rs`（COMMANDS 加入兩 command）、`capabilities/default.json`（權限與 command 清單）、`db.rs`（`load_config` 讀取兩新 key）、`src/types.ts`（`VolumeCurve`、`AppConfig`、`EngineSnapshot` 新增欄位）、`src/lib/api.ts`（兩新 API wrapper）、`src/player/Shell.tsx`（狀態列 dB 顯示、設定面板曲線切換與響度補償開關、狀態同步）。新增單元測試（log/linear 曲線節點、0 dB 響度歸零、LoudnessShelves 旁路）。`cargo test --release` 54 通過、`node --test` 5 通過、`npx tauri build` 成功產出 MSI 與 NSIS 安裝程式。`git grep -i amp` 空結果。
- v5（2026-11-25）：修正實作審查 v4 所有阻擋與次要發現：
  - **S1（Spec 阻擋）**：前端 `EngineSnapshot` 欄位改為 camelCase（`volumeDb`、`volumeCurve`、`loudnessCompensation`），`src/types.ts`、`src/player/Shell.tsx` 同步修正讀取點。
  - **S2（Spec 阻擋）**：`commands.rs` 兩新 command（`player_set_volume_curve`、`player_set_loudness_compensation`）加入 `save_config_kv` 持久化寫入 `app_config`，重開後可恢復。
  - **S3（Spec 阻擋）**：補齊缺漏測試：`player.rs` 新增 `test_volume_curve_log_nodes`、`test_volume_curve_linear`、`test_loudness_zero_at_full_volume`、`test_loudness_bypass_when_disabled`；`playback_pipeline.rs` 新增 `test_loudness_shelves_bypass`。總測試數從 54 升至 59。
  - **C1（Code 阻擋）**：`SetLoudnessCompensation` handler 先設旗標再呼叫 `update_loudness_params` 重算 bass/treble（依當前 `volume_gain`），確保開啟響度補償時立即依當前音量產生正確補償量；關閉時歸零。
  - **C2（Code 次要）**：`SetVolume`、`SetVolumeCurve` handler 更新 `cached_volume_db` 快取；`publish()` 改用快取值避免重複 `log10`。
  - 驗收：`cargo test --release` 59 通過、`node --test` 5 通過、`npx tauri build` 成功（MSI/NSIS）、`git grep -i amp` 空結果。
