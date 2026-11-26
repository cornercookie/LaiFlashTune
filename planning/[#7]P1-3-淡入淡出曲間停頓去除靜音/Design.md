<!-- 本文件為設計與實作合併文件，不另外維護 Implementation.md -->
# [#7]P1-3-淡入淡出曲間停頓去除靜音 Design.md

## 文件資訊表

| 項目 | 內容 |
|------|------|
| 版本 | v25（2026-11-25，吸收設計審查 Security v5 SE1-a 衍生，FadeGate 增益僅於 `next()` 套用一次，`fill()` 不套用增益，重新送審） |
| 對應 Planning | `planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/planning.md` |
| 設計依據 | `planning/P1-playback-quality/Design.md` 第 3 項（淡入淡出、曲間停頓、去除靜音） |
| 程式基準 | `src-tauri/src/player.rs`、`src-tauri/src/dsp.rs`、`src-tauri/src/models.rs`、`src-tauri/src/eq.rs`、`src-tauri/src/playback_pipeline.rs`、`src-tauri/src/commands.rs`、`src-tauri/src/main.rs`、`src-tauri/build.rs`、`src-tauri/capabilities/default.json`、`src-tauri/src/db.rs`、`src-tauri/src/scanner.rs`、`src-tauri/src/audio_decode.rs`、`src/types.ts`、`src/lib/api.ts`、`src/player/Shell.tsx`、`src/player/nextTrack.ts` |

## 需求對照表（R1…R6）

| 編號 | Planning 原文 (`planning.md`) | 現況（檔案：行號） | 設計決策 |
|------|------------------------------|-------------------|----------|
| R1 | 開始播放、暫停、繼續播放、停止：預設 150 ms 淡入淡出，0 為關 | `player.rs:924-931` Play 直接 `sink.play()`；`:933-939` Pause 直接 `sink.pause()`；`:956-965` Stop 直接 `sink.stop()`；`dsp.rs:689-733` `fill()` 輸出直接送出，無增益包絡 | 新增 `FadeGate` 置於 `PlaybackCore`（執行中段落、`playback_pipeline.rs:83`）與 `OutputProxy`（實際輸出取樣時套用 fade gain、`playback_pipeline.rs:1558-1586`）。命令執行緒下達淡出/淡入段落（目標增益、總輸出樣本數、目標動作、世代識別）；`OutputProxy::next()` **每個實際輸出樣本**獨立線性插值計算 fade gain，段落結束後鎖定 `base_gain = to_gain`（不恢復 1.0）。完成通知只在對應淡出樣本被實際取出後才發送，避免截斷淡出尾段。暫停/停止：先啟動淡出段落，當輸出增益歸零後，由命令迴圈 tick（`rx.recv_timeout(80ms)`）再呼叫 `sink.pause()`/`sink.stop()`，**不在音訊回呼內直接操作 sink**。Play（開始/繼續）：若舊段落世代已過期即撤銷，再啟動 0 → 音量 的淡入段落。 |
| R2 | 拖進度條：預設 30 ms（只為了去掉爆音） | `player.rs:966-969` Seek 直接 `restart_from(position_ms)`，保留 `engine.is_playing`，無淡門 | Seek：淡出 `seek_ms` → `restart_from(position_ms, play=is_playing)` → 淡入 `play_ms`（僅 `is_playing` 時）。暫停中 Seek 僅執行 `restart_from(position_ms, play=false)`，不啟動播放，不啟動淡入；恢復播放時才走 `resume_ms` 淡入。連續 Seek 以世代識別 latest-wins 覆寫進行中的淡段。淡出/淡入時間分別用 `seek_ms` / `play_ms`，0 為立即。 |
| R3 | 手動切歌：預設 300 ms | `Shell.tsx:578-600` 手動切歌走 `playSong → playerLoad(path, songId, token) → playerPlay()`；`player.rs:1098` `Next|Prev` 為空操作；`commands.rs:39-50` `player_load` 僅保證 Load 入列；`commands.rs:53-67` `player_set_next` 已有 `SetNext` 指令 | 手動切歌改為：前端 `playSong` **不再發保險 `playerPlay()`**，只呼叫 `playerLoad`（讓後端決定載入時機）。`player_load` handler（`player.rs:891-913`）若當前有進行中的淡段，不立即 `engine.load()`，而是設 `PendingLoad`（path/song_id/token/rg）；淡出歸零後由命令迴圈執行 `PendingLoad`（內含 `sink.play()`）。`playerPlay` handler 若有進行中的 `SwitchLoad` 段落（fade_gen 匹配），不立即 `sink.play()`，等淡出歸零後執行 `PendingLoad`。**已暫停或無可消耗音源時**：直接完成載入（不等待淡出歸零，因為無音源輸出可截斷），於新軌輸出前設定 `play_ms` 淡入。`Next|Prev` handler 不再為空：若待播槽有下一軌，下達同樣的 `SwitchLoad` 流程（`player_set_next` 復用，搭配淡出段落）；否則回錯「無下一首」。舊 Pause/Stop/SwitchLoad 完成通知若 fade_gen 已過期即捨棄。**SetNext/ClearNext 僅撤銷下一首來源（遞增 stream_gen 並清空 pending_stream_source / pending_stream_meta，不歸零 stream_gen），不影響淡段動作世代**。 |
| R4 | 曲間停頓：0–10 秒，在無縫排入之前插入指定長度靜音；跟交叉淡化互斥 | `playback_pipeline.rs:349-357` `PlaybackCore::next()` 回傳單一交錯樣本 f32、`add_played_out_frames(1)` 每個樣本累計；`:1038-1076` `TrackChain::next()` 唯一消費交接點；`player.rs:1054-1057` `SetCrossfade` 只寫 `crossfade_ms` | 新增 `gap_ms`（0–10000 ms，預設 0）。互斥：寫入 `gap_ms > 0` 時 `crossfade_ms = 0`；寫入 `crossfade_ms > 0` 時 `gap_ms = 0`。gap **僅在固定 48 kHz 雙聲道的 `PlaybackCore::next()`（`:351`）層產生與遞減**：軌 A 結束後若 `gap_ms > 0` 且 `crossfade_ms == 0`，設 `gap_remaining = gap_ms × 48000 × 2 / 1000`（48 kHz 雙聲道交錯樣本數），`next()` 回傳 `Some(0.0)` 且**不**呼叫 `add_played_out_frames`，進度不前進；gap 耗盡後載入軌 B（記憶體軌 `take_next()`，串流軌 `pending_stream_source`），走 `play_ms` 淡入。**`OutputProxy` 取出樣本時不遞減 gap**，避免兩套契約。串流軌曲終進入 WaitingForNext 時，若 `gap_ms > 0` 且 `crossfade_ms == 0`，亦一次性設 `gap_remaining`，等待期間不得重設計數。 |
| R5 | 去除靜音：記憶體軌在解碼後找出開頭與結尾低於 −60 dBFS 的段落，播放時跳過；長檔串流只處理開頭；預設關 | `player.rs:494` `collect_pcm_limit` 全段解碼進 `TrackChainItem`，無修剪；長檔 `SymStream::open(path, position_ms)` 從頭解碼；`player.rs:273-275` `played_out_frames` 以樣本數累計，`ms = samples × 1000 / (rate × channels)` | 新增 `silence_removal` 開關（預設關）、固定閾值 −60 dBFS（振幅 0.001）。記憶體軌：**只在一處**`collect_pcm_limit()` 後以完整聲道 frame（樣本組）為單位從頭尋找首個非靜音 frame 索引 `head`，從尾尋找 `tail`，切取 `[head_frame, tail_frame]`；`TrackChainItem`（`playback_pipeline.rs:925`）新增 `leading_silence_samples: usize`、`trailing_silence_samples: usize`；`native_frame_pos` 從 **0** 起算（索引裁切後陣列）；`played_out_frames` 從 0 起算。**`TrackResampler::set_track`（`:1166-1185`）從 0 起算，不從 `leading_silence_samples` 起算**，避免原檔偏移與裁切後索引混用。原檔導靜音偏移獨立保存，進度計算在 `player.rs:275` 加上 `leading_ms = leading_silence_samples × 1000 / (sample_rate × channels)`，與 `player.rs:275` 公式一致，**不再除以 channels 兩次**。保留原始 `duration_ms` 與 Seek 座標，進度條時間仍對應原檔（例如開頭 3 秒空白被跳過，進度條從 0:03 起跳）。長檔串流：僅在 prepare 時有界掃描開頭靜音（見 §8 限制），透過 `SymStream::open(path, position_ms)` 的 `position_ms` 參數傳入 leading 偏移；mid-track Seek 不重新修剪。壓縮格式前置靜音（如 AAC priming 2112 樣本）不在此設計範圍，進度以解碼後樣本幀為準。 |
| R6 | 睡眠計時暫停時也走淡出 | `Shell.tsx:516-532` 已有倒數計時，`:524` 到時呼叫 `playerPause()`；`:1642-1648` 已有控制項 | 更正現況：睡眠計時已存在，到期即呼叫 `playerPause()`。本任務不新增指令，複用 `Pause` 指令的淡出路徑（音量 → 0 後 `sink.pause()`）。驗收將既有倒數到期 → Pause → 淡出完成納入。睡眠計時實作時不得直接呼叫 `sink.pause()` 繞過淡出。 |

## 架構變更圖

### 1. 指令與 FadeGate 狀態機（樣本-based，實際輸出時推進）

```mermaid
flowchart TD
    UI[前端設定面板<br/>Shell.tsx] --> CMD[player_set_fade_settings /<br/>player_set_gap_ms /<br/>player_set_silence_removal<br/>commands.rs]
    CMD --> SETTING[EqShared 設定<br/>fade_*_ms / gap_ms /<br/>silence_removal<br/>eq.rs：版本號 bump]
    SETTING --> CTRL[命令執行緒<br/>player.rs 命令迴圈<br/>rx.recv_timeout(80ms)]
    CTRL --> SEG[執行中淡段<br/>PlaybackCore.fade_gate:<br/>from_gain/to_gain/總樣本/<br/>已耗樣本/目標動作/世代ID<br/>playback_pipeline.rs:83]
    SEG -->|實際輸出取樣時推進| FG[FadeGate 增益計算<br/>0..=1 線性插值<br/>OutputProxy::next()<br/>playback_pipeline.rs:1558-1586]
    FG -->|段落結束鎖定 base_gain| SINK[sink.pause() / sink.stop() / sink.play()<br/>由命令迴圈 tick 執行<br/>不在音訊回呼內操作 sink]
    FG -->|淡入段落| DSP[DSP 鏈：速度→增益→EQ→<br/>響度→音效→限幅→FadeGate<br/>dsp.rs / playback_pipeline.rs]
    DSP --> OUT[rodio Sink 音量<br/>對數曲線 gain]
    OUT --> SPK[喇叭輸出]
```

### 2. 曲間停頓與去除靜音資料流

```mermaid
flowchart LR
    A[軌 A 結尾<br/>PlaybackCore::next] --> GAP{gap_ms > 0<br/>且 crossfade_ms == 0}
    GAP -- 是 | G[gap_remaining = gap_ms × 48000 × 2 / 1000<br/>next() 回傳 Some(0.0)<br/>不累計 played_out_frames<br/>進度不前進]
    GAP -- 否 | N[直接 take_next()<br/>無縫交接]
    G --> G2{gap_remaining > 0?}
    G2 -- 是 | G
    G2 -- 否 | B[軌 B 開頭<br/>play_ms 淡入]
    N --> B
    DECODE[解碼 PCM] --> TRIM{silence_removal 開}
    TRIM -- 是 | CUT[頭尾低於 −60 dBFS<br/>以完整聲道 frame 切取<br/>leading/trailing_silence_samples 記錄<br/>native_frame_pos 從 0 起算<br/>導靜音偏移獨立接入進度]
    TRIM -- 否 | PASS[原樣本]
    CUT --> RES[TrackResampler<br/>從 0 起算，不從 leading 起算]
```

## 詳細設計

### 1. FadeGate 淡入淡出狀態機（`PlaybackCore`，`src-tauri/src/playback_pipeline.rs`）

FadeGate 的包絡以**輸出樣本計數**推進，在 `OutputProxy::next()` 逐個實際輸出樣本時計算線性插值增益。段落結束後**鎖定 `base_gain = to_gain`**，不恢復全增益，直到新段落覆蓋。完成通知只在對應淡出樣本被實際取出後才發送，避免截斷淡出尾段。

型別：

```rust
pub enum FadeTarget { None, Pause, Stop, Seek, SwitchLoad, StreamInstalled }

/// 執行中的淡段，由命令執行緒建立、OutputProxy 每輸出樣本消耗
pub struct FadeSegment {
    pub gen: u64,                 // 淡段世代識別，新指令遞增，舊段落 gen 不符即捨棄
    pub target: FadeTarget,       // 段落結束後待執行的動作
    pub from_gain: f32,
    pub to_gain: f32,
    pub total_samples: u64,       // 0 表示立即生效（to_gain 直接套用）
    pub consumed_samples: u64,    // 已輸出的樣本數
}

/// 待載入資料（SwitchLoad 淡出歸零後由命令迴圈執行 Load）
pub struct PendingLoad {
    pub path: String,
    pub song_id: i64,
    pub token: u64,
    pub rg: (Option<f32>, Option<f32>, Option<f32>, Option<f32>),
}

/// 待安裝的串流來源（已由命令執行緒背景開流完成，等候曲終/gap 耗盡安裝）
pub struct StreamMeta {
    pub song_id: i64,
    pub duration_ms: i64,
    pub rate: u32,
    pub channels: u16,
    pub path: String,
    pub decoder: String,
    pub position_ms: i64,
    pub gen: u64,                 // 開流啟動時攜帶的世代，交付與安裝前均與 stream_gen 比對
}

pub struct FadeGate {
    pub active: Option<FadeSegment>, // 目前執行中的淡段
    pub base_gain: f32,              // 段落結束後鎖定的增益（預設 1.0，段落結束後設為 to_gain）
    pub fade_gen: u64,               // 下一個淡段分配的世代（與串流來源世代分開）
    pub pending_load: Option<PendingLoad>,   // SwitchLoad 淡出完成後待載入的軌
    pub pending_stream_source: Option<Box<dyn Iterator<Item = f32> + Send>>, // 已開流來源
    pub pending_stream_meta: Option<StreamMeta>, // 串流中繼資料（含 gen）
    pub stream_gen: u64,             // 目前有效串流安裝世代；開流交付前與 meta.gen 比對，安裝前再比對；不歸零，持續遞增
    pub stream_install_ready: Option<u64>,   // 由 next() 在回呼中設定，攜帶當時的 stream_gen；命令執行緒 tick 在同一同步區段核對世代、交接狀態及 gap，再取出來源並消費 ready（僅消費一次）
    pub done_target: Option<(FadeTarget, u64)>, // (動作, 完成世代)，命令迴圈處理
    pub gap_remaining: u64,          // 曲間停頓剩餘樣本數（固定 48 kHz 雙聲道交錯樣本），不累計 played_out_frames
}
```

每個實際輸出樣本增益（由 `OutputProxy::next()` 取出 `out_buffer` 樣本時呼叫，`playback_pipeline.rs:1558-1586`；**在命令執行緒持 FadeGate 鎖下於音訊回呼中執行，鎖操作極短，無阻塞 I/O**）：

```rust
// 持 FadeGate 鎖（命令執行緒與回呼共用，鎖操作僅操作 u64/f32/Option，不等待 I/O、不等待 ffmpeg 程序或程序回收）
if let Some(ref mut seg) = active {
    seg.consumed_samples += 1;          // 回呼更新：本樣本已輸出
    let t = if seg.total_samples == 0 { 1.0 }
            else { clamp(seg.consumed_samples as f64 / seg.total_samples as f64, 0.0, 1.0) };
    let gain = seg.from_gain + (seg.to_gain - seg.from_gain) * t;
    if t >= 1.0 {
        // 段落結束：鎖定 base_gain = to_gain，設 done_target，等候命令執行緒執行動作
        base_gain = seg.to_gain;
        active = None;
        done_target = Some((seg.target, seg.gen));
    }
    Some(gain)
} else {
    Some(base_gain) // 無淡段時保持 base_gain（預設 1.0，段落結束後為 to_gain）
}
```

回呼每個樣本持鎖更新 `consumed_samples`、`base_gain`、`active`、`done_target`；命令執行緒持鎖寫入新段落、世代、ready 標記。兩條路徑均不在鎖內進行阻塞操作。

### 1.5 跨執行緒同步契約（FadeGate）

FadeGate 的執行中段落（`active`、`base_gain`、`consumed_samples`、`done_target`）由命令執行緒寫入，**由目前作用中的輸出路徑每個樣本持鎖讀取並更新可變狀態**：本機路徑 `OutputProxy`（音訊回呼執行緒）與電台路徑 `DspSource`（電台填充執行緒）二者擇一啟用，兩條路徑共用同一 `Arc<Mutex<FadeGate>>`，由 `fade_gen` 世代保證只有一條路徑在實際推進同一個淡段的進度。電台由 `next()`（實際逐樣本取樣出口）持鎖讀取 `core.fade_gate` 並更新 `consumed_samples`、`base_gain`、`active`、`done_target`，再乘於輸出樣本；不在 `fill()` 中預先完成通知，僅在實際逐樣本取樣出口推進。`Arc` 本身不提供內部資料同步，必須遵守以下契約：

- **內部可變性**：`FadeGate` 包裝於 `Arc<Mutex<FadeGate>>`（或 `Arc<RwLock<FadeGate>>`，讀取頻率遠高於寫入時）。每個輸出樣本持鎖讀取並更新 `consumed_samples`、`base_gain`、`active`、`done_target`；命令執行緒持鎖寫入新段落、世代、ready 標記、done_target。兩條路徑均不在鎖內進行阻塞操作，**不在鎖內呼叫 ffmpeg 取消或程序回收**。
- **段落與世代一致快照**：`active`（`FadeSegment.gen`）與 `stream_install_ready`（攜帶的 `stream_gen`）必須與 `stream_gen` 在同一同步區段讀取與寫入，避免讀取到新的增益段落卻對應舊的來源世代。
- **完成通知單次發布**：段落結束後由目前作用中的輸出路徑持鎖設定 `done_target = Some((target, gen))`，命令執行緒執行對應動作後持鎖清除。舊世代的 `done_target` 若 `gen` 不符即捨棄，不重複發布，避免 Pause／Stop 或交接停住。
- **鎖順序**：命令執行緒鎖 FadeGate → 建立段落 / 設定 `stream_install_ready`；輸出路徑鎖 FadeGate → 更新消耗計數與段落狀態 → 釋鎖。兩者操作僅操作 u64/f32/Option，極短，禁止任何執行緒持 FadeGate 鎖等待 ffmpeg 程序退出或 I/O。
- **回呼遇到競爭**：若輸出路徑無法立即取得 FadeGate 鎖（命令執行緒正在更新段落），輸出路徑使用上一次讀取到的快照繼續輸出，並將該快照中的樣本數以本地計數累加（本地 `local_consumed += k`）；重新取得鎖後，若 `fade_gen` 未變，將本地計數合併至共享 `consumed_samples`（`consumed_samples = max(consumed_samples, local_base + local_consumed)`，僅合併一次）；若 `fade_gen` 已變（段落已被新指令取代），丟棄舊進度與未發布的 `done_target`，重新以新段落起算。完成通知與合併後進度一致，維持非阻塞取鎖。

命令執行緒處理 `done_target`：

- `Pause`（fade_gen 匹配）→ `sink.pause()`，`PlaybackState::Paused`
- `Stop`（fade_gen 匹配）→ `sink.stop()`，`PlaybackState::Finished`
- `Seek`（fade_gen 匹配）→ `restart_from(position_ms, play=is_playing)`；若原 `is_playing`，再啟動 `play_ms` 淡入；並清空 `pending_stream_source` / `pending_stream_meta`，遞增 `stream_gen`
- `SwitchLoad`（fade_gen 匹配）→ 從 `pending_load` 執行 `playerLoad(path, song_id, token)`（內含 `sink.play()`），然後首次輸出前啟動 `play_ms` 淡入；並清空 `pending_stream_source` / `pending_stream_meta`，遞增 `stream_gen`
- `StreamInstalled`（fade_gen 匹配）→ 命令執行緒 `sink.play()`，設 `PlaybackState::Playing`；若當前為暫停狀態則不呼叫 `sink.play()`，保留暫停，新軌輸出前再以 `play_ms` 淡入；清空 `pending_stream_source` / `pending_stream_meta`

**舊動作取消**：新指令（Play/Load/Seek/Stop/Pause）會遞增 `fade_gen` 並建立新 `FadeSegment`；舊段落若 `gen` 不符即不再執行 `done_target` 動作。**`SetNext` / `ClearNext` 僅遞增 `stream_gen` 並清空 `pending_stream_source` / `pending_stream_meta`，不歸零 `stream_gen`，不影響淡段動作世代**。不得在音訊回呼內持鎖操作 `sink`。

### 2. 各情境行為

| 情境 | 入口 | 條件 | 淡出 | 中間（命令迴圈執行） | 淡入 | 預設 ms |
|------|------|------|------|-------------------|------|---------|
| 開始播放 | `playerPlay` → `engine.play()` | 無舊軌輸出 | — | — | 0 → 音量 | `play_ms` 150 |
| 暫停 | `playerPause` → `engine.pause()` | 有音源輸出 | 音量 → 0 | `sink.pause()`，`Paused` | — | `pause_ms` 150 |
| 繼續 | `playerPlay`（Paused 狀態） | 無音源輸出 | — | — | 0 → 音量 | `resume_ms` 150 |
| 停止 | `playerStop` → `engine.stop()` | 有音源輸出 | 音量 → 0 | `sink.stop()`，`Finished` | — | `stop_ms` 150 |
| 拖進度條 | `playerSeek` → `engine.seek(position_ms)` | 有音源輸出 | 音量 → 0 (`seek_ms`) | `restart_from(position_ms, play=is_playing)` | 0 → 音量 (`play_ms`，僅 `is_playing` 時) | `seek_ms` 30 |
| 手動切歌 | `playSong` → `playerLoad`（設 `PendingLoad`） | 有舊軌輸出 | 音量 → 0 (`switch_ms`, `SwitchLoad` 段落) | `PendingLoad` 載入新軌（token 遞增，內含 `sink.play()`） | 0 → 音量 (`play_ms`) | `switch_ms` 300 |
| 手動切歌（暫停/無音源） | `playSong` → `playerLoad`（設 `PendingLoad`） | 無舊軌輸出 | — | 直接執行 `PendingLoad`（內含 `sink.play()`） | 0 → 音量 (`play_ms`) | `switch_ms` 300 |
| 睡眠計時暫停 | `Shell.tsx:524` `playerPause()` | 有音源輸出 | 音量 → 0 | `sink.pause()`，`Paused` | — | `pause_ms` |
| 自然交接（記憶體軌） | `TrackChain::next()` / `pending_stream_meta` | 軌 A 結束 + gap 耗盡 | — | `take_next()` 載入記憶體軌，或安裝 Stream 軌 | 新軌輸出前 `play_ms` 淡入 | `play_ms` 150 |

0 ms 表示不建立淡段、直接生效。

### 3. 曲間停頓（gap）與交叉淡化互斥

- `gap_ms` 範圍 0–10 000 ms，預設 0。
- 互斥：寫入 `gap_ms > 0` 時將 `crossfade_ms` 歸 0；寫入 `crossfade_ms > 0`（現有 `player_set_crossfade`，`player.rs:1054-1057`）時將 `gap_ms` 歸 0。後寫者勝，不回錯。
- 交接點：`PlaybackCore::next()`（`playback_pipeline.rs:351`），軌 A 結束後若 `gap_ms > 0` 且 `crossfade_ms == 0`，設 `gap_remaining = gap_ms × 48000 × 2 / 1000`（48 kHz 雙聲道交錯樣本數），`next()` 回傳 `Some(0.0)` 且**不**呼叫 `add_played_out_frames`，進度不前進；gap 耗盡後載入軌 B（記憶體軌 `take_next()`，串流軌 `pending_stream_source`），走 `play_ms` 淡入。串流軌曲終進入 WaitingForNext 時，若 `gap_ms > 0` 且 `crossfade_ms == 0`，亦一次性設 `gap_remaining`，等待期間不得重設計數。

### 4. 去除靜音（silence removal）

- 開關 `silence_removal`，預設關；固定閾值 −60 dBFS（振幅 0.001），不另設 UI 閾值。
- 記憶體軌（短檔）：`collect_pcm_limit()` 後**只在一處**以完整聲道 frame（樣本組）為單位從頭尋找首個非靜音 frame 索引 `head`，從尾尋找 `tail`，切取 `[head_frame, tail_frame]`；`TrackChainItem`（`playback_pipeline.rs:925`）新增 `leading_silence_samples: usize`、`trailing_silence_samples: usize`；`native_frame_pos` 從 **0** 起算（索引裁切後陣列）；`played_out_frames` 從 0 起算。**`TrackResampler::set_track`（`:1166-1185`）從 0 起算，不從 `leading_silence_samples` 起算**，避免原檔偏移與裁切後索引混用。原檔導靜音偏移獨立保存，進度計算在 `player.rs:275` 加上 `leading_ms = leading_silence_samples × 1000 / (sample_rate × channels)`，與 `player.rs:275` 公式一致，**不再除以 channels 兩次**。保留原始 `duration_ms` 與 Seek 座標，進度條時間仍對應原檔（例如開頭 3 秒空白被跳過，進度條從 0:03 起跳）。
- 長檔串流軌：僅處理開頭；prepare 時以有界掃描偵測開頭靜音（見 §8 限制），透過 `SymStream::open(path, position_ms)` 的 `position_ms` 參數傳入 leading 偏移；mid-track Seek 不重新修剪。壓縮格式前置靜音（如 AAC priming 2112 樣本）不在此設計範圍，進度以解碼後樣本幀為準。

### 5. 預載與轉換資料鏈（R4/R5 偏移傳遞）

- 直接載入：`collect_pcm_limit()` 後 `trim_silence()`，寫入 `TrackChainItem.leading_silence_samples` / `trailing_silence_samples`，`native_frame_pos` 從 0 起算。
- 預載：`playback_pipeline.rs:1817` `prepare_track` worker 解碼完成後同樣呼叫 `trim_silence()`，偏移寫入 `PreparedTrack`（`:1610-1625`）新增兩欄位；`FfmpegDeferred`（`:1638-1649`）新增 `position_ms: i64` 欄位。
- 轉換：`prepared_to_trackchain_item()`（`:1081-1100`）將 `PreparedTrack` 的兩欄位複製進 `TrackChainItem`；`TrackResampler::set_track`（`:1166`）從 0 起算（不從 `leading_silence_samples` 起算）。
- 串流交接（開流／安裝／狀態轉移）：
  - **開流時機（命令執行緒，不在音訊回呼）**：`poll_completion`（`player.rs:1105-1112`，命令執行緒）收到 `PreparedTrack::Stream` 時，由背景執行緒依串流後端分流開流：Symphonia 類型呼叫 `SymStream::open(&mut file, &path_buf, position_ms)`；FFmpeg 類型呼叫 `open_with_seek(position_ms)`（需補齊 `playback_pipeline.rs:1665-1667` 的 `FfmpegStreamHandle::open_with_seek()`，建立獨立取消控制，新來源持有該取消控制）。將所得 `stream_source: Box<dyn Iterator<Item=f32> + Send>` 與 `StreamMeta { song_id, duration_ms, rate, channels, path, decoder, position_ms, gen: stream_gen }` 寫入 `PlaybackCore.pending_stream_source` / `pending_stream_meta`，**並以當前 `stream_gen` 作為 `meta.gen`（開流時的來源請求世代，作為後續安裝的有效世代）**。若開流期間收到 SetNext/ClearNext，會遞增 `stream_gen` 並依完整來源回收契約取出完整 `pending_stream_source` / `pending_stream_meta`，晚到的開流結果因 `meta.gen != stream_gen` 而被捨棄，其取消控制移交回收執行緒負責取消與程序回收。**不呼叫 `sink.play()`，不動 TrackChain，不改變播放狀態**。核取與寫入須與取消操作（player_load/Seek/ClearNext/SetNext）序列化。
  - **安裝時機（曲終／gap 耗盡，由 `PlaybackCore::next()` 與命令執行緒 tick 執行）**：
    - `PlaybackCore::next()` 在 `WaitingForNext` 狀態下的共同交接分支：當 `gap_remaining == 0`（或無 gap 需求）：若 `pending_stream_source` 存在且 `meta.gen == stream_gen`，設 `stream_install_ready = Some(stream_gen)`，**不在 `next()` 回呼中呼叫 `set_stream_source()`**，避免在音訊回呼或核心鎖內觸發可能阻塞的 FFmpeg 回收。若 `pending_stream_source` 已空：從 NextCoordinator 取得下一 PreparedTrack；若為 `TrackData::Memory`，`prepared_to_trackchain_item` → 建立／安裝 TrackChain（清空舊、append 新軌）、重設 TrackResampler，設 `done_target = Some((StreamInstalled, fade_gen))`；若為 `TrackData::Stream`，設 `pending_stream_meta`（待背景開流），輸出 0.0 等待開流完成。若兩者皆無：回傳 `Some(0.0)` 維持 `WaitingForNext`。
    - 命令執行緒 tick 消費 `stream_install_ready`：若 `stream_install_ready == Some(stream_gen)` 且 `pending_stream_source` 存在且 `meta.gen == stream_gen`，在命令執行緒中呼叫 `core.install_stream_source()`（執行 `set_stream_source()`，**清除舊 TrackChain**、**重建 RateStage**、**依新軌 `rate/channels` 重設 TrackResampler、保留目前音效參數**，舊來源移交回收執行緒析構），更新 `seq`/`token`/`song_id`/`path`/`decoder`，寫入 `stream_song_id`/`stream_duration_ms`/`stream_seek_origin_ms = position_ms`，`reset_played_out_frames()`，設 `stream_install_ready = None`，設 `done_target = Some((StreamInstalled, fade_gen))`。若 `meta.gen != stream_gen` 或 `pending_stream_source` 已清，依完整來源回收契約取出完整來源，釋鎖後交回收執行緒，`stream_install_ready = None`。
  - **命令執行緒 tick 處理 `done_target`**：
    - `StreamInstalled`（fade_gen 匹配）→ 命令執行緒 `sink.play()`，設 `PlaybackState::Playing`；若當前為暫停狀態則不呼叫 `sink.play()`，保留暫停，新軌輸出前再以 `play_ms` 淡入；依完整來源回收契約取出完整 `pending_stream_source` / `pending_stream_meta`，釋鎖後交回收執行緒。
    - `SwitchLoad`（fade_gen 匹配）→ 從 `pending_load` 執行 `playerLoad(path, song_id, token)`（內含 `sink.play()`），然後首次輸出前啟動 `play_ms` 淡入；依完整來源回收契約取出完整 `pending_stream_source` / `pending_stream_meta`，釋鎖後交回收執行緒。
  - **失效與消費契約**：`player_load` / `Seek` / `ClearNext` / `SetNext` 指令會遞增 `stream_gen` 並清空 `pending_stream_source` / `pending_stream_meta`，**不歸零 `stream_gen`**（持續遞增的來源請求世代，與淡段 `fade_gen` 分開），舊世代資料不再被安裝。開流結果交付時消費對應協調器 slot，區分「等待來源」與「已安裝、等待通知處理」，避免重複取出。Memory 安裝不以 `fade_gen` 覆寫來源世代。

  **完整來源回收契約**：清槽（player_load/Seek/ClearNext/SetNext）、安裝前失效（`meta.gen != stream_gen`）、過期交付、換軌舊來源移除，皆在鎖內取出完整 `pending_stream_source` / `pending_stream_meta`，**釋鎖後交回收執行緒負責取消與程序回收**；不得只移交取消控制，也不得在鎖內析構來源（避免持鎖等待 ffmpeg 程序退出阻塞音訊取樣）。回收執行緒使用 `try_wait()` / `is_finished()` + deadline，逾時 `kill()` + `drop()`，不阻塞 `wait()` / `join()`。

  **開流／掃描配額契約（SE4）**：背景開流與串流頭部靜音掃描啟動前必須取得配額，過期交付、排隊回收及尚未終止的工作持續佔用配額；配額耗盡或回收佇列滿時採非阻塞處置（拒絕新開流/掃描、不阻塞音訊取樣），完成回收後才釋放配額。回收中仍占配額，清理完成前阻止新準備，與既有 `SE3` 模式一致。

  - **串流曲終**：串流軌曲終時 `next_stream()` 回傳 `None`，進入 `WaitingForNext` 輸出靜音，由下一個 `PreparedStream` 經相同開流→安裝→`sink.play()` 流程重新安裝；若下一首為記憶體軌，由 `PlaybackCore::next()` WaitingForNext 分支安裝並恢復 `Playing`，串流與記憶體軌共用同一曲終交接入口。

### 6. 兩條 DSP 路徑的淡段共享（本機 vs 電台）

- **設定**（fade 6 個 ms、gap_ms、silence_removal）單一來源為 `EqShared`（`eq.rs`），任一變更 bump 版本號。
- **執行中段落**（active_segment、base_gain、consumed_samples、done_target）由命令執行緒寫入，**由目前作用中的輸出路徑每個樣本持鎖讀取並更新可變狀態**：本機路徑 `OutputProxy` 與電台路徑 `DspSource` 二者擇一啟用，共用同一 `PlaybackCore.FadeGate`（`playback_pipeline.rs:83`），包裝於 `Arc<Mutex<FadeGate>>`，由 `fade_gen` 世代保證只有一條路徑在實際推進同一個淡段的進度。
- **本機路徑**：`OutputProxy` 持有 `core: Arc<PlaybackCore>`（`:1511`），`next()` 每個實際輸出樣本持鎖讀取 `core.fade_gate` 並更新 `consumed_samples`、`base_gain`、`active`、`done_target` 計算 fade gain。**`total_samples` 用裝置取樣率與聲道數換算**（`OutputProxy::sample_rate()` `1599-1600` 返回裝置取樣率）。
- **電台路徑**：`DspSource`（`dsp.rs:604-619`）目前只持有 `Arc<EqShared>`；新增 `Arc<Mutex<FadeGate>>` 參數，由 `DspSource::next()`（實際逐樣本取樣出口）每個樣本持鎖讀取 `core.fade_gate` 並更新 `consumed_samples`、`base_gain`、`active`、`done_target` 計算 fade gain，再乘於輸出樣本，不在 `fill()` 中預先完成通知，不維護獨立 fade 狀態。**`total_samples` 用 `DspSource.sample_rate/channels` 換算**（`dsp.rs:758-759`）。**gap 是固定 48 kHz 雙聲道**，不受各路徑取樣率影響，gap 僅在 `PlaybackCore::next()` 處理。
- `DspSource::new` 呼叫點（`player.rs` 電台建立）需傳入 `core.fade_gate` 克隆。

### 7. 指令契約（`src-tauri/src/commands.rs`）

| command | 引數（camelCase） | 行為 | 註冊 |
|---------|------------------|------|------|
| `player_set_fade_settings` | `playMs`, `pauseMs`, `resumeMs`, `stopMs`, `seekMs`, `switchMs`（i64，各 0–5000，否則回錯） | 寫入 `EqShared` 六項 fade ms、bump 版本，`save_config_kv` 持久化六個 key | `main.rs` handler、`build.rs` COMMANDS、`capabilities/default.json` `allow-player-set-fade-settings` |
| `player_set_gap_ms` | `ms`（i64，0–10000，否則回錯） | 寫 `EqShared.gap_ms`；`ms > 0` 時 `crossfade_ms = 0`；持久化 `gap_ms` | 同上 `allow-player-set-gap-ms` |
| `player_set_silence_removal` | `enabled`（bool） | 寫 `EqShared.silence_removal`、bump 版本；持久化 `silence_removal` | 同上 `allow-player-set-silence-removal` |

現有 `player_set_crossfade` 保持不變，但其 handler 新增：`ms > 0` 時 `gap_ms = 0`。

前端讀取：`EngineSnapshot` 新增 `fadePlayMs`、`fadePauseMs`、`fadeResumeMs`、`fadeStopMs`、`fadeSeekMs`、`fadeSwitchMs`、`gapMs`、`silenceRemoval`（camelCase，由 `#[serde(rename_all = "camelCase")]` 產出）。

### 8. 資料變更

- `songs` 表：無變更。
- `app_config` 新增 9 key：`fade_play_ms`、`fade_pause_ms`、`fade_resume_ms`、`fade_stop_ms`、`fade_seek_ms`、`fade_switch_ms`、`gap_ms`、`crossfade_ms`、`silence_removal`；缺欄回退預設（150/150/150/150/30/300、0、0、false）。`AppConfig`（`models.rs:154`）新增對應欄位（含 `crossfade_ms`）與 `Default`；`load_config`（`db.rs:854-855`）讀取 9 新 key，缺欄用 `AppConfig::default()`，啟動時套入 `engine.crossfade_ms` 和 `core.set_crossfade_ms`。
- `TrackChainItem`（`playback_pipeline.rs:925`）新增 `leading_silence_samples: usize`、`trailing_silence_samples: usize`。
- `PreparedTrack`（`playback_pipeline.rs:1610-1625`）新增 `leading_silence_samples: usize`、`trailing_silence_samples: usize`。
- `FfmpegDeferred`（`playback_pipeline.rs:1638-1649`）新增 `position_ms: i64` 欄位。
- `EngineSnapshot`（`models.rs:256`）新增上述 8 個 fade/gap/silence 欄位（camelCase）。
- `EqShared`（`eq.rs`）新增 8 個設定欄位，任一變更 bump 版本號，讓 DSP 兩條路徑同步刷新。
- `PlaybackCore`（`playback_pipeline.rs:83`）新增 `FadeGate` 與 `fade_gen`；新增 `pending_stream_source: Option<Box<dyn Iterator<Item=f32> + Send>>`、`pending_stream_meta: Option<StreamMeta>`、`stream_gen: u64`、`stream_song_id: Option<i64>`、`stream_duration_ms: Option<i64>`、`stream_seek_origin_ms: u64`（串流軌歌曲身分與進度來源，供 `publish()` 與 `position_ms()` 讀取，取代 TrackChain 僅讀）。
- `DspSource`（`dsp.rs:604`）新增 `Arc<Mutex<FadeGate>>` 欄位，由 `next()`（實際逐樣本取樣出口）逐樣本套用 fade gain 一次、推進進度與發布完成通知；`fill()` 不套用 fade gain，不更新淡段狀態；`DspSource::new` 增加 `fade_gate: Arc<Mutex<FadeGate>>` 參數。

### 9. 串流頭部靜音掃描（長檔，§R5 補充）

- 直接載入入口：`player.rs:457-467` `SymStream::open(path, position_ms)`。在 `SymStream::open` 之前，於背景執行緒以有界解碼偵測開頭靜音 `leading_ms`，然後 `SymStream::open(path, leading_ms)` 從靜音結束處開始播放。
- 預載入口：`playback_pipeline.rs:1817` `prepare_track` worker。同樣在 `prepare_track` 內部偵測開頭靜音，透過 `position_ms` 傳入 `SymStream::open` 並寫入 `FfmpegDeferred.position_ms`。
- 限制：最多解碼 60 秒音訊或 50 MB PCM，取先到；到限仍全靜音時視為「無靜音」（從 0 播放），不回錯。**60 秒／50 MB 僅限制已解碼量，不作為執行期限**。
- 期限與監督者（SE3 補齊）：背景開流／掃描工作綁定獨立執行期限（預設 10 秒，由監督者線程監控），期限與「無新區塊進入」兩者任一觸發即 `cancel` 並 `kill()` ffmpeg 子程序；`stdout.read()` 在阻塞時不會進入下一次取消檢查，因此讀取區塊必須由監督者線程以 `recv_timeout` 方式監控，逾時無新資料即視為卡住並終止。Symphonia 卡住時以同一監督者隔離，保留資源計數直到回收完成。60 秒音訊／50 MB PCM 門檻標為提案，不冒充現有 400 MB / 10 s 保護，不等同於執行期限。
- 取消：`cancel: Arc<AtomicBool>`，每區塊檢查；逾時 `kill()` ffmpeg 子程序；回收由既有回收執行緒負責（`SE3` 模式）。

### 10. 前端（`src/types.ts`、`src/lib/api.ts`、`src/player/Shell.tsx`）

- `types.ts`：`EngineSnapshot` 增加 8 個 fade/gap/silence 欄位。
- `api.ts`：新增 `playerSetFadeSettings(...)`、`playerSetGapMs(ms)`、`playerSetSilenceRemoval(enabled)`，對照現有 `playerSetCrossfade` 寫法。
- `Shell.tsx`：設定面板新增「淡入淡出」六項時間輸入（ms，0 為關）、「曲間停頓」滑桿 0–10 s、「去除靜音」開關；呼叫上述指令，錯誤顯示於狀態列；初始化從 `player_status` 快照讀取目前值。**`playSong` 移除保險 `playerPlay()` 呼叫**（Shell.tsx:600），改為只呼叫 `playerLoad`，由後端在淡出歸零後載入並播放。不新增任何無法驅動引擎的控制項。

### 11. 互斥持久化與啟動順序（gap / crossfade）

- 權威來源：`app_config` 的 `gap_ms` 與 `crossfade_ms`（兩者皆持久化，新增 `crossfade_ms` key）。
- 雙向清零：寫入 `gap_ms > 0` 時 `crossfade_ms = 0` 並同時持久化；寫入 `crossfade_ms > 0` 時 `gap_ms = 0` 並同時持久化。
- 啟動套用順序：`load_config` 讀取 9 個 key（含 `crossfade_ms`），若 `crossfade_ms > 0` 則 `gap_ms = 0`，反之亦然；寫入 `EqShared` 並 bump 版本，套入 `engine.crossfade_ms` 和 `core.set_crossfade_ms`，再還原 session／UI。
- `session.crossfade`（`Shell.tsx:220` 保存，`:437、447、456` 恢復）作為 UI 快照，**從權威值更新，不反向覆寫** `app_config`，避免重啟恢復舊 `crossfade` 後清除 `gap_ms`。

## 實作步驟（順序不可顛倒）

1. `models.rs`：`PlayerCommand` 新增 `SetFadeSettings`、`SetGapMs`、`SetSilenceRemoval` 三變體；`EngineSnapshot` 新增 8 欄位；`AppConfig` 新增 9 欄位（含 `crossfade_ms`）與 `Default`；新增 `FadeTarget`/`FadeSegment`/`FadeGate`/`PendingLoad`/`StreamMeta` 型別（置於 `playback_pipeline.rs` 與 `PlaybackCore` 同處）。
2. `eq.rs`：`EqShared` 新增 8 個設定欄位，變更時 bump 版本；提供存取器供 `player.rs` 與 `playback_pipeline.rs` 使用。
3. `playback_pipeline.rs`：`PlaybackCore` 持有 `FadeGate` 與 `fade_gen`，新增 `pending_stream_source: Option<Box<dyn Iterator<Item=f32> + Send>>`、`pending_stream_meta: Option<StreamMeta>`、`stream_gen: u64`、`stream_song_id: Option<i64>`、`stream_duration_ms: Option<i64>`、`stream_seek_origin_ms: u64`；`next()`（`:351`）以 `gap_remaining` 優先回傳靜音不累計進度，在 `WaitingForNext` 狀態下若 `gap_remaining == 0`：若 `pending_stream_source` 存在且 `meta.gen == stream_gen` 則設 `stream_install_ready = true`（不在此處呼叫 `set_stream_source()`，僅在命令執行緒完成安裝）；若 `pending_stream_source` 已空，直接從 `NextCoordinator.take_next()` 取得下一軌；若為 `TrackData::Memory`，`prepared_to_trackchain_item` → 重建 TrackChain（清空舊、append 新軌）、重設 TrackResampler，設 `done_target = StreamInstalled`；若為 `TrackData::Stream`，設 `pending_stream_meta`，輸出 0.0 等待開流完成；若兩者皆無回傳 `Some(0.0)` 維持 `WaitingForNext`；`OutputProxy::next()`（`:1558-1586`）每個實際輸出樣本以樣本計數推進 `FadeGate` 增益，段落結束鎖定 `base_gain`，不遞減 gap，不調用 `set_stream_source()`；`TrackChain::next()`（`:1058-1061`）在 `gap_ms > 0` 時設 `gap_remaining`（串流進入 WaitingForNext 時一次性啟動，等待期間不得重設計數）；軌 A 結束時若下一軌為 Stream 變體（`prepared_to_trackchain_item` 回傳 `None`），設 `pending_stream_meta`，不立即安裝；`TrackChainItem` 新增兩欄位；`PreparedTrack` 新增兩欄位；`prepared_to_trackchain_item` 複製兩欄位；`collect_pcm_limit` 與 `:1817` 預載 `prepare_track` 後加 `trim_silence()`；`FfmpegDeferred` 新增 `position_ms` 欄位；`TrackResampler::set_track`（`:1166`）從 0 起算。
4. `dsp.rs`：`DspSource` 新增 `Arc<Mutex<FadeGate>>` 欄位，由 `next()`（實際逐樣本取樣出口）逐樣本套用 fade gain 一次、推進進度與發布完成通知；`fill()` 不套用 fade gain，不更新淡段狀態；`DspSource::new` 增加 `fade_gate: Arc<Mutex<FadeGate>>` 參數。
5. `player.rs`：`Pause`/`Stop`/`Play`/`Seek`/`Next|Prev` handler 改為下達淡段而非直接 `sink.pause()`/`sink.stop()`；`Load` handler 若有進行中的淡段設 `PendingLoad`，不立即執行 `engine.load()`；`Play` handler 若有進行中的 `SwitchLoad` 段落不立即 `sink.play()`；暫停中 `Seek` 保留 `is_playing`，不啟動播放；電台建立處傳入 `core.fade_gate`；`position_ms` 進度計算加上 `leading_ms`；`poll_completion`（`:1105-1112`）僅把 `PreparedTrack` 放進 `NextCoordinator.slot`（Stream 之開流由背景執行緒於 `poll_completion` 收到結果後完成，結果存入 `pending_stream_source` / `pending_stream_meta`，並以當前 `stream_gen` 作為 `meta.gen`），不安裝播放、不呼叫 `sink.play()`；命令執行緒 tick 處理 `stream_install_ready` → `core.install_stream_source()`（執行 `set_stream_source()`，**清除舊 TrackChain**、**重建 RateStage**、**依新軌 `rate/channels` 重設 TrackResampler、保留目前音效參數**，舊來源移交回收執行緒），更新 `seq`/`token`/`song_id`/`path`/`decoder`，寫入 `stream_song_id`/`stream_duration_ms`/`stream_seek_origin_ms`，`reset_played_out_frames()`，設 `done_target = StreamInstalled` 供 `sink.play()`；tick 處理 `done_target = SwitchLoad` → 執行 `PendingLoad`；tick 處理 `done_target = StreamInstalled` → `sink.play()`，設 `PlaybackState::Playing`；`publish()`（`:280-291`）從 `stream_song_id`/`stream_duration_ms` 讀取串流軌歌曲身分與總長度；新增三指令 handler；`main.rs` 開啟資料庫後載入 9 個 `app_config` key 套入 `EqShared` 與 `engine.crossfade_ms`。
6. `commands.rs`：新增 `player_set_fade_settings`、`player_set_gap_ms`、`player_set_silence_removal` 三個 Tauri 指令包裝函式（參數邊界驗證）。
7. `main.rs`：註冊三新指令進 invoke handler。
8. `build.rs`：`COMMANDS` 加入三新指令字串。
9. `capabilities/default.json`：加入三個 `allow-*` 權限。
10. `db.rs`：`load_config` 讀取 9 新 key，缺欄用 `AppConfig::default()` 預設；gap/crossfade 互斥啟動套用。
11. `src/types.ts`、`src/lib/api.ts`：前端型別與 command 包裝。
12. `src/player/Shell.tsx`：設定面板六項 fade 時間、gap 滑桿、去除靜音開關；`playSong` 移除保險 `playerPlay()`；睡眠計時到期走 `playerPause()`（不繞過淡出）；狀態同步與錯誤顯示；gap/crossfade 雙向清零保存。
13. 測試：新增單元測試（FadeGate 樣本-based 增益在 0/50%/100% 樣本點、0 ms 立即生效、段落結束鎖定 base_gain 不恢復 1.0、暫停→歸零後動作、舊世代段落捨棄、gap 靜音樣本長度不累計進度、gap_ms × 48000 × 2 / 1000 長度正確、trim_silence 頭尾偵測與樣本對齊、串流頭部修剪、指令參數邊界、gap/crossfade 互斥）；`cargo test --release` 全過；`node --test "src/player/*.test.mjs"` 全過。
14. `npx tauri build` 成功，確認新 `dist/assets/index-*.js` 嵌入 exe；關閉舊 exe 再建置。

## 完成條件

- [ ] 六項淡入淡出時間可分別設定（0 為關），預設播放/暫停/繼續/停止 150 ms、進度條 30 ms、切歌 300 ms。
- [ ] 暫停與繼續無爆音；拖進度條無喀聲；手動切歌走 300 ms 淡出＋可設停頓＋淡入（接到 `playSong → playerLoad → PendingLoad → playerLoad` 實際路徑，前端不發保險 `playerPlay()`）。
- [ ] 曲間停頓 0–10 s 可設定，與交叉淡化互斥（設定一方另一方歸 0），停頓靜音不累計歌曲進度（`gap_remaining` 樣本不呼叫 `add_played_out_frames`），長度正確（`gap_ms × 48000 × 2 / 1000` 個樣本）。
- [ ] 去除靜音預設關；開啟後記憶體軌頭尾低於 −60 dBFS 被跳過（以完整聲道 frame 對齊，`ms = samples × 1000 / (sample_rate × channels)`），長檔串流僅開頭（透過 `position_ms` 傳入 `SymStream::open`）；進度條時間仍對應原檔。
- [ ] 睡眠計時暫停經由 `Pause` 指令走淡出（Shell.tsx:524 到期呼叫 `playerPause()`，不直接呼叫 `sink.pause()`）。
- [ ] 三新 Tauri command 已註冊於 `main.rs`、`build.rs`、`capabilities/default.json`，參數邊界拒絕非法值。
- [ ] `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過。
- [ ] `node --test "src/player/*.test.mjs"` 全過。
- [ ] `npx tauri build` 成功（產出 exe 與安裝程式；缺 `.ico` 不算失敗）。
- [ ] `git grep -n -i -e 'a[i]mp'` 結果為空（不寫參考播放器名稱）。

## 驗收方法（可重現）

- **包絡數值驗證（單元測試）**：建立 44.1 kHz 立體聲正弦測試軌，設定 `pause_ms = 200`，記錄輸出樣本；確認淡出期間增益線性從 1.0 降到 0.0，淡出後 `base_gain` 鎖定 0.0 直到新段落；確認暫停命令在增益歸零後才執行 `sink.pause()`。
- **爆音／喀聲聽測（實際驗收）**：素材為開頭含 3 秒空白的 MP3 檔案；操作：播放→暫停→繼續、拖進度條、手動切歌；觀察點：暫停/繼續無爆音、拖進度條無喀聲、去除靜音開啟後進度條從 0:03 起跳；記錄方式：測試人員簽章 + 錄音波形截圖附在 `Review.md`。不自行新增無依據的音質門檻。
- **曲間停頓驗收**：素材為兩首短 MP3；操作：排入兩首到佇列（非順序模式自然播完，因順序模式播完會停止），設定 `gap_ms = 2000`，播放至軌 A 結束；預期：軌 A 結束後 2 秒靜音（測量為 192,000 個輸出樣本，於 48 kHz 雙聲道下等於 2000 ms），進度條在靜音期間不前進，然後軌 B 以 150 ms 淡入播放。
- **長檔去除靜音驗收**：素材為超過 15 分鐘且開頭與尾端皆含已知長度靜音的長檔（走串流路徑）；操作：開啟去除靜音播放；預期：從開頭靜音結束處開始播放，進度條從對應原檔時間起跳，尾端靜音不被修剪。
- **睡眠計時驗收**：操作：設定睡眠計時 1 分鐘，等到期；預期：到期呼叫 `playerPause()`，音量先淡出到 0 再 `sink.pause()`，錄音顯示先漸弱後靜音，無直接切斷。

## 風險

1. **回呼執行緒不可阻塞**：淡段狀態由命令執行緒寫、`OutputProxy` 每輸出樣本讀；動作（`sink.pause()`/`stop()`/`play()`/`restart_from()`/`install_stream_source()`）延後到命令迴圈 tick（80 ms）執行，極短淡出（如 30 ms Seek）可能尚未歸零就被新指令覆寫，採世代識別 latest-wins，此為設計預期。`install_stream_source()` 不在音訊回呼內調用，舊來源移交回收執行緒，避免阻塞。
2. **兩條 DSP 路徑必須同套**：本機軌（`OutputProxy`）與電台軌（`DspSource`）都要經 `FadeGate`，否則只有長檔有淡入淡出；設定單一來源為 `EqShared`，執行中段落單一來源為 `PlaybackCore.FadeGate`。
3. **連續 Seek 指令**：拖進度條產生大量 `Seek`，每次 30 ms 淡出可能聽起來斷續；建議前端對拖曳做防抖，後端以世代識別 latest-wins 覆寫進行中的淡段。
4. **壓縮格式取樣對齊**：AAC priming 等導致解碼後樣本幀與原檔時間非嚴格 1:1；進度條以解碼後樣本幀累計（`leading_silence_samples` 起算），不採用 wall-clock，避免與原檔時間脫節。
5. **舊設定檔缺新 key**：`load_config` 從 `Default` 開場、缺欄保留預設，舊檔載入後為預設淡時間 + 停頓關 + 靜音移除關，行為可預期。
6. **gap 與 crossfade 互斥的持久化**：`session.crossfade` 與新 `gap_ms`/`crossfade_ms` 可能在重啟時互相覆蓋；session 僅從權威值更新，不反向覆寫，避免恢復舊 `crossfade` 清除 `gap_ms`。
7. **串流頭部掃描限制**：串流開頭靜音掃描需自行定義有界限制（執行緒、最大樣本數/時間、到限結果、取消與 FFmpeg 回收），不得冒充現有 400 MB / 10 s 保護；新增門檻須標為提案。
8. **前端保險 Play 移除風險**：移除 `Shell.tsx:600` `playerPlay()` 保險呼叫後，若其他代碼路徑依賴 `playerLoad` 不自動播放，可能導致載入後不發音；需確認 `playerLoad` 內部確實呼叫 `sink.play()`（`player.rs:558-562`）。

## 版本紀錄

- v1（2026-11-25）：初版，根據 `planning.md` Acceptance Criteria 與 `planning/P1-playback-quality/Design.md` 第 3 項撰寫；Design 審查 Spec 階段不通過（S1–S10，見 `review_result_design_v1.md`）。
- v2（2026-11-25）：吸收設計審查 v1 所有阻擋/重要發現；複審仍不通過（S1–S10 的部分遺留，見 `review_result_design_v2.md`）。
- v3（2026-11-25）：吸收設計審查 v2 大部分發現；複審仍不通過（#1 frame/樣本混用、#2 base_gain 鎖定矛盾、#3 Load 保險 Play 語義、#4 gap 進度標記、#6 持久化互相覆蓋、#7 串流掃描接入點錯誤、#8 驗收覆蓋不完整，見 `review_result_design_v3.md`）。
- v4（2026-11-25）：吸收設計審查 v3 全部 8 號發現；複審仍不通過（#1 frame/樣本混用、#2 完成通知時機、#3 Switch 載入契約、#4 gap 長度少算一半、#6 crossfade 啟動接線、#7 預載串流偏移、#8 驗收證據不足，見 `review_result_design_v4.md`）。
- v5（2026-11-25）：吸收設計審查 v4 全部 7 號發現；複審仍不通過（#1 起算契約不一致、#2 毫秒轉樣本數取樣率未定義、#3 暫停/無音源 Switch 路徑缺失、#4 gap 消耗位置兩套契約、#7 預載串流偏移消費端未安排、#8 驗收前提不符，見 `review_result_design_v5.md`）。
- v6（2026-11-25）：吸收設計審查 v5 全部 6 號發現；複審收到「有條件通過」，條件為補齊 #7 預載串流交接流程（見 `review_result_design_v6.md`）。
- v7（2026-11-25）：補齊設計審查 v6 #7；複審仍不通過（#7a 交接觸發時機、#7b 偏移單位不一致、#7c 安裝契約缺重取樣器與身分更新，見 `review_result_design_v7.md`）。
- v8（2026-11-25）：吸收設計審查 v7 #7a/7b/7c；複審仍不通過（#1 開流與 sink 操作在音訊回呼臨界區、#2 `song_id` 未接到快照讀取來源、#3 與 EOF 狀態機未接合，見 `review_result_design_v8.md`）。
- v9（2026-11-25）：吸收設計審查 v8 #1/2/3；複審仍不通過（#1 串流安裝後離開 WaitingForNext 狀態轉移未定義、#2 `done_target = Switch` 兩種動作衝突、#3 安裝步驟退漏重取樣器與完整播放身分更新，見 `review_result_design_v9.md`）。
- v10（2026-11-25）：吸收設計審查 v9 全部 3 個阻擋發現；複審仍不通過（#1 串流曲終→記憶體軌交接仍未閉合、#2 `StreamInstalled` 完成世代與待安裝來源未綁定、#3 開流執行緒與時機三種描述不一致，見 `review_result_design_v10.md`）。
- v11（2026-11-25）：吸收設計審查 v10 全部 3 個發現；複審仍不通過（#1 串流 EOF→記憶體軌仍缺可到達交接入口、gap 啟動未接通；#2 安裝前世代比對依賴尚未明確建立的完成通知、`done_target` 可空、自然交接未必有淡出完成通知；#3 開流執行緒與時機三種描述不一致，見 `review_result_design_v11.md`）。
- v12（2026-11-25）：吸收設計審查 v11 全部 3 個發現；複審仍不通過（#1 串流 EOF→記憶體軌仍缺可到達交接入口、gap 啟動未接通；#2 安裝前世代比對依賴尚未明確建立的完成通知、`done_target` 可空、自然交接未必有淡出完成通知；#3 開流執行緒與時機三種描述不一致，見 `review_result_design_v12.md`）。
- v13（2026-11-25）：吸收設計審查 v12 全部 2 個發現；複審仍不通過（#1 串流 EOF→記憶體軌仍缺可到達交接入口、gap 啟動未接通；#2 交付前未先核取 `meta.gen == pending_gen`，過期開流結果仍可覆蓋有效的待安裝來源，見 `review_result_design_v13.md`）。
- v14（2026-11-25）：吸收設計審查 v13 全部 3 個發現：(#1) 明列串流 EOF → 記憶體軌共同曲終交接入口：串流曲終進入 WaitingForNext，由 NextCoordinator 準備好的下一軌（Memory）在曲終/gap 耗盡時安裝並恢復 Playing，gap 啟動一次，串流與記憶體軌共用同一入口；(#2) `StreamMeta.gen` 與 `done_target.1` 比對，`player_load`/`Seek`/`ClearNext` 清除規則，恢復 `Playing`；(§5) 串流交接完整；複審仍不通過（#1 串流 EOF→記憶體軌仍缺可到達交接入口、gap 啟動未接通；#2 安裝前世代比對依賴尚未明確建立的完成通知、`done_target` 可空、自然交接未必有淡出完成通知；#3 開流執行緒與時機三種描述不一致，見 `review_result_design_v14.md`）。
- v15（2026-11-25）：吸收設計審查 v14 全部 3 個發現：(#1) `StreamMeta` 重新加入 `gen: u64` 欄位，開流啟動時攜帶當前 `pending_gen`，安裝前與 `install_gen` 比對，過期不得安裝；`player_load`/`Seek`/`ClearNext` 遞增 `pending_gen` 並重設 `install_gen = 0`，舊世代資料失效；(§3) 統一開流時機：`poll_completion`（命令執行緒）→ 背景開流 → 結果存入 `pending_stream_source/meta` + 設 `install_gen` → 曲終/gap 耗盡由 `PlaybackCore::next()` 安裝 → `sink.play()` 由命令執行緒 tick 處理；複審仍不通過（#1 Memory 軌安裝時通知世代未建立，`install_gen` 僅由串流開流設定，Load 則清為 0，直接載入串流 A → 預載 Memory B → A 曲終 → 安裝 B 時通知帶 0 被捨棄；#2 實作步驟未同步正文，D:247 仍寫成「TrackChain 下一軌為 Memory」，見 `review_result_design_v14.md`）。
- v16（2026-11-25）：吸收設計審查 v15 全部 6 個發現：(#1) `set_stream_source()` 不在音訊回呼內調用，舊來源移交回收執行緒析構，`next()` 只標記 `stream_install_ready`，由命令執行緒 tick 處理 `StreamInstalled` → `sink.play()`；(§2) `FadeGate` 新增 `fade_gen` 與 `stream_gen` 分離，`SetNext`/`ClearNext` 僅遞增 `stream_gen` 並清空 `pending_stream_source/meta`，不影響淡段動作世代；(§3) FFmpeg 開流依後端分流，建立獨立取消控制，明確指定修改 `playback_pipeline.rs:1665-1667` 的 `FfmpegStreamHandle::open_with_seek()`；(§4) 串流安裝時重建 RateStage，沿用現有載入路徑，保留當前音效設定；(§5) 開流取得所有權時消費對應協調器 slot，區分「等待來源」與「已安裝、等待通知處理」，避免重複取出；(§6) 程式基準中的建置腳本路徑修正為 `src-tauri/build.rs`；複審仍不通過（#1 串流安裝通知形成循環依賴，可能持續停在 WaitingForNext；#2 來源世代拆分未完成，仍無一致的過期結果判定基準；#3 完整串流初始化要求未落入正文與實作步驟；#4 獨立取消控制的所有權仍未接清，見 `review_result_design_code_v3.md`）。
- v18（2026-11-25）：吸收設計審查 v17 全部 4 個發現：(#1) 命令迴圈 tick 直接消費 `stream_install_ready`、核對 `meta.gen == stream_gen`、執行 `install_stream_source()` 並完成狀態轉移，移除「先收到安裝完成通知才能安裝」的循環依賴；(§2) 統一來源請求世代：開流交付時設 `stream_gen`，安裝前與 `meta.gen` 比對，安裝後不歸零；`SetNext`/`ClearNext` 遞增 `stream_gen` 並清空 `pending_stream_source/meta`，僅撤銷下一首來源，不影響淡段動作世代；(§3) `install_stream_source()` 說明與實作步驟補充重建 RateStage、依新軌格式重設 TrackResampler、清除舊 TrackChain，保留目前音效參數；(§4) 新開流來源持有獨立取消控制；過期結果、清槽及換軌皆將完整來源移出共享鎖後交回收執行緒，負責取消與程序回收。重新送審。
- v19（2026-11-25）：吸收設計審查 v18 全部 4 個發現，將摘要修正完整落入正文：(§1) `next()` 設 `stream_install_ready`，命令執行緒 tick 直接消費 ready、核對 `meta.gen == stream_gen`、呼叫 `install_stream_source()` 完成安裝、再設 `done_target = StreamInstalled` 僅用於 `sink.play()`，移除循環依賴；(§2) 移除 `stream_gen = 0` 歸零與 `stream_gen = fade_gen` 混用，`stream_gen` 持續遞增且與 `fade_gen` 分開，Memory 安裝不覆寫來源世代；(§3) `install_stream_source()` 合約補入「清除舊 TrackChain、重建 RateStage、依新軌格式重設 TrackResampler、保留目前音效參數」；(§4) 新開流來源持有獨立取消控制，過期結果、清槽及換軌將完整來源移出共享鎖後交回收執行緒。實作步驟第 3、5 點同步調整。重新送審。
- v20（2026-11-25）：吸收設計審查 v19 全部 1 個重要發現（#4 清槽與過期來源回收契約未完整落入正文），在§5 串流交接補充「完整來源回收契約」：清槽、安裝前失效、過期交付、換軌舊來源移除皆在鎖內取出完整 `pending_stream_source` / `pending_stream_meta`，釋鎖後交回收執行緒負責取消與程序回收；不得只移交取消控制，不得在鎖內析構來源；回收執行緒使用 `try_wait()` / `is_finished()` + deadline，逾時 `kill()` + `drop()`，不阻塞 `wait()` / `join()`。§5 各處「清空」語句同改為依回收契約執行。重新送審。
- v21（2026-11-25）：吸收設計審查 Security v1 全部 4 個重要契約缺口 SE1–SE4：(§1.5) 新增「跨執行緒同步契約（FadeGate）」：FadeGate 包裝於 `Arc<Mutex<FadeGate>>`，命令執行緒寫入、回呼與電台路徑唯讀快照；段落與世代在同一同步區段讀寫；完成通知單次消費；鎖順序禁止持鎖等待 I/O／回收；回呼無法取得鎖時使用上一快照繼續輸出。 (§2) `stream_install_ready` 改為 `Option<u64>` 攜帶 `stream_gen`，tick 在同一同步區段核對世代、交接狀態及 gap，再取出來源並消費 ready（僅一次）。 (§3) §9 串流頭部靜音掃描補充獨立執行期限與監督者：預設 10 秒，期限與「無新區塊進入」任一觸發即 `cancel` + `kill()`；`stdout.read()` 阻塞由監督者以 `recv_timeout` 監控，逾時無新資料視為卡住並終止；Symphonia 卡住以同一監督者隔離並保留資源計數；60 秒／50 MB 僅限制已解碼量，不等同於執行期限。 (§4) §5 補充「開流／掃描配額契約」：啟動前取得配額，過期交付、排隊回收及未終止工作持續佔用；配額耗盡或回收佇列滿時非阻塞處置，完成回收後才釋放；回收中仍占配額，清理完成前阻止新準備。重新送審。
- v22（2026-11-25）：吸收設計審查 Security v2 SE1，統一回呼與命令執行緒的讀寫責任：(§1.1) 程式碼塊明確回呼每個樣本持鎖更新 `consumed_samples`、`base_gain`、`active`、`done_target`，命令執行緒持鎖寫入新段落、世代、ready 標記，兩條路徑均不在鎖內進行阻塞操作；(§1.5) 回呼每個樣本持鎖讀取並更新可變狀態，命令執行緒持鎖寫入，完成通知單次發布，鎖順序禁止持鎖等待 I/O／回收，回呼遇到競爭使用上一快照並由新快照的 `consumed_samples` 繼續進度；(§6) 兩條 DSP 路徑的讀寫責任同步調整。重新送審。
- v23（2026-11-25）：吸收設計審查 Security v3 SE1-a/SE1-b：(§1.5) 電台路徑 `DspSource` 亦逐樣本持鎖更新 `consumed_samples`、`base_gain`、`active`、`done_target`，不在 `fill()` 預先完成通知，僅在實際逐樣本取樣出口推進；兩條路徑共用同一 `Arc<Mutex<FadeGate>>`，由 `fade_gen` 世代保證只有一條路徑推進同一個淡段進度；鎖競爭期間以本地 `local_consumed` 累加樣本數，重新取得鎖後若 `fade_gen` 未變則合併一次至共享 `consumed_samples`，若 `fade_gen` 已變則丟棄舊進度與未發布 `done_target`；(§6) 兩條 DSP 路徑的讀寫責任同步調整。重新送審。
- v24（2026-11-25）：吸收設計審查 Security v4 SE1-a：電台路徑由 `next()`（實際逐樣本取樣出口）而非 `fill()` 推進進度與完成通知，刪除 `fill()` 更新淡段狀態的指示；§1.5、§6、§8 資料變更、§13 實作步驟第 4 點同步調整；`Arc<FadeGate>` 全部統一為 `Arc<Mutex<FadeGate>>`。重新送審。
- v25（2026-11-25）：吸收設計審查 Security v5 SE1-a 衍生：FadeGate 增益僅於 `next()`（實際逐樣本取樣出口）套用一次，`fill()` 不套用 fade gain；§235、§263 同步調整。重新送審。
