//! The sound of a live conversation: rates changed, the agent's own voice
//! taken out of what the microphone hears, and speech told from other
//! sound by a model.
//!
//! **Echo cancellation.** What the loudspeaker is given is kept on the
//! microphone's clock ([`FarLine`]): each piece placed at the count of
//! samples the microphone had given when it was handed to the player. The
//! loudspeaker's sound reaches the microphone later by the player's,
//! the device's and the room's delays, which [`Canceller`] finds by
//! correlating the two (at 4 kHz, over half a second, up to half a second
//! late), and then the room's echo of it is learned by an adaptive filter
//! (normalized least mean squares, 64 ms long) and taken away. While the
//! person speaks over it (double talk: what is left is much louder than
//! the filter leaves of the echo), the filter does not learn; while only
//! the loudspeaker sounds, what is left of the echo is quietened further.
//!
//! **Speech, by a model.** [`SpeechModel`] asks RNNoise's network (through
//! `nnnoiseless`) how likely each 10 ms is to be speech, the microphone's
//! 16 kHz raised to the 48 kHz it was trained on.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The rate the microphone is read at, and echo is cancelled at.
pub const RATE: u32 = 16_000;

/// The adaptive filter's length: 64 ms of the room's echo.
pub const ECHO_TAPS: usize = 1024;

/// How late the loudspeaker's sound may reach the microphone: half a
/// second.
pub const MOST_DELAY: usize = (RATE / 2) as usize;

/// How much of what the loudspeaker was given is kept: delay, filter and
/// the correlation's window, with room to spare.
const KEPT: usize = 3 * RATE as usize;

/// Changes a stream's rate by a ratio of small whole numbers (16 to 24 kHz,
/// 24 to 16, 16 to 48), through a windowed-sinc filter in polyphase form.
#[derive(Clone, Debug)]
pub struct Resampler {
    up: usize,
    down: usize,
    /// Each phase's taps, oldest sample last.
    phases: Vec<Vec<f32>>,
    taps: usize,
    history: Vec<f32>,
    /// Where the next output falls, in the raised rate, from `history[0]`.
    at: usize,
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

impl Resampler {
    /// From `from` samples a second to `to`.
    pub fn new(from: u32, to: u32) -> Resampler {
        let (from, to) = (from.max(1) as usize, to.max(1) as usize);
        let g = gcd(from, to);
        let (up, down) = (to / g, from / g);
        let taps = 16;
        let length = taps * up;
        // The cut: below the lower Nyquist rate of the two, in the raised
        // rate's cycles per sample, with a little room.
        let cutoff = 0.5 / up.max(down) as f64 * 0.92;
        // Centred on a whole input sample, so that the stream is late by
        // exactly half the taps.
        let middle = ((taps / 2) * up) as f64;
        let prototype: Vec<f64> = (0..length)
            .map(|n| {
                let t = n as f64 - middle;
                let sinc = if t == 0.0 {
                    2.0 * cutoff
                } else {
                    (2.0 * std::f64::consts::PI * cutoff * t).sin() / (std::f64::consts::PI * t)
                };
                let x = std::f64::consts::PI * t / middle;
                let blackman = 0.42 + 0.5 * x.cos() + 0.08 * (2.0 * x).cos();
                sinc * blackman * up as f64
            })
            .collect();
        let phases = (0..up)
            .map(|p| (0..taps).map(|k| prototype[p + k * up] as f32).collect())
            .collect();
        Resampler {
            up,
            down,
            phases,
            taps,
            history: vec![0.0; taps - 1],
            at: (taps - 1) * up,
        }
    }

    /// The samples `input` makes at the new rate, the stream's earlier
    /// samples taken into account.
    pub fn process(&mut self, input: &[i16]) -> Vec<i16> {
        self.history.extend(input.iter().map(|&s| f32::from(s)));
        let mut out = Vec::with_capacity(input.len() * self.up / self.down + 1);
        while self.at / self.up < self.history.len() {
            let newest = self.at / self.up;
            let phase = &self.phases[self.at % self.up];
            let mut sum = 0.0f32;
            for (k, tap) in phase.iter().enumerate() {
                sum += tap * self.history[newest - k];
            }
            out.push(sum.round().clamp(-32768.0, 32767.0) as i16);
            self.at += self.down;
        }
        // What no output needs again; brought down by much, the next
        // output may fall past what has come, and is waited for.
        let done = (self.at / self.up)
            .saturating_sub(self.taps - 1)
            .min(self.history.len());
        if done > 0 {
            self.history.drain(..done);
            self.at -= done * self.up;
        }
        out
    }

