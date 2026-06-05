//! Procedural synthesis of the ambient sound beds.
//!
//! Everything here is generated from noise and oscillators into raw mono `f32`
//! PCM at [`SAMPLE_RATE`] — no asset files. Each generator returns a buffer that
//! loops seamlessly so it can be fed to `rodio` with `repeat_infinite()` without
//! an audible click at the wrap.
//!
//! The generators are deterministic (fixed RNG seeds) so a given build always
//! produces the same beds, which keeps behaviour reproducible across runs.

use std::f32::consts::TAU;

use rand::{rngs::StdRng, Rng, SeedableRng};

pub const SAMPLE_RATE: u32 = 44_100;

/// A gentle surf bed: low rumble plus broadband foam hiss, modulated by slow
/// wave swells. Loops seamlessly over `seconds`.
pub fn sea_bed(seconds: f32) -> Vec<f32> {
    let sr = SAMPLE_RATE as f32;
    let n = (seconds * sr) as usize;
    // Extra tail used to crossfade the end back into the start (see `seamless`).
    let fade = (0.5 * sr) as usize;

    let mut rng = StdRng::seed_from_u64(0x05EA_05EA);

    // Brown noise (integrated white, with a small leak to stop DC drift) gives
    // the low rumble; a one-pole low-passed white stream adds the foam hiss.
    let mut brown = 0.0f32;
    let mut hiss = 0.0f32;
    let mut raw = Vec::with_capacity(n + fade);
    for _ in 0..(n + fade) {
        let white: f32 = rng.random_range(-1.0..1.0);
        brown = (brown + 0.02 * white) * 0.998;
        // One-pole low-pass on a fresh white sample → softened "shhh".
        let white2: f32 = rng.random_range(-1.0..1.0);
        hiss = hiss * 0.85 + white2 * 0.15;
        raw.push(brown * 6.0 + hiss * 0.5);
    }

    let mut buf = seamless(raw, n, fade);

    // Wave swells: a few LFOs whose periods divide the loop exactly (k cycles
    // over the whole buffer) so the modulation itself is seamless.
    for (i, s) in buf.iter_mut().enumerate() {
        let t = i as f32 / n as f32; // 0..1 across the loop
        let swell = 0.55
            + 0.30 * (TAU * 2.0 * t).sin()
            + 0.10 * (TAU * 3.0 * t).sin()
            + 0.05 * (TAU * 5.0 * t).sin();
        *s *= swell.clamp(0.0, 1.2);
    }

    normalize(&mut buf, 0.5);
    buf
}

/// A sparse dawn-chorus bed: randomized chirps scattered over `seconds`, each a
/// short frequency-swept whistle with a quick envelope. All chirps finish well
/// before the end so the loop boundary is silence and joins cleanly.
pub fn bird_bed(seconds: f32) -> Vec<f32> {
    let sr = SAMPLE_RATE as f32;
    let n = (seconds * sr) as usize;
    let mut buf = vec![0.0f32; n];

    let mut rng = StdRng::seed_from_u64(0x00B1_2D50);

    // Leave a margin at the end so a chirp (plus its echo tail) never crosses
    // the wrap point.
    let margin = 1.5;
    let mut t = 0.3;
    while t < seconds - margin {
        let base = rng.random_range(1800.0..4200.0);
        let syllables = rng.random_range(1..=4);
        let mut at = t;
        for _ in 0..syllables {
            let dur = rng.random_range(0.04..0.13);
            let sweep = rng.random_range(-1.0..1.0) * rng.random_range(200.0..1400.0);
            let amp = rng.random_range(0.25..0.5);
            render_chirp(&mut buf, sr, at, dur, base, base + sweep, amp);
            at += dur + rng.random_range(0.01..0.06);
        }
        // Gap until the next bird call. Kept fairly long so the chorus stays
        // sparse — a few birds rather than a constant chatter.
        t += rng.random_range(1.2..3.5);
    }

    // Light echo for a sense of open air. The decaying taps stay within the
    // end margin because the last chirp finishes before `seconds - margin`.
    let delay = (0.11 * sr) as usize;
    for i in delay..n {
        buf[i] += buf[i - delay] * 0.18;
    }

    normalize(&mut buf, 0.45);
    buf
}

