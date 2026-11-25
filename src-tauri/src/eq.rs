//! 18 段等化器（對標 參考播放器 的 18 段、±15 dB、前級、開關）。
//!
//! 參數放在共享的 `EqShared`，播放中的音源每個 frame 檢查版本號，
//! 拖推桿只換係數、不重開音源，所以不會斷音。
//! 各段之間有重疊，直接把推桿 dB 丟給 peaking 濾波器會疊出比推桿大得多的增益
//!（18 段全 +12 實測中心點會到 +25 dB）。`design_gains` 解互相干擾矩陣，
//! 讓每段中心頻率的實際響應等於推桿值；畫面上的曲線也用同一套算出來。

use std::f64::consts::PI;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

pub const BANDS: usize = 18;
pub const EQ_HZ: [f32; BANDS] = [
    31.0, 63.0, 87.0, 125.0, 175.0, 250.0, 350.0, 500.0, 700.0, 1000.0, 1400.0, 2000.0, 2800.0, 4000.0, 5600.0,
    8000.0, 11200.0, 16000.0,
];
pub const GAIN_LIMIT_DB: f32 = 15.0;
/// 半八度間距用 Q=2.0：比 2.87 平滑，互相干擾交給 design_gains 補償。
const Q: f64 = 2.0;
/// 補償後內部增益的上限。極端鋸齒曲線會要求很大的內部增益，夾住避免濾波器發散。
const INTERNAL_LIMIT_DB: f64 = 30.0;

#[derive(Clone, Debug, PartialEq)]
pub struct EqParams {
    pub enabled: bool,
    pub bands: [f32; BANDS],
    pub preamp_db: f32,
    pub normalize: bool,
    pub rg_preamp_has: f32,
    pub rg_preamp_none: f32,
    pub rg_mode: crate::models::ReplayGainMode,
    pub volume_norm_mode: crate::models::VolumeNormalization,
    pub is_album_playback_context: bool,
    pub rg_peak: Option<f32>,
    /// 等化器以外的音效（速度、殘響…）。跟等化器共用版本號，一起即時生效。
    pub fx: crate::dsp::FxParams,
    /// 音量曲線：Log / Linear
    pub volume_curve: crate::models::VolumeCurve,
    /// 響度補償：是否啟用
    pub loudness_enabled: bool,
    /// 響度補償：低頻擱架頻率 (Hz)
    pub loudness_bass_hz: f32,
    /// 響度補償：低頻擱架增益
    pub loudness_bass_db: f32,
    /// 響度補償：高頻擱架頻率 (Hz)
    pub loudness_treble_hz: f32,
    /// 響度補償：高頻擱架增益
    pub loudness_treble_db: f32,
}

impl Default for EqParams {
    fn default() -> Self {
        Self {
            enabled: true,
            bands: [0.0; BANDS],
            preamp_db: 0.0,
            normalize: false,
            rg_preamp_has: 0.0,
            rg_preamp_none: -6.0,
            rg_mode: crate::models::ReplayGainMode::default(),
            volume_norm_mode: crate::models::VolumeNormalization::default(),
            is_album_playback_context: false,
            rg_peak: None,
            fx: crate::dsp::FxParams::default(),
            volume_curve: crate::models::VolumeCurve::default(),
            loudness_enabled: false,
            loudness_bass_hz: 150.0,
            loudness_bass_db: 0.0,
            loudness_treble_hz: 6000.0,
            loudness_treble_db: 0.0,
        }
    }
}

pub struct EqShared {
    params: Mutex<EqParams>,
    version: AtomicU64,
}

impl EqShared {
    pub fn new() -> Self {
        Self { params: Mutex::new(EqParams::default()), version: AtomicU64::new(1) }
    }

