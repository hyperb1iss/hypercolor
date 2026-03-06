use hypercolor_types::audio::{
    AudioData, AudioPipelineConfig, AudioSourceType, CHROMA_BINS, MEL_BANDS, SPECTRUM_BINS,
};

// ─── AudioData::silence ───────────────────────────────────────────────

#[test]
fn silence_has_correct_spectrum_length() {
    let data = AudioData::silence();
    assert_eq!(data.spectrum.len(), SPECTRUM_BINS);
}

#[test]
fn silence_has_correct_mel_bands_length() {
    let data = AudioData::silence();
    assert_eq!(data.mel_bands.len(), MEL_BANDS);
}

#[test]
fn silence_has_correct_chromagram_length() {
    let data = AudioData::silence();
    assert_eq!(data.chromagram.len(), CHROMA_BINS);
}

#[test]
fn silence_all_levels_zero() {
    let data = AudioData::silence();
    assert!(data.rms_level.abs() < f32::EPSILON);
    assert!(data.peak_level.abs() < f32::EPSILON);
    assert!(data.bpm.abs() < f32::EPSILON);
    assert!(data.beat_confidence.abs() < f32::EPSILON);
    assert!(data.spectral_centroid.abs() < f32::EPSILON);
    assert!(data.spectral_flux.abs() < f32::EPSILON);
}

#[test]
fn silence_no_beat_or_onset() {
    let data = AudioData::silence();
    assert!(!data.beat_detected);
    assert!(!data.onset_detected);
}

#[test]
fn silence_spectrum_all_zeroes() {
    let data = AudioData::silence();
    assert!(data.spectrum.iter().all(|&v| v.abs() < f32::EPSILON));
    assert!(data.mel_bands.iter().all(|&v| v.abs() < f32::EPSILON));
    assert!(data.chromagram.iter().all(|&v| v.abs() < f32::EPSILON));
}

// ─── Band convenience methods ─────────────────────────────────────────

#[test]
fn bass_returns_average_of_first_40_bins() {
    let mut data = AudioData::silence();
    // Set bins 0..40 to 0.5
    for bin in &mut data.spectrum[..40] {
        *bin = 0.5;
    }
    let bass = data.bass();
    assert!((bass - 0.5).abs() < f32::EPSILON);
}

#[test]
fn mid_returns_average_of_bins_40_to_130() {
    let mut data = AudioData::silence();
    for bin in &mut data.spectrum[40..130] {
        *bin = 0.8;
    }
    let mid = data.mid();
    assert!((mid - 0.8).abs() < 1e-6, "mid was {mid}, expected ~0.8");
}

#[test]
fn treble_returns_average_of_bins_130_to_end() {
    let mut data = AudioData::silence();
    for bin in &mut data.spectrum[130..] {
        *bin = 0.3;
    }
    let treble = data.treble();
    assert!((treble - 0.3).abs() < f32::EPSILON);
}

#[test]
fn silence_bands_are_zero() {
    let data = AudioData::silence();
    assert!(data.bass().abs() < f32::EPSILON);
    assert!(data.mid().abs() < f32::EPSILON);
    assert!(data.treble().abs() < f32::EPSILON);
}

#[test]
fn band_methods_handle_empty_spectrum() {
    let data = AudioData {
        spectrum: vec![],
        mel_bands: vec![],
        chromagram: vec![],
        beat_detected: false,
        beat_confidence: 0.0,
        beat_phase: 0.0,
        beat_pulse: 0.0,
        bpm: 0.0,
        rms_level: 0.0,
        peak_level: 0.0,
        spectral_centroid: 0.0,
        spectral_flux: 0.0,
        onset_detected: false,
        onset_pulse: 0.0,
    };
    assert!(data.bass().abs() < f32::EPSILON);
    assert!(data.mid().abs() < f32::EPSILON);
    assert!(data.treble().abs() < f32::EPSILON);
}