    /// The whole of `samples` from `from` to `to`, not late at all.
    pub fn once(from: u32, to: u32, samples: &[i16]) -> Vec<i16> {
        if from == to {
            return samples.to_vec();
        }
        let r = Resampler::new(from, to);
        let middle = (r.taps / 2) * r.up;
        let want = samples.len() * r.up / r.down;
        (0..want)
            .map(|n| {
                let u = n * r.down + middle;
                let (newest, phase) = (u / r.up, &r.phases[u % r.up]);
                let mut sum = 0.0f32;
                for (k, tap) in phase.iter().enumerate() {
                    if let Some(&x) = newest.checked_sub(k).and_then(|i| samples.get(i)) {
                        sum += tap * f32::from(x);
                    }
                }
                sum.round().clamp(-32768.0, 32767.0) as i16
            })
            .collect()
    }
}

/// Raw 16-bit little-endian samples as numbers.
pub fn samples_of(bytes: &[u8]) -> Vec<i16> {
    bytes
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
        .collect()
}

/// What the loudspeaker was given, on the microphone's clock: the count of
/// samples the microphone had given when each piece was handed to the
/// player.
#[derive(Debug, Default)]
pub struct FarLine {
    clock: AtomicU64,
    line: Mutex<Line>,
}

#[derive(Debug, Default)]
struct Line {
    /// The microphone's count the first sample kept falls at.
    start: u64,
    samples: VecDeque<f32>,
}

impl FarLine {
    /// A line with nothing on it.
    pub fn new() -> Arc<FarLine> {
        Arc::new(FarLine::default())
    }

    /// How many samples the microphone has given.
    pub fn clock(&self) -> u64 {
        self.clock.load(Ordering::SeqCst)
    }

    /// The microphone gave `n` samples more.
    pub fn heard(&self, n: usize) {
        self.clock.fetch_add(n as u64, Ordering::SeqCst);
    }

    /// `samples` were handed to the player now: after what was handed to it
    /// before, or at the microphone's count if that came first (it fell
    /// silent between).
    pub fn played(&self, samples: &[i16]) {
        let now = self.clock();
        let mut line = self.line.lock().unwrap_or_else(|e| e.into_inner());
        let end = line.start + line.samples.len() as u64;
        if end < now {
            let gap = (now - end).min(KEPT as u64) as usize;
            line.samples.extend(std::iter::repeat(0.0).take(gap));
            let end = line.start + line.samples.len() as u64;
            if end < now {
                line.start += now - end;
            }
        }
        line.samples.extend(samples.iter().map(|&s| f32::from(s)));
        let keep_from = now.saturating_sub(KEPT as u64);
        while line.start < keep_from && !line.samples.is_empty() {
            line.samples.pop_front();
            line.start += 1;
        }
        if line.samples.is_empty() {
            line.start = now;
        }
    }

    /// What the line holds at the microphone's counts `from .. from + len`
    /// (silence where it holds nothing).
    fn window(&self, from: i64, len: usize, into: &mut Vec<f32>) {
        into.clear();
        into.resize(len, 0.0);
        let line = self.line.lock().unwrap_or_else(|e| e.into_inner());
        let start = line.start as i64;
        for (i, slot) in into.iter_mut().enumerate() {
            let at = from + i as i64 - start;
            if at >= 0 {
                if let Some(&v) = line.samples.get(at as usize) {
                    *slot = v;
                }
            }
        }
    }
}

/// Finds how late the loudspeaker's sound reaches the microphone: the lag
/// at which the two, at 4 kHz, correlate best.
#[derive(Debug, Default)]
struct Estimator {
    near: VecDeque<f32>,
    far: VecDeque<f32>,
    /// The last lag found, at 4 kHz, and how many times running.
    candidate: Option<(usize, u32)>,
}

/// The correlation's window, at 4 kHz: half a second.
const WINDOW: usize = 2_000;

/// The lags tried, at 4 kHz.
const LAGS: usize = MOST_DELAY / 4;

impl Estimator {
    /// Takes a frame of the microphone's sound and what the loudspeaker
    /// was given at the same counts.
    fn take(&mut self, near: &[f32], far: &[f32]) {
        for (n, f) in near.chunks(4).zip(far.chunks(4)) {
            self.near.push_back(n.iter().sum::<f32>() / n.len() as f32);
            self.far.push_back(f.iter().sum::<f32>() / f.len() as f32);
        }
        while self.near.len() > WINDOW {
            self.near.pop_front();
        }
        while self.far.len() > WINDOW + LAGS {
            self.far.pop_front();
        }
    }

