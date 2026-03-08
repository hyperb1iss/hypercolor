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

// Effect functions
export { effect } from './effects'
export type { EffectFnOptions, ShaderContext } from './effects'
export { canvas } from './effects'
export type { CanvasFnOptions, DrawFn, FactoryFn } from './effects'

// Control factories
export { num, combo, toggle, color, hue, text } from './controls'
export type { ControlSpec, ControlMap, ControlShorthand } from './controls'

// Palette runtime
export { createPaletteFn, getPalette, paletteNames, samplePalette, samplePaletteCSS } from './palette'
export type { PaletteEntry, PaletteFn } from './palette'

// Audio (pull model for canvas effects)
export { getAudioData as audio } from './audio'

// ── Control helpers ─────────────────────────────────────────────────────

export type {
    BaseControls,
    BooleanControlDefinition,
    ColorControlDefinition,
    ComboboxControlDefinition,
    ControlDefinition,
    ControlDefinitionType,
    ControlValues,
    HueControlDefinition,
    NumberControlDefinition,
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

export type { EffectConfig } from './effects'
export { BaseEffect } from './effects'
export type { CanvasEffectConfig } from './effects'
export { CanvasEffect } from './effects'
export type { UniformValue, WebGLEffectConfig } from './effects'
export { WebGLEffect } from './effects'

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

export { createDebugLogger, debug, printStartupBanner } from './utils'
export type { HSLColor, RGBColor, UpdateFunction } from './utils'

// ── Initialization ──────────────────────────────────────────────────────

export { initializeEffect } from './init'
export type { InitializationMode, InitOptions } from './init'