#[test]
fn band_methods_handle_short_spectrum() {
    let data = AudioData {
        spectrum: vec![1.0; 50], // shorter than mid/treble ranges
        mel_bands: vec![],
        chromagram: vec![],
        beat_detected: false,
        beat_confidence: 0.0,
        beat_phase: 0.0,
        beat_pulse: 0.0,
        bpm: 0.0,
        rms_level: 0.0,
        peak_level: 0.0,
        spectral_centroid: 0.0,
        spectral_flux: 0.0,
        onset_detected: false,
        onset_pulse: 0.0,
    };
    // Bass covers 0..40, all 1.0
    assert!((data.bass() - 1.0).abs() < f32::EPSILON);
    // Mid covers 40..50 (clamped to spectrum length)
    assert!((data.mid() - 1.0).abs() < f32::EPSILON);
    // Treble covers 130..50 → empty → 0.0
    assert!(data.treble().abs() < f32::EPSILON);
}

// ─── AudioSourceType ──────────────────────────────────────────────────

#[test]
fn source_type_default_is_system_monitor() {
    assert_eq!(AudioSourceType::default(), AudioSourceType::SystemMonitor);
}

#[test]
fn source_type_named_holds_string() {
    let src = AudioSourceType::Named("alsa_output.monitor".into());
    if let AudioSourceType::Named(name) = &src {
        assert_eq!(name, "alsa_output.monitor");
    } else {
        panic!("Expected Named variant");
    }
}

#[test]
fn source_type_variants_are_distinct() {
    assert_ne!(AudioSourceType::SystemMonitor, AudioSourceType::Microphone);
    assert_ne!(AudioSourceType::Microphone, AudioSourceType::None);
    assert_ne!(AudioSourceType::None, AudioSourceType::Named(String::new()));
}

// ─── AudioPipelineConfig ──────────────────────────────────────────────────────

#[test]
fn config_default_values() {
    let cfg = AudioPipelineConfig::default();
    assert_eq!(cfg.source, AudioSourceType::SystemMonitor);
    assert_eq!(cfg.fft_size, 1024);
    assert!((cfg.smoothing - 0.15).abs() < f32::EPSILON);
    assert!((cfg.gain - 1.0).abs() < f32::EPSILON);
    assert!((cfg.noise_floor - (-60.0)).abs() < f32::EPSILON);
    assert!((cfg.beat_sensitivity - 1.5).abs() < f32::EPSILON);
}

#[test]
fn config_custom_values() {
    let cfg = AudioPipelineConfig {
        source: AudioSourceType::Microphone,
        fft_size: 2048,
        smoothing: 0.3,
        gain: 2.5,
        noise_floor: -40.0,
        beat_sensitivity: 0.8,
    };
    assert_eq!(cfg.source, AudioSourceType::Microphone);
    assert_eq!(cfg.fft_size, 2048);
    assert!((cfg.smoothing - 0.3).abs() < f32::EPSILON);
    assert!((cfg.gain - 2.5).abs() < f32::EPSILON);
}

// ─── Serde round-trip ─────────────────────────────────────────────────

#[test]
fn audio_data_json_round_trip() {
    let original = AudioData::silence();
    let json = serde_json::to_string(&original).expect("serialize AudioData");
    let restored: AudioData = serde_json::from_str(&json).expect("deserialize AudioData");
    assert_eq!(original, restored);
}

#[test]
fn audio_config_json_round_trip() {
    let original = AudioPipelineConfig {
        source: AudioSourceType::Named("hw:0".into()),
        fft_size: 4096,
        smoothing: 0.5,
        gain: 3.0,
        noise_floor: -50.0,
        beat_sensitivity: 2.0,
    };
    let json = serde_json::to_string(&original).expect("serialize AudioPipelineConfig");
    let restored: AudioPipelineConfig =
        serde_json::from_str(&json).expect("deserialize AudioPipelineConfig");
    assert_eq!(original, restored);
}

#[test]
fn source_type_json_round_trip() {
    let variants = [
        AudioSourceType::SystemMonitor,
        AudioSourceType::Named("test_device".into()),
        AudioSourceType::Microphone,
        AudioSourceType::None,
    ];
    for variant in &variants {
        let json = serde_json::to_string(variant).expect("serialize AudioSourceType");
        let restored: AudioSourceType =
            serde_json::from_str(&json).expect("deserialize AudioSourceType");
        assert_eq!(variant, &restored);
    }
}
