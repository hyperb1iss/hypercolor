/**
 * Audio analysis data types.
 */

/**
 * Comprehensive audio analysis data for effect use.
 * All fields are computed per-frame by getAudioData().
 */
export interface AudioData {
    // ── Legacy (backwards-compatible) ───────────────────────────────
    /** Normalized overall level (0-1) */
    level: number
    /** Raw level in dB (-100 to 0) */
    levelRaw: number
    /** Tone density / spectral flatness (0-1) */
    density: number
    /** Stereo width (0-1) */
    width: number
    /** Raw FFT frequency data (200 elements) */
    frequencyRaw: Int8Array
    /** Normalized frequency data (200 elements, 0-1) */
    frequency: Float32Array
    /** Bass level (0-1) */
    bass: number
    /** Mid level (0-1) */
    mid: number
    /** Treble level (0-1) */
    treble: number
    /** Beat detection (0-1) */
    beat: number
    /** Decaying beat impulse (0-1) */
    beatPulse: number
    /** Short-term level envelope */
    levelShort: number
    /** Long-term level envelope */
    levelLong: number
    /** Bass envelope (attack) */
    bassEnv: number
    /** Mid envelope (attack) */
    midEnv: number
    /** Treble envelope (attack) */
    trebleEnv: number
    /** Tempo estimate (BPM) */
    tempo: number
    /** Level momentum (-1 to 1) */
    momentum: number
    /** Positive swell (0-1) */
    swell: number

    // ── Perceptual Frequency Analysis ───────────────────────────────
    /** A-weighted frequency data (200 elements, 0-1) */
    frequencyWeighted: Float32Array
    /** Mel-scale frequency bands (24 elements, 0-1) */
    melBands: Float32Array
    /** Mel bands with rolling AGC normalization (24 elements, 0-1) */
    melBandsNormalized: Float32Array

    // ── Onset & Rhythm Detection ────────────────────────────────────
    /** Spectral flux (0-1) — spectrum change rate */
    spectralFlux: number
    /** Band-specific spectral flux [bass, mid, treble] (0-1 each) */
    spectralFluxBands: Float32Array
    /** Onset strength (0-1) */
    onset: number
    /** Onset impulse (0-1) — decaying pulse */
    onsetPulse: number
    /** Beat phase (0-1) — position within beat cycle */
    beatPhase: number
    /** Beat confidence (0-1) */
    beatConfidence: number

    // ── Harmonic Analysis ───────────────────────────────────────────
    /** Chromagram / pitch class profile (12 elements, 0-1) */
    chromagram: Float32Array
    /** Dominant pitch class (0-11: C, C#, D, ..., B) */
    dominantPitch: number
    /** Dominant pitch confidence (0-1) */
    dominantPitchConfidence: number
    /** Harmonic hue (0-360) — Circle of Fifths mapped to color wheel */
    harmonicHue: number
    /** Chord mood (-1 to 1) — negative=minor, positive=major */
    chordMood: number

    // ── Timbre & Texture ────────────────────────────────────────────
    /** Spectral centroid / brightness (0-1) */
    brightness: number
    /** Spectral spread (0-1) */
    spread: number
    /** Spectral rolloff (0-1) */
    rolloff: number
    /** Roughness / dissonance (0-1) */
    roughness: number
}

/** Screen zone data from 28x20 grid. */
export interface ScreenZoneData {
    hue: Float32Array
    saturation: Float32Array
    lightness: Float32Array
    width: number
    height: number
}