    /// The delay, in samples at 16 kHz, when the correlation names one
    /// clearly enough, twice running (or once, very clearly).
    fn estimate(&mut self) -> Option<usize> {
        if self.near.len() < WINDOW || self.far.len() < WINDOW + LAGS {
            return None;
        }
        let near: Vec<f32> = self.near.iter().copied().collect();
        let far: Vec<f32> = self.far.iter().copied().collect();
        let near_energy: f32 = near.iter().map(|v| v * v).sum();
        if near_energy <= f32::EPSILON {
            return None;
        }
        // far[LAGS + t] is at the same count as near[t]; at lag l, near[t]
        // is matched with far[LAGS + t - l].
        let mut best = (0usize, 0.0f32);
        let mut far_energy: f32 = far[LAGS..LAGS + WINDOW].iter().map(|v| v * v).sum();
        for lag in 0..LAGS {
            let base = LAGS - lag;
            if lag > 0 {
                // Slide the far window one sample earlier.
                let entering = far[base];
                let leaving = far[base + WINDOW];
                far_energy += entering * entering - leaving * leaving;
            }
            if far_energy <= f32::EPSILON {
                continue;
            }
            let mut dot = 0.0f32;
            for (n, f) in near.iter().zip(&far[base..base + WINDOW]) {
                dot += n * f;
            }
            let score = dot.abs() / (near_energy * far_energy.max(0.0)).sqrt();
            if score > best.1 {
                best = (lag, score);
            }
        }
        let (lag, score) = best;
        if score < 0.2 {
            self.candidate = None;
            return None;
        }
        let runs = match self.candidate {
            Some((previous, runs)) if previous.abs_diff(lag) <= 2 => runs + 1,
            _ => 1,
        };
        self.candidate = Some((lag, runs));
        (runs >= 2 || score >= 0.5).then_some(lag * 4)
    }
}

/// Takes the loudspeaker's echo out of what the microphone hears, a frame
/// at a time.
#[derive(Debug)]
pub struct Canceller {
    far: Arc<FarLine>,
    /// The filter, newest sample last.
    weights: Vec<f32>,
    /// The microphone's count of the next frame's first sample.
    position: u64,
    /// How late the loudspeaker's sound arrives, once known.
    delay: Option<usize>,
    estimator: Estimator,
    frames: u64,
    /// How much quieter the echo is after the filter, in energy, learned
    /// while only the loudspeaker sounds.
    reduction: f32,
    /// Frames of double talk running, and the gain applied last.
    double_talk: u32,
    gain: f32,
    window: Vec<f32>,
}

/// Samples taken before the delay found, in case it was found a little
/// late.
const AHEAD: usize = 64;

/// The quietest a frame of the loudspeaker's sound can be, as a root mean
/// square, and still teach the filter.
const FAR_FLOOR: f32 = 30.0;

impl Canceller {
    /// A canceller of the echo of what `far` was given.
    pub fn new(far: Arc<FarLine>) -> Canceller {
        Canceller {
            far,
            weights: vec![0.0; ECHO_TAPS],
            position: 0,
            delay: None,
            estimator: Estimator::default(),
            frames: 0,
            reduction: 1.0,
            double_talk: 0,
            gain: 1.0,
            window: Vec::new(),
        }
    }

    /// How late the loudspeaker's sound reaches the microphone, in
    /// samples, once found.
    pub fn delay(&self) -> Option<usize> {
        self.delay
    }

    /// How many times quieter the echo is after the filter (energy), as
    /// learned so far.
    pub fn reduction(&self) -> f32 {
        self.reduction
    }

