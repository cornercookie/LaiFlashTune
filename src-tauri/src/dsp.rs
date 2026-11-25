//! 音效：對標 參考播放器 的十種內建效果（殘響、鑲邊、合唱、音高、節奏、回音、速度、低音、高音增強、人聲消除）。
//!
//! 參數跟等化器一起放在 `EqShared`，播放中每個區塊（1024 frame）讀版本號即時套用。
//! 速度／節奏／音高會改變輸出樣本數：節奏用 WSOLA（與 SoundTouch 同一類演算法）伸縮時間不改音高，
//! 速度與音高用三次內插重取樣。三者都在原值時整段旁路，輸出與輸入逐樣本相同。
//! 播放位置以「已消耗的輸入 frame」計算，改速度時進度條仍對應歌曲時間。

use crate::eq::{EqProcessor, EqShared};
use rodio::Source;
use serde::{Deserialize, Serialize};
use std::f32::consts::PI;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FxParams {
    /// 0.5–2.0，像磁帶變速：音高與節奏一起變。
    pub speed: f32,
    /// 0.5–2.0，只改節奏不改音高。
    pub tempo: f32,
    /// 半音，−12 到 +12，不改節奏。
    pub pitch: f32,
    pub bass_on: bool,
    pub bass_db: f32,
    pub bass_hz: f32,
    pub enhancer_on: bool,
    pub enhancer: f32,
    pub voice_remover: bool,
    pub chorus_on: bool,
    pub chorus_depth: f32,
    pub chorus_rate: f32,
    pub chorus_mix: f32,
    pub flanger_on: bool,
    pub flanger_depth: f32,
    pub flanger_rate: f32,
    pub flanger_feedback: f32,
    pub flanger_mix: f32,
    pub echo_on: bool,
    pub echo_ms: f32,
    pub echo_feedback: f32,
    pub echo_mix: f32,
    pub reverb_on: bool,
    pub reverb_room: f32,
    pub reverb_damp: f32,
    pub reverb_mix: f32,
}

impl Default for FxParams {
    fn default() -> Self {
        Self {
            speed: 1.0,
            tempo: 1.0,
            pitch: 0.0,
            bass_on: false,
            bass_db: 6.0,
            bass_hz: 100.0,
            enhancer_on: false,
            enhancer: 0.3,
            voice_remover: false,
            chorus_on: false,
            chorus_depth: 0.5,
            chorus_rate: 1.0,
            chorus_mix: 0.5,
            flanger_on: false,
            flanger_depth: 0.6,
            flanger_rate: 0.3,
            flanger_feedback: 0.5,
            flanger_mix: 0.5,
            echo_on: false,
            echo_ms: 300.0,
            echo_feedback: 0.35,
            echo_mix: 0.35,
            reverb_on: false,
            reverb_room: 0.5,
            reverb_damp: 0.5,
            reverb_mix: 0.3,
        }
    }
}

fn fin(v: f32, default: f32, lo: f32, hi: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { default }
}

impl FxParams {
    pub fn sanitized(self) -> Self {
        let d = Self::default();
        Self {
            speed: fin(self.speed, d.speed, 0.5, 2.0),
            tempo: fin(self.tempo, d.tempo, 0.5, 2.0),
            pitch: fin(self.pitch, d.pitch, -12.0, 12.0),
            bass_db: fin(self.bass_db, d.bass_db, 0.0, 15.0),
            bass_hz: fin(self.bass_hz, d.bass_hz, 40.0, 250.0),
            enhancer: fin(self.enhancer, d.enhancer, 0.0, 1.0),
            chorus_depth: fin(self.chorus_depth, d.chorus_depth, 0.0, 1.0),
            chorus_rate: fin(self.chorus_rate, d.chorus_rate, 0.1, 5.0),
            chorus_mix: fin(self.chorus_mix, d.chorus_mix, 0.0, 1.0),
            flanger_depth: fin(self.flanger_depth, d.flanger_depth, 0.0, 1.0),
            flanger_rate: fin(self.flanger_rate, d.flanger_rate, 0.05, 2.0),
            flanger_feedback: fin(self.flanger_feedback, d.flanger_feedback, 0.0, 0.9),
            flanger_mix: fin(self.flanger_mix, d.flanger_mix, 0.0, 1.0),
            echo_ms: fin(self.echo_ms, d.echo_ms, 20.0, 2000.0),
            echo_feedback: fin(self.echo_feedback, d.echo_feedback, 0.0, 0.9),
            echo_mix: fin(self.echo_mix, d.echo_mix, 0.0, 1.0),
            reverb_room: fin(self.reverb_room, d.reverb_room, 0.0, 1.0),
            reverb_damp: fin(self.reverb_damp, d.reverb_damp, 0.0, 1.0),
            reverb_mix: fin(self.reverb_mix, d.reverb_mix, 0.0, 1.0),
            ..self
        }
    }

