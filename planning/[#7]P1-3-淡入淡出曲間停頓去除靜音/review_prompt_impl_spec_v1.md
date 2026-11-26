# P1-3 Implementation v1 — Implementation 審查請求 (Spec 階段)

## 背景
SonicLedger：Tauri v2＋Rust＋React 可攜 Windows 音樂播放器。本輪審查**實作**（Implementation）的 Spec 階段，確認程式碼正確反映 Design.md 的需求與完成條件，有沒有缺漏或偏離設計。審查時只讀，不得改 repo。

## 審查對象
- 設計文件：`planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md` (v25)
- 實作程式碼：`src-tauri/src/player.rs`、`src-tauri/src/dsp.rs`、`src-tauri/src/models.rs`、`src-tauri/src/eq.rs`、`src-tauri/src/playback_pipeline.rs`、`src-tauri/src/commands.rs`、`src-tauri/src/main.rs`、`src-tauri/build.rs`、`src-tauri/capabilities/default.json`、`src-tauri/src/db.rs`、`src/types.ts`、`src/lib/api.ts`、`src/player/Shell.tsx`、`src/player/Tools.tsx`

## 已驗證的執行結果（供參考，非審查結論）
- `cargo test --release --manifest-path src-tauri/Cargo.toml`：59/59 passed
- `node --test "src/player/*.test.mjs"`：5/5 passed
- `npx tauri build`：成功，產出 exe + MSI + NSIS
- `git grep -n -i -e 'a[i]mp'`：結果為空

## 審查重點（Spec 階段）
1. **需求對照（R1–R6）**：實作是否正確覆蓋 Design.md §需求對照表的六項需求（淡入淡出、進度條、手動切歌、曲間停頓、去除靜音、睡眠計時淡出）？
2. **完成條件**：Design.md §完成條件的 9 項是否都被實作並驗證？
3. **設計一致性**：實作是否有偏離 Design.md 的契約（例如 fade gain 是否僅於 `next()` 套用一次、`fill()` 不套用；`stream_install_ready` 是否由命令執行緒消費；gap 是否不累計 `played_out_frames`）？
4. **新控制項**：前端是否有無法驅動引擎的控制項？(AGENTS.md 規則：控制項要能驅動引擎才可以畫)
5. **參考播放器名稱**：文件與程式碼是否未出現禁止名稱？

## 輸出格式要求
`# | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論` markdown 表，嚴重度為 阻擋／重要／次要。
最後一行：`結論：通過 / 有條件通過 / 不通過`