    pub fn get(&self) -> EqParams {
        self.params.lock().map(|p| p.clone()).unwrap_or_default()
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    pub fn update(&self, f: impl FnOnce(&mut EqParams)) {
        if let Ok(mut p) = self.params.lock() {
            f(&mut p);
            for b in p.bands.iter_mut() {
                *b = if b.is_finite() { b.clamp(-GAIN_LIMIT_DB, GAIN_LIMIT_DB) } else { 0.0 };
            }
            p.preamp_db = if p.preamp_db.is_finite() { p.preamp_db.clamp(-GAIN_LIMIT_DB, GAIN_LIMIT_DB) } else { 0.0 };
            p.fx = p.fx.clone().sanitized();
        }
        self.version.fetch_add(1, Ordering::AcqRel);
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    pub(crate) b0: f32,
    pub(crate) b1: f32,
    pub(crate) b2: f32,
    pub(crate) a1: f32,
    pub(crate) a2: f32,
    pub(crate) z1: f32,
    pub(crate) z2: f32,
}

#[derive(Clone, Copy)]
struct Coef {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

fn peaking(sample_rate: f64, freq: f64, gain_db: f64) -> Option<Coef> {
    if gain_db.abs() < 0.01 || sample_rate <= 0.0 || freq >= sample_rate * 0.48 {
        return None;
    }
    let a = 10f64.powf(gain_db / 40.0);
    let w0 = 2.0 * PI * freq / sample_rate;
    let alpha = w0.sin() / (2.0 * Q);
    let cosw = w0.cos();
    let a0 = 1.0 + alpha / a;
    Some(Coef {
        b0: (1.0 + alpha * a) / a0,
        b1: (-2.0 * cosw) / a0,
        b2: (1.0 - alpha * a) / a0,
        a1: (-2.0 * cosw) / a0,
        a2: (1.0 - alpha / a) / a0,
    })
}

fn coef_db(c: &Coef, sample_rate: f64, freq: f64) -> f64 {
    let w = 2.0 * PI * freq / sample_rate;
    let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
    let nr = c.b0 + c.b1 * c1 + c.b2 * c2;
    let ni = -(c.b1 * s1 + c.b2 * s2);
    let dr = 1.0 + c.a1 * c1 + c.a2 * c2;
    let di = -(c.a1 * s1 + c.a2 * s2);
    10.0 * ((nr * nr + ni * ni) / (dr * dr + di * di)).log10()
}

fn chain_db(internal: &[f64; BANDS], sample_rate: f64, freq: f64) -> f64 {
    EQ_HZ
        .iter()
        .zip(internal.iter())
        .filter_map(|(hz, g)| peaking(sample_rate, *hz as f64, *g))
        .map(|c| coef_db(&c, sample_rate, freq))
        .sum()
}

fn solve(mut m: [[f64; BANDS]; BANDS], mut y: [f64; BANDS]) -> Option<[f64; BANDS]> {
    for c in 0..BANDS {
        let p = (c..BANDS).max_by(|a, b| m[*a][c].abs().total_cmp(&m[*b][c].abs()))?;
        if m[p][c].abs() < 1e-9 {
            return None;
        }
        m.swap(c, p);
        y.swap(c, p);
        for r in 0..BANDS {
            if r != c {
                let k = m[r][c] / m[c][c];
                for j in c..BANDS {
                    m[r][j] -= k * m[c][j];
                }
                y[r] -= k * y[c];
            }
        }
    }
    let mut out = [0.0; BANDS];
    for i in 0..BANDS {
        out[i] = y[i] / m[i][i];
    }
    Some(out)
}

/// 推桿值 → 實際丟給濾波器的增益，使中心頻率的響應等於推桿值。
pub fn design_gains(bands: &[f32; BANDS], sample_rate: f64) -> [f64; BANDS] {
    let target: [f64; BANDS] = std::array::from_fn(|i| bands[i] as f64);
    if target.iter().all(|g| g.abs() < 0.01) {
        return [0.0; BANDS];
    }
    // 超出取樣率能表達的頻段（例如 22.05k 取樣的 11.2k/16k）不參與求解。
    let usable: [bool; BANDS] = std::array::from_fn(|i| (EQ_HZ[i] as f64) < sample_rate * 0.48);
    let mut g = target;
    for _ in 0..3 {
        let mut m = [[0.0; BANDS]; BANDS];
        for j in 0..BANDS {
            if !usable[j] {
                m[j][j] = 1.0;
                continue;
            }
            let probe = if g[j].abs() > 0.5 { g[j] } else { 1.0 };
            let c = peaking(sample_rate, EQ_HZ[j] as f64, probe);
            for i in 0..BANDS {
                if usable[i] {
                    m[i][j] = c.as_ref().map(|c| coef_db(c, sample_rate, EQ_HZ[i] as f64)).unwrap_or(0.0) / probe;
                }
            }
        }
        let mut y = target;
        for i in 0..BANDS {
            if !usable[i] {
                y[i] = 0.0;
            }
        }
        match solve(m, y) {
            Some(next) => g = next.map(|x| x.clamp(-INTERNAL_LIMIT_DB, INTERNAL_LIMIT_DB)),
            None => return target,
        }
    }
    g
}

pub fn make_chain(bands: &[f32; BANDS], sample_rate: u32) -> Vec<Biquad> {
    let fs = sample_rate as f64;
    design_gains(bands, fs)
        .iter()
        .zip(EQ_HZ.iter())
        .filter_map(|(g, hz)| peaking(fs, *hz as f64, *g))
        .map(|c| Biquad { b0: c.b0 as f32, b1: c.b1 as f32, b2: c.b2 as f32, a1: c.a1 as f32, a2: c.a2 as f32, z1: 0.0, z2: 0.0 })
        .collect()
}

impl Biquad {
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    /// 換係數但保留延遲狀態，拖推桿時不會從零重啟濾波器。
    fn retune(&mut self, next: &Biquad) {
        self.b0 = next.b0;
        self.b1 = next.b1;
        self.b2 = next.b2;
        self.a1 = next.a1;
        self.a2 = next.a2;
    }
}

pub fn db_to_linear(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// 每個聲道一條濾波鏈，跟著 EqShared 的版本即時換係數。
pub struct EqProcessor {
    shared: std::sync::Arc<EqShared>,
    seen: u64,
    sample_rate: u32,
    channels: usize,
    pub(crate) chains: Vec<Vec<Biquad>>,
    enabled: bool,
    norm_gain: f32,
    gain_target: f32,
    gain_now: f32,
    idx: usize,
    rg_preamp_has: f32,
    rg_preamp_none: f32,
    rg_mode: crate::models::ReplayGainMode,
    volume_norm_mode: crate::models::VolumeNormalization,
    rg_track_gain_db: Option<f32>,
    rg_track_peak: Option<f32>,
    rg_album_gain_db: Option<f32>,
    rg_album_peak: Option<f32>,
    has_rg_track: bool,
    has_rg_album: bool,
    has_rg: bool,
    is_album_playback_context: bool,
    current_rg_preamp_db: f32,
    current_rg_source: String,
}

impl EqProcessor {
    pub fn new(shared: std::sync::Arc<EqShared>, sample_rate: u32, channels: u16, norm_gain: f32) -> Self {
        let params = shared.get();
                let mut p = Self {
                    shared,
                    seen: 0,
                    sample_rate: sample_rate.max(1),
                    channels: channels.max(1) as usize,
                    chains: Vec::new(),
                    enabled: true,
                    norm_gain,
                    gain_target: 1.0,
                    gain_now: 1.0,
                    idx: 0,
                    rg_preamp_has: params.rg_preamp_has,
                    rg_preamp_none: params.rg_preamp_none,
                    rg_mode: params.rg_mode,
                    volume_norm_mode: params.volume_norm_mode,
                    rg_track_gain_db: None,
                    rg_track_peak: None,
                    rg_album_gain_db: None,
                    rg_album_peak: None,
                    has_rg_track: false,
                    has_rg_album: false,
                    has_rg: false,
                    is_album_playback_context: params.is_album_playback_context,
                    current_rg_preamp_db: 0.0,
                    current_rg_source: "Unknown".to_string(),
                };
        p.refresh();
        p.gain_now = p.gain_target;
        p
    }

    fn refresh(&mut self) {
            let v = self.shared.version();
            if v == self.seen {
                return;
            }
            self.seen = v;
            let params = self.shared.get();

            // 同步前級設定（來自 EqParams）
            self.rg_preamp_has = params.rg_preamp_has;
            self.rg_preamp_none = params.rg_preamp_none;
            if params.rg_mode != self.rg_mode {
                // 若 shared 帶入新模式，更新（主要由 set_rg_context 設定）
                self.rg_mode = params.rg_mode;
            }
            // 同步音量正規化模式與專輯播放上下文
            self.volume_norm_mode = params.volume_norm_mode;
            self.is_album_playback_context = params.is_album_playback_context;

            // 決定 norm_gain 來源與前級
            match self.volume_norm_mode {
            crate::models::VolumeNormalization::Peak => {
                let eq_preamp = if params.enabled { db_to_linear(params.preamp_db) } else { 1.0 };
                self.gain_target = self.norm_gain * eq_preamp;
                self.has_rg = false;
                self.current_rg_preamp_db = params.preamp_db;
                self.current_rg_source = "Peak".to_string();
            }
            crate::models::VolumeNormalization::Off => {
                let eq_preamp = if params.enabled { db_to_linear(params.preamp_db) } else { 1.0 };
                self.gain_target = 1.0 * eq_preamp;
                self.has_rg = false;
                self.current_rg_preamp_db = 0.0;
                self.current_rg_source = "Off".to_string();
            }
            crate::models::VolumeNormalization::Replaygain => {
                if matches!(self.rg_mode, crate::models::ReplayGainMode::Off) {
                    let eq_preamp = if params.enabled { db_to_linear(params.preamp_db) } else { 1.0 };
                    self.gain_target = 1.0 * eq_preamp;
                    self.has_rg = false;
                    self.current_rg_preamp_db = 0.0;
                    self.current_rg_source = "RG_Off".to_string();
                } else {
                    let (selected_gain_db, selected_peak, rg_source, has_rg) = match self.rg_mode {
                        crate::models::ReplayGainMode::Track => {
                            if self.has_rg_track {
                                (self.rg_track_gain_db, self.rg_track_peak, "RG_Track".to_string(), true)
                            } else {
                                (None, None, "RG_None".to_string(), false)
                            }
                        }
                        crate::models::ReplayGainMode::Album => {
                            if self.has_rg_album {
                                (self.rg_album_gain_db, self.rg_album_peak, "RG_Album".to_string(), true)
                            } else if self.has_rg_track {
                                (self.rg_track_gain_db, self.rg_track_peak, "RG_Track_Fallback".to_string(), true)
                            } else {
                                (None, None, "RG_None".to_string(), false)
                            }
                        }
                        crate::models::ReplayGainMode::Auto => {
                            let is_album_ctx = self.is_album_playback_context;
                            if is_album_ctx && self.has_rg_album {
                                (self.rg_album_gain_db, self.rg_album_peak, "RG_Album".to_string(), true)
                            } else if self.has_rg_track {
                                let source = if is_album_ctx { "RG_Track_Fallback".to_string() } else { "RG_Track".to_string() };
                                (self.rg_track_gain_db, self.rg_track_peak, source, true)
                            } else {
                                (None, None, "RG_None".to_string(), false)
                            }
                        }
                        crate::models::ReplayGainMode::Off => unreachable!(),
                    };

                    let (requested_gain_linear, rg_preamp_db) = if has_rg {
                        let gain_with_preamp = selected_gain_db.unwrap_or(0.0) + self.rg_preamp_has;
                        (db_to_linear(gain_with_preamp), self.rg_preamp_has)
                    } else {
                        (db_to_linear(self.rg_preamp_none), self.rg_preamp_none)
                    };

                    let norm_gain_final = if let Some(peak) = selected_peak {
                        let max_gain = 1.0 / peak.max(0.0001);
                        requested_gain_linear.min(max_gain)
                    } else {
                        requested_gain_linear
                    };

                    let eq_preamp = if params.enabled { db_to_linear(params.preamp_db) } else { 1.0 };
                    self.gain_target = norm_gain_final * eq_preamp;
                    self.has_rg = has_rg;
                    self.current_rg_preamp_db = rg_preamp_db;
                    self.current_rg_source = rg_source;
                }
            }
        };

        let chain = if params.enabled { make_chain(&params.bands, self.sample_rate) } else { Vec::new() };
        let reuse = self.enabled == params.enabled && self.chains.first().map(|c| c.len()) == Some(chain.len());
        if reuse {
            for ch in self.chains.iter_mut() {
                for (f, n) in ch.iter_mut().zip(chain.iter()) {
                    f.retune(n);
                }
            }
        } else {
            self.chains = (0..self.channels).map(|_| chain.clone()).collect();
        }
        self.enabled = params.enabled;
    }

    pub fn set_rg_context(&mut self, track_gain: Option<f32>, track_peak: Option<f32>,
                          album_gain: Option<f32>, album_peak: Option<f32>,
                          rg_mode: crate::models::ReplayGainMode, volume_norm_mode: crate::models::VolumeNormalization,
                          is_album_playback: bool) {
        self.rg_mode = rg_mode;
        self.volume_norm_mode = volume_norm_mode;
        self.rg_track_gain_db = track_gain;
        self.rg_track_peak = track_peak;
        self.rg_album_gain_db = album_gain;
        self.rg_album_peak = album_peak;
        self.has_rg_track = track_gain.is_some();
        self.has_rg_album = album_gain.is_some();
        self.is_album_playback_context = is_album_playback;
        self.seen = 0;
    }

    pub fn set_norm_gain(&mut self, norm_gain: f32) {
        self.norm_gain = norm_gain;
        self.seen = 0;
    }

    pub fn gain_target(&self) -> f32 { self.gain_target }
    pub fn has_rg(&self) -> bool { self.has_rg }
    pub fn current_rg_preamp_db(&self) -> f32 { self.current_rg_preamp_db }
    pub fn current_rg_source(&self) -> String { self.current_rg_source.clone() }
    pub fn rg_mode(&self) -> crate::models::ReplayGainMode { self.rg_mode }
    pub fn volume_norm_mode(&self) -> crate::models::VolumeNormalization { self.volume_norm_mode }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let ch = self.idx % self.channels;
        self.idx = self.idx.wrapping_add(1);
        if ch == 0 {
            self.refresh();
            // 前級／正規化變更用約 10ms 的平滑，避免拖桿時爆音。
            self.gain_now += (self.gain_target - self.gain_now) * 0.004;
        }
        let mut s = x * self.gain_now;
        if let Some(chain) = self.chains.get_mut(ch) {
            for f in chain.iter_mut() {
                s = f.process(s);
            }
        }
        // 不在這裡限幅：後面的音效還要處理，最後由 DspSource 統一限幅。
        if s.is_finite() { s } else { 0.0 }
    }
}

/// 畫面曲線：用與音訊執行緒相同的設計算 20Hz–20kHz 響應（含前級）。
pub fn response_curve(params: &EqParams, sample_rate: u32, points: usize) -> Vec<[f32; 2]> {
    let fs = sample_rate.max(1) as f64;
    let internal = if params.enabled { design_gains(&params.bands, fs) } else { [0.0; BANDS] };
    let pre = if params.enabled { params.preamp_db as f64 } else { 0.0 };
    let n = points.clamp(16, 512);
    let top = 20000f64.min(fs * 0.48);
    (0..n)
        .map(|k| {
            let f = 20.0 * (top / 20.0).powf(k as f64 / (n - 1) as f64);
            [f as f32, (chain_db(&internal, fs, f) + pre) as f32]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(bands: &[f32; BANDS], fs: u32, f: f64) -> f64 {
        chain_db(&design_gains(bands, fs as f64), fs as f64, f)
    }

    #[test]
    fn center_response_matches_sliders() {
        let patterns: [[f32; BANDS]; 4] = [
            [12.0; BANDS],
            std::array::from_fn(|i| if i % 3 == 0 { 9.0 } else { -4.0 }),
            std::array::from_fn(|i| if i < 5 { 15.0 - i as f32 * 3.0 } else { 0.0 }),
            std::array::from_fn(|i| if i == 9 { 15.0 } else { 0.0 }),
        ];
        for fs in [44100u32, 48000, 96000] {
            for p in patterns.iter() {
                for i in 0..BANDS {
                    let got = at(p, fs, EQ_HZ[i] as f64);
                    assert!((got - p[i] as f64).abs() < 0.6, "fs={fs} band {i}: want {} got {got:.2}", p[i]);
                }
            }
        }
    }

    #[test]
    fn flat_is_bypass() {
        assert!(make_chain(&[0.0; BANDS], 44100).is_empty());
    }

    #[test]
    fn live_update_changes_output_without_new_processor() {
        let shared = std::sync::Arc::new(EqShared::new());
        let mut p = EqProcessor::new(shared.clone(), 44100, 1, 1.0);
        let tone = |n: usize| (2.0 * std::f32::consts::PI * 63.0 * n as f32 / 44100.0).sin() * 0.05;
        let rms = |p: &mut EqProcessor, start: usize| {
            let mut acc = 0.0f32;
            for n in start..start + 44100 {
                let y = p.process(tone(n));
                if n >= start + 22050 {
                    acc += y * y;
                }
            }
            (acc / 22050.0).sqrt()
        };
        let flat = rms(&mut p, 0);
        shared.update(|e| e.bands[1] = 12.0);
        let boosted = rms(&mut p, 44100);
        let db = 20.0 * (boosted / flat).log10();
        assert!((db - 12.0).abs() < 1.0, "63Hz +12 dB should be audible live, got {db:.2} dB");
        shared.update(|e| e.enabled = false);
        let off = rms(&mut p, 88200);
        assert!((20.0 * (off / flat).log10()).abs() < 0.3, "disabled EQ is bypass");
    }

    #[test]
    fn preamp_applies() {
        let shared = std::sync::Arc::new(EqShared::new());
        shared.update(|e| e.preamp_db = -6.0);
        let mut p = EqProcessor::new(shared, 44100, 2, 1.0);
        let y = p.process(0.5);
        assert!((y - 0.5 * db_to_linear(-6.0)).abs() < 1e-3);
    }
}