    /// Takes the microphone's next frame and returns it without the
    /// loudspeaker's echo.
    pub fn frame(&mut self, mic: &[i16]) -> Vec<i16> {
        let len = mic.len();
        if len == 0 {
            return Vec::new();
        }
        let start = self.position;
        self.position += len as u64;
        self.far.heard(len);
        let near: Vec<f32> = mic.iter().map(|&s| f32::from(s)).collect();
        let mut zero = Vec::new();
        self.far.window(start as i64, len, &mut zero);
        self.estimator.take(&near, &zero);
        self.frames += 1;
        let far_level = (zero.iter().map(|v| v * v).sum::<f32>() / len.max(1) as f32).sqrt();
        if self.frames % 10 == 0 && far_level > FAR_FLOOR {
            if let Some(found) = self.estimator.estimate() {
                match self.delay {
                    Some(delay) if delay.abs_diff(found) <= 8 => {}
                    Some(delay) => {
                        // The path moved: shift what was learned with it.
                        let shift = found as i64 - delay as i64;
                        let mut moved = vec![0.0; ECHO_TAPS];
                        for (i, w) in self.weights.iter().enumerate() {
                            let j = i as i64 + shift;
                            if (0..ECHO_TAPS as i64).contains(&j) {
                                moved[j as usize] = *w;
                            }
                        }
                        self.weights = moved;
                        self.delay = Some(found);
                    }
                    None => self.delay = Some(found),
                }
            }
        }
        let Some(delay) = self.delay else {
            return mic.to_vec();
        };
        // The loudspeaker's sound that can be in this frame: from a filter's
        // length before it, the delay taken away.
        let from = start as i64 - delay as i64 + AHEAD as i64 - ECHO_TAPS as i64 + 1;
        let mut window = std::mem::take(&mut self.window);
        self.far.window(from, len + ECHO_TAPS - 1, &mut window);
        let aligned_level =
            (window[ECHO_TAPS - 1..].iter().map(|v| v * v).sum::<f32>() / len.max(1) as f32).sqrt();
        let any_far = window.iter().any(|v| *v != 0.0);
        if !any_far {
            self.window = window;
            self.gain = 1.0;
            self.double_talk = 0;
            return mic.to_vec();
        }
        // What the filter leaves, before it learns from this frame.
        let mut echo_energy = 0.0f32;
        let mut left_energy = 0.0f32;
        let near_energy: f32 = near.iter().map(|v| v * v).sum();
        for (i, d) in near.iter().enumerate() {
            let x = &window[i..i + ECHO_TAPS];
            let y: f32 = self.weights.iter().zip(x).map(|(w, x)| w * x).sum();
            echo_energy += y * y;
            left_energy += (d - y) * (d - y);
        }
        let learned = self.reduction >= 8.0;
        let talking = learned
            && left_energy > near_energy.min(4.0 * echo_energy / self.reduction) * 4.0
            && left_energy > 0.25 * near_energy;
        if talking {
            self.double_talk += 1;
            // Talk this long while the loudspeaker sounds is more likely a
            // changed room: learn again.
            if self.double_talk > 100 {
                self.reduction = 1.0;
                self.double_talk = 0;
            }
        } else {
            self.double_talk = 0;
        }
        let teach = !talking && aligned_level > FAR_FLOOR;
        let mut out = Vec::with_capacity(len);
        let mut x_energy: f32 = window[..ECHO_TAPS].iter().map(|v| v * v).sum();
        let mut after = 0.0f32;
        for (i, d) in near.iter().enumerate() {
            if i > 0 {
                let entering = window[i + ECHO_TAPS - 1];
                let leaving = window[i - 1];
                x_energy = (x_energy + entering * entering - leaving * leaving).max(0.0);
            }
            let x = &window[i..i + ECHO_TAPS];
            let y: f32 = self.weights.iter().zip(x).map(|(w, x)| w * x).sum();
            let e = d - y;
            after += e * e;
            if teach {
                let step = 0.5 * e / (x_energy + ECHO_TAPS as f32 * 100.0);
                for (w, x) in self.weights.iter_mut().zip(x) {
                    *w += step * x;
                }
            }
            out.push(e);
        }
        if teach && after > 0.0 {
            let now = (near_energy / after).clamp(1.0, 1e5);
            self.reduction += (now - self.reduction) * 0.1;
        }
        // What is left of the echo, quietened while only the loudspeaker
        // sounds; the person's voice let through while they speak over it.
        let target = if talking || aligned_level <= FAR_FLOOR {
            1.0
        } else {
            0.25
        };
        let mut gain = self.gain;
        let step = (target - gain) / len.max(1) as f32;
        let out = out
            .into_iter()
            .map(|e| {
                gain += step;
                (e * gain).round().clamp(-32768.0, 32767.0) as i16
            })
            .collect();
        self.gain = target;
        self.window = window;
        out
    }
}

/// How likely each frame is to be speech, by RNNoise's network.
pub struct SpeechModel {
    state: Box<nnnoiseless::DenoiseState<'static>>,
    up: Resampler,
    pending: Vec<f32>,
    scratch: Vec<f32>,
}

impl std::fmt::Debug for SpeechModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpeechModel").finish_non_exhaustive()
    }
}