/// A deep underwater bed: a sub-bass drone (rumble + a near-infrasonic sine)
/// overlaid with occasional diver-style regulator exhales — a cluster of
/// bubbles roughly once per loop. Use a long `seconds` (≈30) so the exhales feel
/// minutes-apart rather than constant. Loops seamlessly.
pub fn underwater_bed(seconds: f32) -> Vec<f32> {
    let sr = SAMPLE_RATE as f32;
    let n = (seconds * sr) as usize;
    let fade = (0.5 * sr) as usize;

    let mut rng = StdRng::seed_from_u64(0x0DEE_0DEE);

    // Rumble: brown noise through a very heavy low-pass — only the lowest
    // frequencies survive, a felt pressure drone.
    let mut brown = 0.0f32;
    let mut lp = 0.0f32;
    let mut raw = Vec::with_capacity(n + fade);
    for _ in 0..(n + fade) {
        let white: f32 = rng.random_range(-1.0..1.0);
        brown = (brown + 0.02 * white) * 0.998;
        lp = lp * 0.99 + brown * 0.01; // lower cutoff → deeper than the surf rumble
        raw.push(lp * 16.0);
    }
    let mut buf = seamless(raw, n, fade);

    // Sub-bass sine an octave or two below the rumble, for the "much deeper"
    // body. Snap to a whole number of cycles over the loop so it stays seamless.
    let sub_cycles = (40.0 * seconds).round();
    let sub_hz = sub_cycles / seconds;
    for (i, s) in buf.iter_mut().enumerate() {
        let t = i as f32 / n as f32;
        let tremolo = 0.8 + 0.2 * (TAU * t).sin(); // 1 cycle → seamless
        let sub = (TAU * sub_hz * (i as f32 / sr)).sin();
        *s = *s * (0.7 + 0.3 * (TAU * t).sin()) + sub * 0.6 * tremolo;
    }

    // Regulator exhales: one bubble burst per ~30s of loop, kept clear of the
    // boundary so the seam rides the (seamless) drone alone.
    let usable_lo = 4.0;
    let usable_hi = seconds - 6.0;
    if usable_hi > usable_lo {
        let bursts = ((seconds / 30.0).round() as usize).max(1);
        for b in 0..bursts {
            let span = usable_hi - usable_lo;
            let lo = usable_lo + span * (b as f32 / bursts as f32);
            let hi = usable_lo + span * ((b + 1) as f32 / bursts as f32);
            let start = rng.random_range(lo..hi.max(lo + 1e-3));
            render_bubble_burst(&mut buf, sr, start, &mut rng);
        }
    }

    normalize(&mut buf, 0.6);
    buf
}

/// Render one diver-exhale: a tight cluster of short, low, rising "bloops" over
/// a second or two, starting at time `start`.
fn render_bubble_burst(buf: &mut [f32], sr: f32, start: f32, rng: &mut StdRng) {
    let count = rng.random_range(12..24);
    let mut at = start;
    for _ in 0..count {
        let f0 = rng.random_range(60.0..160.0);
        let f1 = f0 * rng.random_range(1.8..3.5); // rising "bloop"
        let dur = rng.random_range(0.03..0.09); // short and bubbly
        let amp = rng.random_range(0.2..0.45);
        render_chirp(buf, sr, at, dur, f0, f1, amp);
        at += rng.random_range(0.04..0.18); // spacing within the exhale
    }
}

