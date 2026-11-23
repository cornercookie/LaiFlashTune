#requires -Version 3.0
# 產生本期驗收用的格式樣本（3 秒、1 kHz 立體聲正弦波，固定標籤）。
# 放置於 src-tauri/tests/fixtures/formats/，由腳本一併 commit。
#
# 需求：本機 ffmpeg（可編碼 mp3/mp2/aac/alac/flac/wav/vorbis/opus/speex/wavpack/tta/wmav2/ac3）。
# APE 例外：ffmpeg 無 APE 編碼器，改用 Monkey's Audio 官方命令列工具 MAC.exe。
#
# 使用方式（以系統管理員或一般使用者於 repo 根目錄執行）：
#   pwsh -File src-tauri/tests/fixtures/formats/make-format-fixtures.ps1
#
# 輸出：src-tauri/tests/fixtures/formats/*.<ext> 與 SOURCES.md

$ErrorActionPreference = 'Stop'
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$outDir = Join-Path $scriptDir 'formats'
if (-not (Test-Path $outDir)) { New-Item -ItemType Directory -Path $outDir | Out-Null }

$duration = 3
$freq = 1000
$rate = 44100
$channels = 2

# 尋找 ffmpeg / ffprobe：exe 旁 bin\\ → PATH
function Find-Tool($name) {
  $candidates = @()
  $exe = [System.Diagnostics.Process]::GetCurrentProcess().MainModule.FileName
  $exeDir = Split-Path -Parent $exe
  $candidates += Join-Path $exeDir $name
  $candidates += Join-Path (Join-Path $exeDir 'bin') $name
  $env:PATH.Split(';') | Where-Object { $_ -ne '' } | ForEach-Object { $candidates += Join-Path $_ $name }
  foreach ($c in $candidates) {
    if (Test-Path $c) { return $c }
  }
  return $null
}

$ffmpeg = Find-Tool 'ffmpeg.exe'
if (-not $ffmpeg) {
  Write-Warning '找不到 ffmpeg.exe，無法產生 ffmpeg 可編碼的樣本。'
} else {
  Write-Host "使用 ffmpeg: $ffmpeg"
}

function Gen-Sine($ext, $extraArgs = @()) {
  if (-not $ffmpeg) { return }
  $out = Join-Path $outDir "sample.$ext"
  $args = @('-y', '-f', 'lavfi', '-i', "sine=frequency=$freq:sample_rate=$rate:duration=$duration", '-ar', $rate, '-ac', $channels) + $extraArgs + @($out)
  & $ffmpeg @args | Out-Null
  if (Test-Path $out) { Write-Host "產生 $out" } else { Write-Warning "產生 $out 失敗" }
}

# ffmpeg 本機可編碼的格式
Gen-Sine 'mp3'
Gen-Sine 'mp2'
Gen-Sine 'aac'
Gen-Sine 'alac' @('-c:a', 'alac')
Gen-Sine 'flac'
Gen-Sine 'wav'
Gen-Sine 'ogg' @('-c:a', 'libvorbis')
Gen-Sine 'opus'
Gen-Sine 'spx' @('-c:a', 'libspeex')
Gen-Sine 'wv' @('-c:a', 'wavpack')
Gen-Sine 'tta' @('-c:a', 'tta')
Gen-Sine 'wmav2' @('-c:a', 'wmav2')
Gen-Sine 'ac3'

# APE：ffmpeg 無編碼器，需 MAC.exe
$mac = Find-Tool 'MAC.exe'
if ($mac) {
  $wav = Join-Path $outDir 'sample.wav'
  if (-not (Test-Path $wav)) { Gen-Sine 'wav' }
  $ape = Join-Path $outDir 'ape.ape'
  & $mac $wav $ape '-c2000' | Out-Null
  if (Test-Path $ape) { Write-Host "產生 $ape (via MAC.exe)" } else { Write-Warning '產生 APE 失敗' }
  # 用 lofty 寫入標籤（需 Rust 環境）略，此處只產生音檔
} else {
  Write-Warning '找不到 MAC.exe，APE 樣本需手動產生（請參閱 SOURCES.md）。'
}

# SOURCES.md 記錄外部樣本來源（ATRAC、TAK 等 ffmpeg 無法編碼的格式）
$sources = @"
# 格式樣本來源與授權

本資料夾置放驗收用的音訊樣本。由 `make-format-fixtures.ps1` 產生的 ffmpeg 可編碼樣本
（mp3/mp2/aac/alac/flac/wav/ogg/opus/spx/wv/tta/wmav2/ac3）為本腳本以 1 kHz 正弦波合成，
無第三方授權限制。

外部樣本（ffmpeg 無法編碼，需另行尋找授權可用的檔案）：
- ATRAC (.aa3/.at3/.oma)：待尋找授權可用樣本，記於此。
- TAK (.tak)：待尋找授權可用樣本，記於此。
- OptimFROG (.ofr)：受阻格式，不產生樣本（記在 ROADMAP 待辦）。
- MIDI (.mid/.rmi)：需要 SoundFont，不在本期驗收。
- 音樂 CD (.cda)：需要 CD 存取，不在本期驗收。

每個外部樣本須記錄：來源、授權、ffprobe 量到的長度與標籤。
"@
$sources | Out-File -FilePath (Join-Path $outDir 'SOURCES.md') -Encoding utf8
Write-Host "完成。樣本位於 $outDir"