impl Default for SpeechModel {
    fn default() -> SpeechModel {
        SpeechModel::new()
    }
}

impl SpeechModel {
    /// The model, for sound at [`RATE`].
    pub fn new() -> SpeechModel {
        SpeechModel {
            state: nnnoiseless::DenoiseState::new(),
            up: Resampler::new(RATE, 48_000),
            pending: Vec::new(),
            scratch: vec![0.0; nnnoiseless::DenoiseState::FRAME_SIZE],
        }
    }

    /// How likely `frame` (at [`RATE`]) is to be speech, from 0 to 1: the
    /// most likely of its 10 ms pieces.
    pub fn probability(&mut self, frame: &[i16]) -> f32 {
        let raised = self.up.process(frame);
        self.pending.extend(raised.iter().map(|&s| f32::from(s)));
        let size = nnnoiseless::DenoiseState::FRAME_SIZE;
        let mut most = 0.0f32;
        while self.pending.len() >= size {
            let piece: Vec<f32> = self.pending.drain(..size).collect();
            let p = self.state.process_frame(&mut self.scratch, &piece);
            most = most.max(p);
        }
        most
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sound like speech, made up: bursts of noise shaped by a moving
    /// resonance, with pauses, deterministic for `seed`.
    fn babble(seed: u64, seconds: f32, rate: u32) -> Vec<i16> {
        let mut state = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let mut random = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((state >> 33) as f32 / (1u64 << 31) as f32) * 2.0 - 1.0
        };
        let n = (seconds * rate as f32) as usize;
        let (mut y1, mut y2) = (0.0f32, 0.0f32);
        (0..n)
            .map(|i| {
                let t = i as f32 / rate as f32;
                // Syllables of 200 ms, every other one silent now and then.
                let syllable = (t / 0.2) as u64;
                let on = (syllable.wrapping_mul(2654435761) >> 7) % 5 != 0;
                let envelope = if on {
                    (std::f32::consts::PI * (t / 0.2).fract()).sin()
                } else {
                    0.0
                };
                let f = 400.0 + 900.0 * (0.5 + 0.5 * (t * 3.1 + seed as f32).sin());
                let r = 0.97f32;
                let c = 2.0 * r * (2.0 * std::f32::consts::PI * f / rate as f32).cos();
                let y = random() + c * y1 - r * r * y2;
                y2 = y1;
                y1 = y;
                (y * envelope * 900.0).clamp(-32000.0, 32000.0) as i16
            })
            .collect()
    }

    fn energy(samples: &[i16]) -> f64 {
        samples
            .iter()
            .map(|&s| f64::from(s) * f64::from(s))
            .sum::<f64>()
            / samples.len().max(1) as f64
    }

    #[test]
    fn rates_change_and_a_tone_stays_a_tone() {
        let tone: Vec<i16> = (0..16_000)
            .map(|i| {
                ((2.0 * std::f64::consts::PI * 440.0 * i as f64 / 16_000.0).sin() * 10_000.0) as i16
            })
            .collect();
        let up = Resampler::once(16_000, 24_000, &tone);
        assert_eq!(up.len(), 24_000);
        let expected: Vec<i16> = (0..24_000)
            .map(|i| {
                ((2.0 * std::f64::consts::PI * 440.0 * i as f64 / 24_000.0).sin() * 10_000.0) as i16
            })
            .collect();
        let error: f64 = up[100..23_900]
            .iter()
            .zip(&expected[100..23_900])
            .map(|(a, b)| (f64::from(*a) - f64::from(*b)).powi(2))
            .sum::<f64>()
            / 23_800.0;
        assert!(error.sqrt() < 200.0, "{}", error.sqrt());
        let back = Resampler::once(24_000, 16_000, &up);
        assert_eq!(back.len(), 16_000);
        // In pieces, as a stream: the same as at once.
        let mut stream = Resampler::new(16_000, 48_000);
        let pieces: Vec<i16> = tone.chunks(320).flat_map(|c| stream.process(c)).collect();
        let mut whole = Resampler::new(16_000, 48_000);
        assert_eq!(pieces, whole.process(&tone));
    }

