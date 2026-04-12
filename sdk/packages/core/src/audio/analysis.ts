/**
 * Audio data access — thin wrapper around Hypercolor runtime.
 *
 * The heavy DSP (mel filterbank, chromagram, spectral flux, beat tracking)
 * runs in the Rust daemon. Effects just read pre-computed values from
 * `engine.audio.*`. This module provides typed access and silent fallbacks.
 */

import { AudioData, ScreenZoneData } from './types'

/** Number of FFT bins from the engine. */
export const FFT_SIZE = 200

/** Number of mel bands. */
export const MEL_BANDS = 24

/** Number of pitch classes for chromagram. */
export const PITCH_CLASSES = 12

/**
 * Get audio analysis data from the Hypercolor runtime.
 * Returns silent defaults when running outside the daemon.
 */
export function getAudioData(): AudioData {
    const hasEngine = typeof engine !== 'undefined' && engine?.audio
    if (!hasEngine) return createSilentAudioData()

    const audio = engine.audio as any

    return {
        // Legacy
        bass: audio.bass ?? 0,
        bassEnv: audio.bassEnv ?? 0,
        beat: Number(audio.beat ?? 0),
        beatConfidence: audio.beatConfidence ?? 0,
        beatPhase: audio.beatPhase ?? 0,
        beatPulse: audio.beatPulse ?? 0,
        brightness: audio.brightness ?? 0.5,
        chordMood: audio.chordMood ?? 0,
        chromagram: audio.chromagram ?? new Float32Array(PITCH_CLASSES),
        density: audio.density ?? 0,
        dominantPitch: audio.dominantPitch ?? 0,
        dominantPitchConfidence: audio.dominantPitchConfidence ?? 0,
        frequency: audio.frequency ?? new Float32Array(FFT_SIZE),
        frequencyRaw: audio.frequencyRaw ?? new Int8Array(FFT_SIZE),
        frequencyWeighted: audio.frequencyWeighted ?? new Float32Array(FFT_SIZE),
        harmonicHue: audio.harmonicHue ?? 0,
        level: audio.level != null ? normalizeAudioLevel(audio.level) : 0,
        levelLong: audio.levelLong ?? 0,
        levelRaw: audio.level ?? -100,
        levelShort: audio.levelShort ?? 0,
        melBands: audio.melBands ?? new Float32Array(MEL_BANDS),
        melBandsNormalized: audio.melBandsNormalized ?? new Float32Array(MEL_BANDS),
        mid: audio.mid ?? 0,
        midEnv: audio.midEnv ?? 0,
        momentum: audio.momentum ?? 0,
        onset: Number(audio.onset ?? 0),
        onsetPulse: audio.onsetPulse ?? 0,
        rolloff: audio.rolloff ?? 0.5,
        roughness: audio.roughness ?? 0.2,
        spectralFlux: audio.spectralFlux ?? 0,
        spectralFluxBands: audio.spectralFluxBands ?? new Float32Array(3),
        spread: audio.spread ?? 0.3,
        swell: audio.swell ?? 0,
        tempo: audio.tempo ?? 120,
        treble: audio.treble ?? 0,
        trebleEnv: audio.trebleEnv ?? 0,
        width: audio.width ?? 0.5,
    }
}

/** Get screen zone color data from the runtime. */
export function getScreenZoneData(): ScreenZoneData {
    const hasEngine = typeof engine !== 'undefined' && engine?.zone
    if (!hasEngine) {
        return {
            height: 20,
            hue: new Float32Array(560),
            lightness: new Float32Array(560),
            saturation: new Float32Array(560),
            width: 28,
        }
    }

    const width = Number.isFinite(engine.zone.width) ? Math.max(1, Math.floor(engine.zone.width)) : 28
    const height = Number.isFinite(engine.zone.height) ? Math.max(1, Math.floor(engine.zone.height)) : 20
    const sampleCount = width * height
    const hue = new Float32Array(sampleCount)
    const saturation = new Float32Array(sampleCount)
    const lightness = new Float32Array(sampleCount)

    for (let i = 0; i < sampleCount; i++) {
        hue[i] = engine.zone.hue[i] ?? 0
        saturation[i] = (engine.zone.saturation[i] ?? 0) / 100
        lightness[i] = (engine.zone.lightness[i] ?? 0) / 100
    }

    return { height, hue, lightness, saturation, width }
}

/** Normalize dB level (-100..0) to 0..1. */
export function normalizeAudioLevel(levelDb: number): number {
    return Math.max(0, Math.min(1, (levelDb + 100) / 100))
}

function createSilentAudioData(): AudioData {
    return {
        bass: 0,
        bassEnv: 0,
        beat: 0,
        beatConfidence: 0,
        beatPhase: 0,
        beatPulse: 0,
        brightness: 0.5,
        chordMood: 0,
        chromagram: new Float32Array(PITCH_CLASSES),
        density: 0,
        dominantPitch: 0,
        dominantPitchConfidence: 0,
        frequency: new Float32Array(FFT_SIZE),
        frequencyRaw: new Int8Array(FFT_SIZE),
        frequencyWeighted: new Float32Array(FFT_SIZE),
        harmonicHue: 0,
        level: 0,
        levelLong: 0,
        levelRaw: -100,
        levelShort: 0,
        melBands: new Float32Array(MEL_BANDS),
        melBandsNormalized: new Float32Array(MEL_BANDS),
        mid: 0,
        midEnv: 0,
        momentum: 0,
        onset: 0,
        onsetPulse: 0,
        rolloff: 0.5,
        roughness: 0.2,
        spectralFlux: 0,
        spectralFluxBands: new Float32Array(3),
        spread: 0.3,
        swell: 0,
        tempo: 120,
        treble: 0,
        trebleEnv: 0,
        width: 0.5,
    }
}
