/**
 * @hypercolor/sdk
 *
 * TypeScript SDK for creating Hypercolor RGB lighting effects.
 *
 * ```typescript
 * import { effect } from '@hypercolor/sdk'
 * import shader from './fragment.glsl'
 *
 * export default effect('Meteor Storm', shader, {
 *     speed:       [1, 10, 5],
 *     palette:     ['SilkCircuit', 'Fire', 'Ice'],
 * })
 * ```
 *
 * @packageDocumentation
 */

// Runtime type declarations (side-effect import)
import './runtime'

// ── Declarative API ─────────────────────────────────────────────────────

// Audio (pull model for canvas effects)
export { getAudioData as audio } from './audio'
export type { ControlMap, ControlShorthand, ControlSpec } from './controls'
// Control factories (effects + faces)
export { color, combo, font, hue, num, rect, sensor, text, toggle } from './controls'
export type { CanvasFnOptions, DrawFn, EffectFnOptions, FactoryFn, ShaderContext } from './effects'
// Effect functions
export { canvas, effect } from './effects'
export type { PaletteEntry, PaletteFn } from './palette'
// Palette runtime
export { createPaletteFn, getPalette, paletteNames, samplePalette, samplePaletteCSS } from './palette'

// ── Control helpers ─────────────────────────────────────────────────────

export type {
    BaseControls,
    BooleanControlDefinition,
    ColorControlDefinition,
    ComboboxControlDefinition,
    ControlDefinition,
    ControlDefinitionType,
    ControlValues,
    FontOptions,
    HueControlDefinition,
    NumberControlDefinition,
    RectControlDefinition,
    RectOptions,
    RectValue,
    SensorOptions,
    TextFieldControlDefinition,
} from './controls'

export {
    boolToInt,
    comboboxValueToIndex,
    getAllControls,
    getControlValue,
    normalizePercentage,
    normalizeSpeed,
} from './controls'

// ── Base Classes ────────────────────────────────────────────────────────

export type { CanvasEffectConfig, EffectConfig, UniformValue, WebGLEffectConfig } from './effects'
export { BaseEffect, CanvasEffect, WebGLEffect } from './effects'

// ── Audio ───────────────────────────────────────────────────────────────

export type { AudioData, ScreenZoneData } from './audio'
export {
    FFT_SIZE,
    getAudioData,
    getBassLevel,
    getBeatAnticipation,
    getFrequencyRange,
    getHarmonicColor,
    getMelRange,
    getMidLevel,
    getMoodColor,
    getPitchClassIndex,
    getPitchClassName,
    getPitchEnergy,
    getScreenZoneData,
    getTrebleLevel,
    hslToRgb,
    isOnBeat,
    MEL_BANDS,
    normalizeAudioLevel,
    normalizeFrequencyBin,
    PITCH_CLASSES,
    pitchClassToHue,
    smoothValue,
} from './audio'

// ── Utilities ───────────────────────────────────────────────────────────

export type { HSLColor, RGBColor, UpdateFunction } from './utils'
export { createDebugLogger, debug, printStartupBanner } from './utils'

// ── Math ────────────────────────────────────────────────────────────────

export type { CanvasSize, DesignBasis, ScaleContext } from './math'
export {
    clamp,
    easeInCubic,
    easeInOutCubic,
    easeInOutQuad,
    easeInQuad,
    easeOutCubic,
    easeOutQuad,
    inverseLerp,
    lerp,
    mix,
    saturate,
    scaleContext,
    smoothApproach,
    smoothAsymmetric,
    smoothstep,
    step,
} from './math'

// ── Faces ───────────────────────────────────────────────────────────────

export type { FaceContext, FaceOptions, FaceUpdateFn, SensorAccessor, SensorReading } from './faces'
export { face } from './faces'
export {
    colorByValue,
    lerpColor,
    palette,
    parseHex,
    radius,
    sensorColors,
    spacing,
    withAlpha,
    withGlow,
} from './faces/tokens'

// ── Gauges ──────────────────────────────────────────────────────────────

export type {
    ArcGaugeOptions,
    BarGaugeOptions,
    RingGaugeOptions,
    SparklineOptions,
} from './gauges'
export { arcGauge, barGauge, ringGauge, sparkline, ValueHistory } from './gauges'

// ── Initialization ──────────────────────────────────────────────────────

export type { InitializationMode, InitOptions } from './init'
export { initializeEffect } from './init'
