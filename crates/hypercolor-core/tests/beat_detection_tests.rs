//! Beat detection under the shipped `[audio]` defaults.
//!
//! The daemon builds its analyzer from the user-facing config section, so
//! these tests do the same and step it one 60 fps render frame at a time.
//! The signal is synthetic and deterministic: a four-on-the-floor kick,
//! offbeat closed hats, and a held sub-bass note, the steady state of a
//! house groove.
//!
//! The offbeat hats are not asserted on. The spectral-flux path fires on
//! bright transients whenever any bass is present, at every threshold, so
//! hat rejection needs a detector change rather than a default.

use hypercolor_core::input::audio::AudioAnalyzer;
use hypercolor_types::audio::AudioPipelineConfig;
use hypercolor_types::config::AudioConfig;

const SAMPLE_RATE: u32 = 48_000;
const FPS: u32 = 60;
const BPM: u32 = 120;
const SECONDS: u32 = 8;
/// Beats before the detector's slow energy baseline settles are ignored.
const WARMUP_SECS: f64 = 1.0;
/// A beat belongs to a hit when it lands this soon after the onset.
const HIT_WINDOW_SECS: f64 = 0.035;
/// Ceiling on beats per second that land on neither a kick nor a hat.
/// Thresholds below 1.0 fire on the held bass five to seven times a second
/// in this groove; the shipped default stays near one.
const MAX_OFF_HIT_BEATS_PER_SEC: f64 = 1.5;

struct Groove {
    samples: Vec<f32>,
    kicks: Vec<f64>,
    hats: Vec<f64>,
}

fn seconds(sample: usize) -> f64 {
    sample as f64 / f64::from(SAMPLE_RATE)
}

fn house_groove() -> Groove {
    let rate = SAMPLE_RATE as f32;
    let total = (SAMPLE_RATE * SECONDS) as usize;
    let beat = (SAMPLE_RATE * 60 / BPM) as usize;
    let mut samples = vec![0.0_f32; total];

    // Kick: a sine whose pitch drops from 150 Hz to 50 Hz as it decays.
    let kick_len = (0.4 * rate) as usize;
    let mut phase = 0.0_f32;
    let kick: Vec<f32> = (0..kick_len)
        .map(|i| {
            let t = i as f32 / rate;
            let freq = 50.0 + 100.0 * (-t / 0.03).exp();
            phase += std::f32::consts::TAU * freq / rate;
            0.5 * (-t / 0.18).exp() * phase.sin()
        })
        .collect();
    let kick_onsets: Vec<usize> = (0..total).step_by(beat).collect();
    for &onset in &kick_onsets {
        for (slot, sample) in samples[onset..].iter_mut().zip(&kick) {
            *slot += sample;
        }
    }

    // Hats: noise bursts on the offbeat eighths. A first difference tilts
    // white noise toward the top octaves, where a closed hat lives.
    let hat_len = (0.06 * rate) as usize;
    let hat_onsets: Vec<usize> = (beat / 2..total).step_by(beat).collect();
    let mut state: u32 = 0x9E37_79B9;
    let mut previous = 0.0_f32;
    for &onset in &hat_onsets {
        for i in 0..hat_len.min(total - onset) {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let noise = (state >> 8) as f32 / (1 << 23) as f32 - 1.0;
            let bright = noise - previous;
            previous = noise;
            let t = i as f32 / rate;
            samples[onset + i] += 0.1 * (-t / 0.03).exp() * bright;
        }
    }

    // Sub bass: E1 held under the whole groove, faded in over 10 ms.
    for (i, slot) in samples.iter_mut().enumerate() {
        let t = i as f32 / rate;
        *slot += 0.2 * (t / 0.01).min(1.0) * (std::f32::consts::TAU * 41.2 * t).sin();
    }

    Groove {
        samples,
        kicks: kick_onsets.into_iter().map(seconds).collect(),
        hats: hat_onsets.into_iter().map(seconds).collect(),
    }
}

/// Steps the analyzer exactly as the daemon render loop does and returns the
/// frame-end time of every detected beat after warm-up.
fn detected_beats(pipeline: &AudioPipelineConfig, samples: &[f32]) -> Vec<f64> {
    let mut analyzer = AudioAnalyzer::with_sample_rate(pipeline, SAMPLE_RATE);
    let per_frame = (SAMPLE_RATE / FPS) as usize;
    let dt = 1.0 / FPS as f32;
    let mut beats = Vec::new();
    for (frame, chunk) in samples.chunks(per_frame).enumerate() {
        analyzer.push_samples(chunk);
        let analyzed = analyzer.analyze(dt).expect("analysis succeeds");
        let time = (frame + 1) as f64 / f64::from(FPS);
        if time >= WARMUP_SECS && analyzed.is_some_and(|audio| audio.beat_detected) {
            beats.push(time);
        }
    }
    beats
}

fn lands_on(beat: f64, onsets: &[f64]) -> bool {
    onsets
        .iter()
        .any(|&onset| (0.0..=HIT_WINDOW_SECS).contains(&(beat - onset)))
}

#[test]
fn default_threshold_fires_on_kicks_not_on_held_bass() {
    let groove = house_groove();
    let pipeline = AudioPipelineConfig::from(&AudioConfig::default());
    let beats = detected_beats(&pipeline, &groove.samples);

    let missed: Vec<f64> = groove
        .kicks
        .iter()
        .copied()
        .filter(|&kick| kick >= WARMUP_SECS)
        .filter(|&kick| !beats.iter().any(|&beat| lands_on(beat, &[kick])))
        .collect();
    assert!(missed.is_empty(), "kicks with no beat: {missed:?}");

    let off_hit: Vec<f64> = beats
        .iter()
        .copied()
        .filter(|&beat| !lands_on(beat, &groove.kicks) && !lands_on(beat, &groove.hats))
        .collect();
    let rate = off_hit.len() as f64 / (f64::from(SECONDS) - WARMUP_SECS);
    assert!(
        rate <= MAX_OFF_HIT_BEATS_PER_SEC,
        "{rate:.2} beats/s fired on the held bass between hits: {off_hit:?}"
    );
}