    /// The loudspeaker's sound reaching the microphone `delay` samples late
    /// through a small room, with the person's voice added where `near`
    /// says.
    fn room(far: &[i16], delay: usize, near: &[i16]) -> Vec<i16> {
        let taps: Vec<f32> = (0..200)
            .map(|k| {
                let decay = (-(k as f32) / 40.0).exp();
                let sign = if (k * 7919) % 3 == 0 { -1.0 } else { 1.0 };
                if k == 0 {
                    0.6
                } else {
                    0.15 * decay * sign
                }
            })
            .collect();
        (0..far.len())
            .map(|n| {
                let mut echo = 0.0f32;
                for (k, tap) in taps.iter().enumerate() {
                    if n >= delay + k {
                        echo += tap * f32::from(far[n - delay - k]);
                    }
                }
                let voice = near.get(n).copied().map(f32::from).unwrap_or(0.0);
                (echo + voice).clamp(-32768.0, 32767.0) as i16
            })
            .collect()
    }

    #[test]
    fn the_loudspeakers_echo_is_found_and_taken_away() {
        let rate = RATE;
        let far = babble(7, 8.0, rate);
        let delay = 1_900; // about 119 ms
                           // The person speaks over it from the sixth second on.
        let mut near = vec![0i16; far.len()];
        let voice = babble(99, 2.0, rate);
        near[6 * rate as usize..].copy_from_slice(&voice[..far.len() - 6 * rate as usize]);
        let mic = room(&far, delay, &near);

        let line = FarLine::new();
        let mut canceller = Canceller::new(line.clone());
        let mut out = Vec::new();
        for (n, frame) in mic.chunks(320).enumerate() {
            // The player is handed each piece as the microphone reaches it.
            line.played(&far[n * 320..n * 320 + frame.len()]);
            out.extend(canceller.frame(frame));
        }
        let found = canceller.delay().expect("a delay found");
        assert!(found.abs_diff(delay) <= 8, "found {found}, not {delay}");

        // Only the loudspeaker, once learned (the fourth and fifth seconds):
        // the echo at least 20 dB quieter.
        let span = 4 * rate as usize..6 * rate as usize;
        let before = energy(&mic[span.clone()]);
        let after = energy(&out[span]);
        let db = 10.0 * (before / after.max(1.0)).log10();
        assert!(db >= 20.0, "the echo is only {db:.1} dB quieter");

        // The person over it: their voice let through, within 3 dB.
        let span = 6 * rate as usize + 3_200..8 * rate as usize;
        let voice_energy = energy(&near[span.clone()]);
        let kept = energy(&out[span]);
        let db = 10.0 * (kept / voice_energy).log10();
        assert!(db.abs() <= 3.0, "the person's voice is {db:.1} dB off");
    }

    #[test]
    fn the_model_hears_speech_and_not_noise() {
        // "Can you hear me now?", said by espeak-ng, at 16 kHz.
        let said = samples_of(include_bytes!("../tests/fixtures/spoken-16k.raw"));
        let mut model = SpeechModel::new();
        let speech: Vec<f32> = said
            .chunks_exact(320)
            .map(|f| model.probability(f))
            .collect();
        let heard = speech.iter().filter(|p| **p >= 0.5).count();
        let mut state = 12345u64;
        let noise: Vec<i16> = (0..RATE as usize * 2)
            .map(|_| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((state >> 40) as i32 % 2000 - 1000) as i16
            })
            .collect();
        let mut model = SpeechModel::new();
        // The network settles in the first quarter of a second.
        let other: Vec<f32> = noise
            .chunks_exact(320)
            .map(|f| model.probability(f))
            .skip(12)
            .collect();
        let misheard = other.iter().filter(|p| **p >= 0.5).count();
        assert!(
            heard * 3 >= speech.len(),
            "speech in {heard} of {} frames",
            speech.len()
        );
        assert!(
            misheard * 20 <= other.len(),
            "speech in {misheard} of {} frames of noise",
            other.len()
        );
    }

    #[test]
    fn odd_frames_and_steep_rates_do_not_break_anything() {
        let line = FarLine::new();
        let mut canceller = Canceller::new(line.clone());
        line.played(&babble(1, 1.0, RATE));
        canceller.delay = Some(100);
        assert!(canceller.frame(&[]).is_empty());
        assert_eq!(canceller.frame(&[5; 7]).len(), 7);
        let mut steep = Resampler::new(16_000, 500);
        let out: usize = (0..20).map(|_| steep.process(&[100; 320]).len()).sum();
        assert!((195..=205).contains(&out), "{out}");
    }

    #[test]
    fn without_the_loudspeaker_the_microphone_is_untouched() {
        let line = FarLine::new();
        let mut canceller = Canceller::new(line);
        let voice = babble(3, 1.0, RATE);
        let out: Vec<i16> = voice.chunks(320).flat_map(|f| canceller.frame(f)).collect();
        assert_eq!(out, voice);
    }
}
