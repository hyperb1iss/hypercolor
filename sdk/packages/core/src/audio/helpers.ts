/**
 * Audio helper utilities — convenience wrappers around pre-computed data.
 */

import { AudioData } from './types'

/** Pitch class names. */
const PITCH_CLASS_NAMES = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B']

/** Circle of Fifths → hue mapping. */
const PITCH_CLASS_TO_HUE = [0, 210, 60, 270, 120, 330, 180, 30, 240, 90, 300, 150]

/** Get average level for a frequency range. */
export function getFrequencyRange(frequency: Float32Array, start: number, end: number): number {
    if (end <= start || frequency.length === 0) return 0
    let sum = 0
    const count = Math.min(end, frequency.length) - start
    for (let i = start; i < Math.min(end, frequency.length); i++) {
        sum += frequency[i]
    }
    return count > 0 ? sum / count : 0
}

/** Get bass level from frequency array. */
export function getBassLevel(frequency: Float32Array): number {
    return getFrequencyRange(frequency, 0, 10)
}

/** Get mid level from frequency array. */
export function getMidLevel(frequency: Float32Array): number {
    return getFrequencyRange(frequency, 10, 80)
}

/** Get treble level from frequency array. */
export function getTrebleLevel(frequency: Float32Array): number {
    return getFrequencyRange(frequency, 80, 200)
}

/** Normalize a single frequency bin value. */
export function normalizeFrequencyBin(value: number, max = 128): number {
    return Math.max(0, Math.min(1, Math.abs(value) / max))
}

/** Smooth a value over time using exponential moving average. */
export function smoothValue(currentValue: number, previousValue: number, smoothing = 0.5): number {
    return previousValue * smoothing + currentValue * (1 - smoothing)
}

/** Get pitch class name from index. */
export function getPitchClassName(pitchClass: number): string {
    return PITCH_CLASS_NAMES[pitchClass % 12]
}

/** Convert pitch class name to index. */
export function getPitchClassIndex(name: string): number {
    const idx = PITCH_CLASS_NAMES.indexOf(name.toUpperCase())
    return idx >= 0 ? idx : 0
}

/** Get energy for a mel band range. */
export function getMelRange(audio: AudioData, startBand: number, endBand: number): number {
    let sum = 0
    const count = Math.min(endBand, 24) - startBand
    for (let i = startBand; i < Math.min(endBand, 24); i++) {
        sum += audio.melBandsNormalized[i]
    }
    return count > 0 ? sum / count : 0
}

/** Get energy for a specific pitch class from chromagram. */
export function getPitchEnergy(audio: AudioData, pitchClass: number | string): number {
    const idx = typeof pitchClass === 'string' ? getPitchClassIndex(pitchClass) : pitchClass % 12
    return audio.chromagram[idx]
}

/** Convert HSL to RGB. */
export function hslToRgb(h: number, s: number, l: number): [number, number, number] {
    const hNorm = h / 360
    if (s === 0) return [l, l, l]

    const hue2rgb = (p: number, q: number, tIn: number) => {
        let t = tIn
        if (t < 0) t += 1
        if (t > 1) t -= 1
        if (t < 1 / 6) return p + (q - p) * 6 * t
        if (t < 1 / 2) return q
        if (t < 2 / 3) return p + (q - p) * (2 / 3 - t) * 6
        return p
    }

    const q = l < 0.5 ? l * (1 + s) : l + s - l * s
    const p = 2 * l - q
    return [hue2rgb(p, q, hNorm + 1 / 3), hue2rgb(p, q, hNorm), hue2rgb(p, q, hNorm - 1 / 3)]
}

/** Get a color harmonized with the current audio. */
export function getHarmonicColor(audio: AudioData, saturation = 0.7, lightness = 0.5): [number, number, number] {
    return hslToRgb(audio.harmonicHue, saturation, lightness)
}

/** Blend between major/minor colors based on chord mood. */
export function getMoodColor(
    majorColor: [number, number, number],
    minorColor: [number, number, number],
    audio: AudioData,
): [number, number, number] {
    const t = audio.chordMood * 0.5 + 0.5
    return [
        minorColor[0] + (majorColor[0] - minorColor[0]) * t,
        minorColor[1] + (majorColor[1] - minorColor[1]) * t,
        minorColor[2] + (majorColor[2] - minorColor[2]) * t,
    ]
}

/** Get beat anticipation value (peaks just before beat). */
export function getBeatAnticipation(audio: AudioData, anticipation = 0.2): number {
    const phase = audio.beatPhase
    const anticipate = phase > 1 - anticipation ? (phase - (1 - anticipation)) / anticipation : 0
    const release = phase < 0.1 ? ((0.1 - phase) / 0.1) * 0.5 : 0
    return Math.max(anticipate, release)
}

/** Check if we're on a beat (within tolerance). */
export function isOnBeat(audio: AudioData, division = 1, tolerance = 0.1): boolean {
    const phase = (audio.beatPhase * division) % 1
    return phase < tolerance || phase > 1 - tolerance
}

/** Get hue for a pitch class via Circle of Fifths. */
export function pitchClassToHue(pitchClass: number): number {
    return PITCH_CLASS_TO_HUE[pitchClass % 12]
}