/// Add one swept-sine syllable into `buf` at time `start`, sweeping from `f0` to
/// `f1` Hz over `dur` seconds with a fast-attack / exponential-decay envelope
/// and a touch of vibrato.
fn render_chirp(buf: &mut [f32], sr: f32, start: f32, dur: f32, f0: f32, f1: f32, amp: f32) {
    let start_i = (start * sr) as usize;
    let len = (dur * sr) as usize;
    if start_i + len >= buf.len() {
        return;
    }
    let mut phase = 0.0f32;
    for k in 0..len {
        let u = k as f32 / len as f32; // 0..1 within the syllable
        let secs = u * dur; // elapsed time within the syllable
        let freq = f0 + (f1 - f0) * u;
        let vibrato = 1.0 + 0.012 * (TAU * 6.0 * secs).sin();
        phase += TAU * freq * vibrato / sr;
        // Quick attack (~6 ms), exponential decay.
        let attack = (u / 0.08).min(1.0);
        let decay = (-3.5 * u).exp();
        buf[start_i + k] += phase.sin() * amp * attack * decay;
    }
}

/// Make a noise buffer loop seamlessly. `raw` must be `n + fade` samples long:
/// the trailing `fade` samples continue the noise past the loop point and are
/// equal-power crossfaded into the first `fade` samples, so sample `n-1` joins
/// sample `0` continuously. Returns exactly `n` samples.
fn seamless(raw: Vec<f32>, n: usize, fade: usize) -> Vec<f32> {
    debug_assert!(raw.len() >= n + fade);
    let mut out = raw;
    // out[0] becomes the continuation sample raw[n], which naturally follows
    // raw[n-1], so the wrap raw[n-1] -> out[0] is continuous.
    for j in 0..fade {
        let t = j as f32 / fade as f32;
        let w_in = (0.5 * std::f32::consts::PI * t).sin(); // 0 -> 1: real body
        let w_out = (0.5 * std::f32::consts::PI * t).cos(); // 1 -> 0: continuation
        out[j] = out[j] * w_in + out[n + j] * w_out;
    }
    out.truncate(n);
    out
}

/// Scale a buffer so its peak magnitude equals `peak`.
fn normalize(buf: &mut [f32], peak: f32) {
    let max = buf.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
    if max > 1e-6 {
        let g = peak / max;
        for s in buf.iter_mut() {
            *s *= g;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_sane(buf: &[f32], peak: f32) {
        assert!(!buf.is_empty());
        assert!(buf.iter().all(|s| s.is_finite()), "samples must be finite");
        let max = buf.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        assert!(max > 0.01, "bed must be audible (peak {max})");
        assert!(
            (max - peak).abs() < 1e-3,
            "peak {max} should normalize to {peak}"
        );
    }

    #[test]
    fn sea_bed_is_sane_and_loops_seamlessly() {
        let buf = sea_bed(2.0);
        assert_eq!(buf.len(), 2 * SAMPLE_RATE as usize);
        assert_sane(&buf, 0.5);
        // The wrap (last -> first sample) must not jump, or it would click.
        let jump = (buf[0] - buf[buf.len() - 1]).abs();
        assert!(jump < 0.05, "loop seam discontinuity {jump}");
    }

    #[test]
    fn underwater_bed_is_sane_and_loops_seamlessly() {
        let buf = underwater_bed(3.0);
        assert_eq!(buf.len(), 3 * SAMPLE_RATE as usize);
        assert_sane(&buf, 0.6);
        // The seam sits on the (seamless) rumble — bubbles are kept clear of it.
        let jump = (buf[0] - buf[buf.len() - 1]).abs();
        assert!(jump < 0.05, "loop seam discontinuity {jump}");
    }

    #[test]
    fn bird_bed_is_sane_and_starts_and_ends_quiet() {
        let buf = bird_bed(4.0);
        assert_eq!(buf.len(), 4 * SAMPLE_RATE as usize);
        assert_sane(&buf, 0.45);
        // Chirps are kept away from the boundary, so the seam is near-silent.
        let edge = (0.05 * SAMPLE_RATE as f32) as usize;
        let head = buf[..edge].iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        let tail = buf[buf.len() - edge..]
            .iter()
            .fold(0.0f32, |m, &s| m.max(s.abs()));
        assert!(head < 0.05 && tail < 0.05, "seam not quiet: {head}/{tail}");
    }
}
