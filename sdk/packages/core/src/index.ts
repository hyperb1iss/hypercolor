/**
 * hypercolor
 *
 * TypeScript SDK for creating Hypercolor RGB lighting effects.
 *
 * ```typescript
 * import { effect, paletteControl } from 'hypercolor'
 * import shader from './fragment.glsl'
 *
 * export default effect('Meteor Storm', shader, {
 *     speed:       [1, 10, 5],
 *     palette:     paletteControl('Palette', ['SilkCircuit', 'Fire', 'Ice']),
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
export { asset, color, combo, font, hue, num, paletteControl, rect, sensor, text, toggle } from './controls'
export type { CanvasFnOptions, DrawFn, EffectFnOptions, FactoryFn, ShaderContext } from './effects'
// Effect functions
export { canvas, effect } from './effects'
export type { PaletteEntry, PaletteFn } from './palette'
// Palette runtime
export { createPaletteFn, getPalette, paletteNames, samplePalette, samplePaletteCSS } from './palette'

// ── Control helpers ─────────────────────────────────────────────────────

export type {
    AssetControlDefinition,
    AssetOptions,
    BaseControls,
    BooleanControlDefinition,
    ColorControlDefinition,
    ComboboxControlDefinition,
    ControlDefinition,
    ControlDefinitionType,
    ControlValues,
    FontOptions,
    HueControlDefinition,
    MediaKind,
    NumberControlDefinition,
    PaletteControlOptions,
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

// ── Layout ──────────────────────────────────────────────────────────────

export type { AnchorPosition, AnchorSize, FitTextOptions, Point, Rect, RingOptions } from './layout'
export { anchor, center, fitText, grid, inset, polar, rail, ring } from './layout'

// ── Motion ──────────────────────────────────────────────────────────────

export type { EasingFn, SpringOptions } from './motion'
export {
    easeOutBack,
    easeOutElastic,
    linear,
    Smoothed,
    Spring,
    smoothed,
    spring,
    Timeline,
    Transition,
    Tween,
    timeline,
    transitionOnChange,
    tween,
} from './motion'

// ── Faces ───────────────────────────────────────────────────────────────

export type {
    AudioAccessor,
    FaceContext,
    FaceDataSources,
    FaceDisplayClass,
    FaceDisplayInfo,
    FaceDisplayShape,
    FaceOptions,
    FaceUpdateFn,
    FaceVariants,
    InjectedDisplayDescriptor,
    LightingAccessor,
    LightingInfo,
    MediaAccessor,
    MediaInfo,
    NetAccessor,
    NetInfo,
    SensorAccessor,
    SensorReading,
} from './faces'
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
    AnimatedArcGauge,
    AnimatedBarGauge,
    AnimatedRingGauge,
    ArcGaugeOptions,
    BarGaugeOptions,
    GaugeAnimateOptions,
    RingGaugeOptions,
    SparklineBand,
    SparklineOptions,
} from './gauges'
export {
    arcGauge,
    barGauge,
    createArcGauge,
    createBarGauge,
    createRingGauge,
    ringGauge,
    sparkline,
    ValueHistory,
} from './gauges'

// ── Initialization ──────────────────────────────────────────────────────

export type { InitializationMode, InitOptions } from './init'
export { initializeEffect } from './init'