    fn rates(&self) -> (f64, f64) {
        // 重取樣率 r 決定音高；WSOLA 伸縮率 sw 補足剩下的節奏。總消耗速率 = speed × tempo。
        let p = 2f64.powf(self.pitch as f64 / 12.0);
        let r = self.speed as f64 * p;
        let sw = self.tempo as f64 / p;
        (r, sw)
    }
}

/* ---------- 基本濾波器 ---------- */

#[derive(Clone, Copy, Default)]
pub struct Bq {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Bq {
    pub fn from(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> Self {
        Self { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0, z1: 0.0, z2: 0.0 }
    }
    pub fn lowpass(sr: f32, f: f32) -> Self {
        let w = 2.0 * PI * f / sr;
        let (c, al) = (w.cos(), w.sin() / (2.0 * 0.7071));
        Self::from((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0, 1.0 + al, -2.0 * c, 1.0 - al)
    }
    pub fn highpass(sr: f32, f: f32) -> Self {
        let w = 2.0 * PI * f / sr;
        let (c, al) = (w.cos(), w.sin() / (2.0 * 0.7071));
        Self::from((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0, 1.0 + al, -2.0 * c, 1.0 - al)
    }
    pub fn lowshelf(sr: f32, f: f32, db: f32) -> Self {
        let a = 10f32.powf(db / 40.0);
        let w = 2.0 * PI * f / sr;
        let (c, s) = (w.cos(), w.sin());
        let al = s / 2.0 * 2f32.sqrt();
        let sa = 2.0 * a.sqrt() * al;
        Self::from(
            a * ((a + 1.0) - (a - 1.0) * c + sa),
            2.0 * a * ((a - 1.0) - (a + 1.0) * c),
            a * ((a + 1.0) - (a - 1.0) * c - sa),
            (a + 1.0) + (a - 1.0) * c + sa,
            -2.0 * ((a - 1.0) + (a + 1.0) * c),
            (a + 1.0) + (a - 1.0) * c - sa,
        )
    }
    pub fn highshelf(sr: f32, f: f32, db: f32) -> Self {
        let a = 10f32.powf(db / 40.0);
        let w = 2.0 * PI * f / sr;
        let (c, s) = (w.cos(), w.sin());
        let al = s / 2.0 * 2f32.sqrt();
        let sa = 2.0 * a.sqrt() * al;
        Self::from(
            a * ((a + 1.0) + (a - 1.0) * c + sa),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
            a * ((a + 1.0) + (a - 1.0) * c - sa),
            (a + 1.0) - (a - 1.0) * c + sa,
            2.0 * ((a - 1.0) - (a + 1.0) * c),
            (a + 1.0) - (a - 1.0) * c - sa,
        )
    }
    pub fn retune(&mut self, n: Bq) {
        self.b0 = n.b0;
        self.b1 = n.b1;
        self.b2 = n.b2;
        self.a1 = n.a1;
        self.a2 = n.a2;
    }
    #[inline]
    pub fn run(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

struct DelayLine {
    buf: Vec<f32>,
    idx: usize,
}

impl DelayLine {
    fn new(len: usize) -> Self {
        Self { buf: vec![0.0; len.max(4)], idx: 0 }
    }
    /// 讀 `delay` 個樣本之前的值（可為小數，線性內插）。
    #[inline]
    fn read(&self, delay: f32) -> f32 {
        let n = self.buf.len();
        let d = delay.clamp(1.0, (n - 2) as f32);
        let i = d.floor() as usize;
        let f = d - i as f32;
        let a = self.buf[(self.idx + n - i) % n];
        let b = self.buf[(self.idx + n - i - 1) % n];
        a + (b - a) * f
    }
    #[inline]
    fn write(&mut self, x: f32) {
        self.idx = (self.idx + 1) % self.buf.len();
        self.buf[self.idx] = x;
    }
}

/* ---------- Freeverb 殘響 ---------- */

struct Comb {
    buf: Vec<f32>,
    idx: usize,
    store: f32,
}

impl Comb {
    #[inline]
    fn run(&mut self, x: f32, fb: f32, damp: f32) -> f32 {
        let out = self.buf[self.idx];
        self.store = out * (1.0 - damp) + self.store * damp;
        self.buf[self.idx] = x + self.store * fb;
        self.idx = (self.idx + 1) % self.buf.len();
        out
    }
}

struct Allpass {
    buf: Vec<f32>,
    idx: usize,
}

impl Allpass {
    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let b = self.buf[self.idx];
        self.buf[self.idx] = x + b * 0.5;
        self.idx = (self.idx + 1) % self.buf.len();
        b - x
    }
}

struct Reverb {
    combs: Vec<Vec<Comb>>,
    allpasses: Vec<Vec<Allpass>>,
}

impl Reverb {
    fn new(sr: u32, channels: usize) -> Self {
        const COMB: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
        const AP: [usize; 4] = [556, 441, 341, 225];
        let k = sr as f32 / 44100.0;
        let ch = channels.min(2).max(1);
        let size = |n: usize, c: usize| (((n + c * 23) as f32) * k).round().max(8.0) as usize;
        Self {
            combs: (0..ch).map(|c| COMB.iter().map(|n| Comb { buf: vec![0.0; size(*n, c)], idx: 0, store: 0.0 }).collect()).collect(),
            allpasses: (0..ch).map(|c| AP.iter().map(|n| Allpass { buf: vec![0.0; size(*n, c)], idx: 0 }).collect()).collect(),
        }
    }
    fn run(&mut self, input: f32, c: usize, fb: f32, damp: f32) -> f32 {
        let c = c.min(self.combs.len() - 1);
        let mut out = 0.0;
        for comb in self.combs[c].iter_mut() {
            out += comb.run(input, fb, damp);
        }
        for ap in self.allpasses[c].iter_mut() {
            out = ap.run(out);
        }
        out
    }
}

/* ---------- 效果鏈 ---------- */

pub(crate) struct FxChain {
    ch: usize,
    sr: f32,
    pub(crate) p: FxParams,
    bass: Vec<Bq>,
    voice_lp: Bq,
    enh_hp: Vec<Bq>,
    chorus: Vec<DelayLine>,
    chorus_phase: f32,
    flanger: Vec<DelayLine>,
    flanger_phase: f32,
    echo: Vec<DelayLine>,
    reverb: Option<Reverb>,
    sr_u: u32,
}

impl FxChain {
    pub(crate) fn new(sr: u32, channels: usize) -> Self {
        let srf = sr.max(1) as f32;
        let ch = channels.max(1);
        Self {
            ch,
            sr: srf,
            p: FxParams::default(),
            bass: vec![Bq::lowshelf(srf, 100.0, 0.0); ch],
            voice_lp: Bq::lowpass(srf, 160.0),
            enh_hp: vec![Bq::highpass(srf, 2500.0); ch],
            chorus: (0..ch).map(|_| DelayLine::new((srf * 0.05) as usize)).collect(),
            chorus_phase: 0.0,
            flanger: (0..ch).map(|_| DelayLine::new((srf * 0.012) as usize)).collect(),
            flanger_phase: 0.0,
            echo: Vec::new(),
            reverb: None,
            sr_u: sr.max(1),
        }
    }

    pub(crate) fn set(&mut self, p: &FxParams) {
        if p.bass_hz != self.p.bass_hz || p.bass_db != self.p.bass_db || self.bass.is_empty() {
            let n = Bq::lowshelf(self.sr, p.bass_hz, p.bass_db);
            for b in self.bass.iter_mut() {
                b.retune(n);
            }
        }
        if p.echo_on && self.echo.is_empty() {
            self.echo = (0..self.ch).map(|_| DelayLine::new((self.sr * 2.05) as usize)).collect();
        }
        if p.reverb_on && self.reverb.is_none() {
            self.reverb = Some(Reverb::new(self.sr_u, self.ch));
        }
        self.p = p.clone();
    }

    fn active(&self) -> bool {
        let p = &self.p;
        p.bass_on || p.enhancer_on || p.voice_remover || p.chorus_on || p.flanger_on || p.echo_on || p.reverb_on
    }

    pub fn frame(&mut self, f: &mut [f32]) {
        let p = &self.p;
        if p.voice_remover && f.len() >= 2 {
            // 去掉置中的人聲：保留側聲道，中央只留 160Hz 以下讓低音不消失。
            let mid = (f[0] + f[1]) * 0.5;
            let side = (f[0] - f[1]) * 0.5;
            let low = self.voice_lp.run(mid);
            f[0] = low + side;
            f[1] = low - side;
        }
        for (c, s) in f.iter_mut().enumerate() {
            let mut x = *s;
            if p.bass_on {
                x = self.bass[c].run(x);
            }
            if p.enhancer_on {
                let h = self.enh_hp[c].run(x);
                x += p.enhancer * 0.7 * (h * 3.0).tanh();
            }
            if p.chorus_on {
                let lfo = (self.chorus_phase + c as f32 * PI * 0.5).sin();
                let d = self.chorus[c].read(self.sr * (0.02 + p.chorus_depth * 0.008 * lfo));
                self.chorus[c].write(x);
                x = x * (1.0 - 0.5 * p.chorus_mix) + d * 0.5 * p.chorus_mix;
            }
            if p.flanger_on {
                let lfo = 0.5 + 0.5 * (self.flanger_phase + c as f32 * PI * 0.5).sin();
                let d = self.flanger[c].read(self.sr * (0.0005 + p.flanger_depth * 0.0045 * lfo));
                self.flanger[c].write(x + d * p.flanger_feedback);
                x = x * (1.0 - 0.5 * p.flanger_mix) + d * 0.5 * p.flanger_mix;
            }
            if p.echo_on {
                let line = &mut self.echo[c];
                let d = line.read(self.sr * p.echo_ms / 1000.0);
                line.write(x + d * p.echo_feedback);
                x += d * p.echo_mix;
            }
            *s = x;
        }
        if p.reverb_on {
            if let Some(rv) = self.reverb.as_mut() {
                let input = f.iter().sum::<f32>() / f.len() as f32 * 0.015;
                let fb = p.reverb_room * 0.28 + 0.7;
                let damp = p.reverb_damp * 0.4;
                let wet = p.reverb_mix * 2.0;
                let dry = 1.0 - p.reverb_mix * 0.5;
                for (c, s) in f.iter_mut().enumerate() {
                    let r = rv.run(input, c, fb, damp);
                    *s = *s * dry + r * wet;
                }
            }
        }
        self.chorus_phase = (self.chorus_phase + 2.0 * PI * p.chorus_rate / self.sr) % (2.0 * PI);
        self.flanger_phase = (self.flanger_phase + 2.0 * PI * p.flanger_rate / self.sr) % (2.0 * PI);
    }
}

/* ---------- 速度／節奏／音高 ---------- */

struct Resampler {
    ch: usize,
    buf: Vec<f32>,
    t: f64,
}

impl Resampler {
    fn new(ch: usize) -> Self {
        Self { ch, buf: vec![0.0; ch], t: 1.0 }
    }
    fn frames(&self) -> usize {
        self.buf.len() / self.ch
    }
    fn has_data(&self) -> bool {
        self.frames() > 1
    }
    fn buffered(&self) -> f64 {
        (self.frames() as f64 - self.t).max(0.0)
    }
    fn run(&mut self, input: &[f32], ratio: f64, out: &mut Vec<f32>) {
        self.buf.extend_from_slice(input);
        let ch = self.ch;
        let n = self.frames();
        while (self.t.floor() as usize) + 2 < n {
            let i = self.t.floor() as usize;
            let f = (self.t - i as f64) as f32;
            for c in 0..ch {
                let y0 = self.buf[(i - 1) * ch + c];
                let y1 = self.buf[i * ch + c];
                let y2 = self.buf[(i + 1) * ch + c];
                let y3 = self.buf[(i + 2) * ch + c];
                let c1 = 0.5 * (y2 - y0);
                let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
                let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
                out.push(((c3 * f + c2) * f + c1) * f + y1);
            }
            self.t += ratio;
        }
        let drop = (self.t.floor() as usize).saturating_sub(1).min(n.saturating_sub(1));
        if drop > 0 {
            self.buf.drain(..drop * ch);
            self.t -= drop as f64;
        }
    }
    fn drain_raw(&mut self, out: &mut Vec<f32>) {
        let start = (self.t.floor() as usize).min(self.frames());
        out.extend_from_slice(&self.buf[start * self.ch..]);
        let last = self.buf.len().saturating_sub(self.ch);
        let hist = self.buf[last..].to_vec();
        self.buf = if hist.len() == self.ch { hist } else { vec![0.0; self.ch] };
        self.t = 1.0;
    }
}

struct Wsola {
    ch: usize,
    seq: usize,
    ovl: usize,
    seek: usize,
    input: Vec<f32>,
    tail: Vec<f32>,
    have_tail: bool,
    skip_fract: f64,
}

impl Wsola {
    fn new(ch: usize, sr: u32) -> Self {
        let sr = sr.max(1) as f64;
        Self {
            ch,
            seq: (sr * 0.060) as usize,
            ovl: (sr * 0.010) as usize,
            seek: (sr * 0.020) as usize,
            input: Vec::new(),
            tail: Vec::new(),
            have_tail: false,
            skip_fract: 0.0,
        }
    }
    fn has_data(&self) -> bool {
        !self.input.is_empty() || self.have_tail
    }
    fn buffered(&self) -> f64 {
        (self.input.len() / self.ch) as f64
    }
    fn mono(&self, frame: usize) -> f32 {
        let s = &self.input[frame * self.ch..frame * self.ch + self.ch];
        s.iter().sum::<f32>()
    }
    fn best_offset(&self) -> usize {
        let ch = self.ch;
        let tail: Vec<f32> = (0..self.ovl).map(|k| self.tail[k * ch..k * ch + ch].iter().sum()).collect();
        let m: Vec<f32> = (0..self.seek + self.ovl).map(|i| self.mono(i)).collect();
        let mut norm: f32 = m[..self.ovl].iter().map(|x| x * x).sum();
        let mut best = (0usize, f32::MIN);
        for o in 0..self.seek {
            let corr: f32 = tail.iter().zip(&m[o..o + self.ovl]).map(|(a, b)| a * b).sum();
            let score = corr / norm.max(1e-9).sqrt();
            // 輕微偏好靠近中央的位置，避免來回跳動。
            let bias = 1.0 - 0.25 * ((o as f32 / self.seek as f32) - 0.5).abs();
            let score = if score > 0.0 { score * bias } else { score / bias };
            if score > best.1 {
                best = (o, score);
            }
            norm += m[o + self.ovl] * m[o + self.ovl] - m[o] * m[o];
        }
        best.0
    }
    fn run(&mut self, input: &[f32], tempo: f64, out: &mut Vec<f32>) {
        self.input.extend_from_slice(input);
        let ch = self.ch;
        loop {
            let frames = self.input.len() / ch;
            let nominal = tempo * (self.seq - self.ovl) as f64;
            let need = (nominal.ceil() as usize + self.ovl).max(self.seq) + self.seek;
            if frames < need {
                break;
            }
            let o = if self.have_tail { self.best_offset() } else { 0 };
            if self.have_tail {
                for k in 0..self.ovl {
                    let w = k as f32 / self.ovl as f32;
                    for c in 0..ch {
                        out.push(self.tail[k * ch + c] * (1.0 - w) + self.input[(o + k) * ch + c] * w);
                    }
                }
            } else {
                out.extend_from_slice(&self.input[o * ch..(o + self.ovl) * ch]);
            }
            out.extend_from_slice(&self.input[(o + self.ovl) * ch..(o + self.seq - self.ovl) * ch]);
            self.tail = self.input[(o + self.seq - self.ovl) * ch..(o + self.seq) * ch].to_vec();
            self.have_tail = true;
            self.skip_fract += nominal;
            let s = self.skip_fract.floor() as usize;
            self.skip_fract -= s as f64;
            self.input.drain(..s * ch);
        }
    }
    fn drain_raw(&mut self, out: &mut Vec<f32>) {
        if self.have_tail {
            out.extend_from_slice(&self.tail);
        }
        out.append(&mut self.input);
        self.tail.clear();
        self.have_tail = false;
        self.skip_fract = 0.0;
    }
}

pub(crate) struct RateStage {
    wsola: Wsola,
    rs: Resampler,
    mid: Vec<f32>,
}

impl RateStage {
    pub(crate) fn new(ch: usize, sr: u32) -> Self {
        Self { wsola: Wsola::new(ch, sr), rs: Resampler::new(ch), mid: Vec::new() }
    }
    pub(crate) fn run(&mut self, input: &[f32], p: &FxParams, flush: bool, out: &mut Vec<f32>) {
        let (r, sw) = p.rates();
        let mut mid = std::mem::take(&mut self.mid);
        mid.clear();
        if (sw - 1.0).abs() > 1e-3 {
            self.wsola.run(input, sw, &mut mid);
            if flush {
                self.wsola.drain_raw(&mut mid);
            }
        } else {
            if self.wsola.has_data() {
                self.wsola.drain_raw(&mut mid);
            }
            mid.extend_from_slice(input);
        }
        if (r - 1.0).abs() > 1e-4 {
            self.rs.run(&mid, r, out);
            if flush {
                self.rs.drain_raw(out);
            }
        } else {
            if self.rs.has_data() {
                self.rs.drain_raw(out);
            }
            out.extend_from_slice(&mid);
        }
        self.mid = mid;
    }
    fn buffered(&self, p: &FxParams) -> f64 {
        let (_, sw) = p.rates();
        self.wsola.buffered() + self.rs.buffered() * sw
    }
}

/* ---------- 音源 ---------- */

pub(crate) const BLOCK: usize = 1024;

/// 解碼後的音訊經過：速度/節奏/音高 → 正規化 + 前級 + 18 段等化器 → 響度補償 → 音效 → 限幅。
pub struct DspSource<S: Iterator<Item = f32>> {
    inner: S,
    channels: u16,
    sample_rate: u32,
    shared: Arc<EqShared>,
    seen: u64,
    p: FxParams,
    eq: EqProcessor,
    fx: FxChain,
    rate: Option<RateStage>,
    block: Vec<f32>,
    out: Vec<f32>,
    pos: usize,
    ended: bool,
    consumed: u64,
    played_ms: Arc<AtomicU64>,
    total: Option<Duration>,
    /// 響度補償：低頻擱架濾波器（每聲道一組）
    loudness_bass: Vec<Bq>,
    /// 響度補償：高頻擱架濾波器（每聲道一組）
    loudness_treble: Vec<Bq>,
    /// 上次套用響度補償時的 EqShared 版本號
    loudness_seen: u64,
}

impl<S: Iterator<Item = f32>> DspSource<S> {
    /// `allow_rate` 為 false 時（電台）不做速度/節奏/音高：即時串流不能比網路快或慢。
    pub fn new(
        inner: S,
        channels: u16,
        sample_rate: u32,
        shared: &Arc<EqShared>,
        norm_gain: f32,
        total: Option<Duration>,
        allow_rate: bool,
    ) -> (Self, Arc<AtomicU64>) {
        let ch = channels.max(1) as usize;
        let played = Arc::new(AtomicU64::new(0));
        let mut s = Self {
            inner,
            channels,
            sample_rate,
            shared: shared.clone(),
            seen: 0,
            p: FxParams::default(),
            eq: EqProcessor::new(shared.clone(), sample_rate, channels, norm_gain),
            fx: FxChain::new(sample_rate, ch),
            rate: allow_rate.then(|| RateStage::new(ch, sample_rate)),
            block: Vec::with_capacity(BLOCK * ch),
            out: Vec::with_capacity(BLOCK * ch * 2),
            pos: 0,
            ended: false,
            consumed: 0,
            played_ms: played.clone(),
            total,
            loudness_bass: vec![Bq::default(); ch],
            loudness_treble: vec![Bq::default(); ch],
            loudness_seen: 0,
        };
        s.refresh();
        (s, played)
    }

    fn refresh(&mut self) {
        let v = self.shared.version();
        if v != self.seen {
            self.seen = v;
            let params = self.shared.get();
            self.p = params.fx;
            self.fx.set(&self.p);

            // 響度補償係數更新
            if params.loudness_enabled {
                let sr = self.sample_rate.max(1) as f32;
                for (i, b) in self.loudness_bass.iter_mut().enumerate() {
                    b.retune(Bq::lowshelf(sr, params.loudness_bass_hz, params.loudness_bass_db));
                }
                for (i, b) in self.loudness_treble.iter_mut().enumerate() {
                    b.retune(Bq::highshelf(sr, params.loudness_treble_hz, params.loudness_treble_db));
                }
            }
            self.loudness_seen = v;
        }
    }

    fn fill(&mut self) -> bool {
        self.refresh();
        let ch = self.channels.max(1) as usize;
        self.out.clear();
        self.pos = 0;
        loop {
            self.block.clear();
            while !self.ended && self.block.len() < BLOCK * ch {
                match self.inner.next() {
                    Some(s) => self.block.push(s),
                    None => self.ended = true,
                }
            }
            let frames = self.block.len() / ch;
            self.block.truncate(frames * ch);
            self.consumed += frames as u64;
            match self.rate.as_mut() {
                Some(r) => r.run(&self.block, &self.p, self.ended, &mut self.out),
                None => self.out.extend_from_slice(&self.block),
            }
            if !self.out.is_empty() || self.ended {
                break;
            }
        }
        let params = self.shared.get();
        let loudness_on = params.loudness_enabled;
        let fx_on = self.fx.active();
        for frame in self.out.chunks_mut(ch) {
            for s in frame.iter_mut() {
                *s = self.eq.process(*s);
            }
            // 響度補償：在 EQ 之後、音效之前套用
            if loudness_on {
                for c in 0..ch {
                    frame[c] = self.loudness_bass[c].run(frame[c]);
                    frame[c] = self.loudness_treble[c].run(frame[c]);
                }
            }
            if fx_on {
                self.fx.frame(frame);
            }
            for s in frame.iter_mut() {
                *s = if s.is_finite() { s.clamp(-1.0, 1.0) } else { 0.0 };
            }
        }
        let buffered = self.rate.as_ref().map(|r| r.buffered(&self.p)).unwrap_or(0.0);
        let frames = (self.consumed as f64 - buffered).max(0.0);
        self.played_ms.store((frames * 1000.0 / self.sample_rate.max(1) as f64) as u64, Ordering::Relaxed);
        !self.out.is_empty()
    }
}

impl<S: Iterator<Item = f32>> Iterator for DspSource<S> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.channels == 0 {
            return None;
        }
        if self.pos >= self.out.len() && !self.fill() {
            return None;
        }
        let s = self.out[self.pos];
        self.pos += 1;
        Some(s)
    }
}

impl<S: Iterator<Item = f32> + Send> Source for DspSource<S> {
    fn current_frame_len(&self) -> Option<usize> { None }
    fn channels(&self) -> u16 { self.channels }
    fn sample_rate(&self) -> u32 { self.sample_rate }
    fn total_duration(&self) -> Option<Duration> { self.total }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 44100;

    fn tone(freq: f32, secs: f32, ch: usize) -> Vec<f32> {
        let n = (SR as f32 * secs) as usize;
        (0..n).flat_map(|i| {
            let s = (2.0 * PI * freq * i as f32 / SR as f32).sin() * 0.3;
            std::iter::repeat(s).take(ch)
        }).collect()
    }

    fn run(input: Vec<f32>, ch: u16, fx: FxParams) -> (Vec<f32>, u64) {
        let shared = Arc::new(EqShared::new());
        shared.update(|p| p.fx = fx.sanitized());
        let (src, played) = DspSource::new(input.into_iter(), ch, SR, &shared, 1.0, None, true);
        let out: Vec<f32> = src.collect();
        (out, played.load(Ordering::Relaxed))
    }

    /// 以上升零交越數估頻率（只看第一聲道）。
    fn freq(x: &[f32], ch: usize) -> f32 {
        let mono: Vec<f32> = x.chunks(ch).map(|f| f[0]).collect();
        let skip = mono.len() / 10;
        let seg = &mono[skip..mono.len() - skip];
        let z = seg.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        z as f32 / (seg.len() as f32 / SR as f32)
    }

    #[test]
    fn all_off_is_passthrough() {
        let input = tone(440.0, 0.5, 2);
        let (out, played) = run(input.clone(), 2, FxParams::default());
        assert_eq!(out.len(), input.len());
        assert!(out.iter().zip(&input).all(|(a, b)| (a - b).abs() < 1e-6));
        assert_eq!(played, 500);
    }

    #[test]
    fn tempo_changes_length_not_pitch() {
        let input = tone(440.0, 3.0, 2);
        let (out, played) = run(input.clone(), 2, FxParams { tempo: 0.8, ..Default::default() });
        let ratio = out.len() as f32 / input.len() as f32;
        assert!((ratio - 1.25).abs() < 0.03, "length ratio {ratio}");
        let f = freq(&out, 2);
        assert!((f - 440.0).abs() < 8.0, "pitch kept, got {f}");
        assert!((played as i64 - 3000).abs() < 30, "position follows song time, got {played}");
    }

    #[test]
    fn pitch_changes_pitch_not_length() {
        let input = tone(440.0, 3.0, 1);
        let (out, _) = run(input.clone(), 1, FxParams { pitch: 12.0, ..Default::default() });
        let ratio = out.len() as f32 / input.len() as f32;
        assert!((ratio - 1.0).abs() < 0.03, "length ratio {ratio}");
        let f = freq(&out, 1);
        assert!((f - 880.0).abs() < 15.0, "one octave up, got {f}");
    }

    #[test]
    fn speed_changes_both() {
        let input = tone(440.0, 2.0, 2);
        let (out, _) = run(input.clone(), 2, FxParams { speed: 1.5, ..Default::default() });
        let ratio = out.len() as f32 / input.len() as f32;
        assert!((ratio - 1.0 / 1.5).abs() < 0.02, "length ratio {ratio}");
        assert!((freq(&out, 2) - 660.0).abs() < 10.0);
    }

    #[test]
    fn voice_remover_cancels_centered_mid() {
        let input = tone(1000.0, 0.5, 2);
        let (out, _) = run(input, 2, FxParams { voice_remover: true, ..Default::default() });
        let rms = (out[4410..].iter().map(|s| s * s).sum::<f32>() / (out.len() - 4410) as f32).sqrt();
        assert!(rms < 0.01, "centered 1kHz should vanish, rms {rms}");
    }

    #[test]
    fn effects_stay_bounded_and_audible() {
        let mut input = tone(220.0, 0.3, 2);
        input.extend(std::iter::repeat(0.0).take(SR as usize * 2));
        let fx = FxParams {
            reverb_on: true, reverb_room: 1.0, reverb_mix: 1.0,
            echo_on: true, echo_feedback: 0.9, echo_mix: 1.0,
            chorus_on: true, flanger_on: true, flanger_feedback: 0.9,
            bass_on: true, bass_db: 15.0, enhancer_on: true, enhancer: 1.0,
            ..Default::default()
        };
        let (out, _) = run(input, 2, fx);
        assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        let tail = &out[SR as usize * 2..];
        let energy: f32 = tail.iter().map(|s| s * s).sum();
        assert!(energy > 1e-3, "reverb/echo tail exists after the tone ends");
    }

    #[test]
    fn live_param_change_mid_stream() {
        let shared = Arc::new(EqShared::new());
        let input = tone(440.0, 4.0, 2);
        let (mut src, _) = DspSource::new(input.clone().into_iter(), 2, SR, &shared, 1.0, None, true);
        let mut out = Vec::new();
        for _ in 0..input.len() / 2 {
            out.push(src.next().unwrap());
        }
        shared.update(|p| p.fx.tempo = 1.25);
        out.extend(src);
        let ratio = out.len() as f32 / input.len() as f32;
        assert!(ratio < 0.95 && ratio > 0.85, "second half faster, ratio {ratio}");
    }
}
