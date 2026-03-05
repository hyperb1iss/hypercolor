/**
 * @hypercolor/sdk
 *
 * TypeScript SDK for creating Hypercolor RGB lighting effects.
 * Provides decorators for control definitions, base effect classes,
 * and audio-reactive utilities.
 *
 * @example
 * ```typescript
 * import {
 *   CanvasEffect,
 *   NumberControl,
 *   Effect,
 *   initializeEffect
 * } from '@hypercolor/sdk'
 *
 * @Effect({ name: 'My Effect', description: 'A demo', author: 'me' })
 * class MyEffect extends CanvasEffect<MyControls> {
 *   @NumberControl({ label: 'Speed', min: 0, max: 10, default: 1 })
 *   speed!: number
 * }
 *
 * initializeEffect(() => new MyEffect({ id: 'demo', name: 'Demo' }).initialize())
 * ```
 *
 * @packageDocumentation
 */

// Runtime type declarations (side-effect import)
import './runtime'

// ── Controls ────────────────────────────────────────────────────────────

export type {
    BaseControls,
    BooleanControlDefinition,
    BooleanControlOptions,
    ColorControlDefinition,
    ColorControlOptions,
    ComboboxControlDefinition,
    ComboboxControlOptions,
    ControlDecoratorOptions,
    ControlDefinition,
    ControlDefinitionType,
    ControlValues,
    EffectOptions,
    HueControlDefinition,
    HueControlOptions,
    NumberControlDefinition,
    NumberControlOptions,
    TextFieldControlDefinition,
    TextFieldControlOptions,
} from './controls'

export {
    BooleanControl,
    boolToInt,
    ColorControl,
    ComboboxControl,
    comboboxValueToIndex,
    Effect,
    extractControlsFromClass,
    extractEffectMetadata,
    getAllControls,
    getControlForProperty,
    getControlValue,
    HueControl,
    METADATA_KEYS,
    normalizePercentage,
    normalizeSpeed,
    NumberControl,
    TextFieldControl,
} from './controls'

// ── Effects ─────────────────────────────────────────────────────────────

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

export type InitializationMode = 'immediate' | 'deferred' | 'metadata-only'

export interface InitOptions {
    mode?: InitializationMode
    onReady?: () => void
}

function detectInitMode(): InitializationMode {
    if (typeof window === 'undefined') return 'metadata-only'
    if ((window as Record<string, unknown>).__HYPERCOLOR_METADATA_ONLY__) return 'metadata-only'
    return 'immediate'
}

/**
 * Initialize a Hypercolor effect with proper lifecycle handling.
 * In metadata-only mode, stores the effect instance for build-time extraction.
 */
export function initializeEffect(
    initFunction: () => void,
    options: InitOptions & { instance?: object } = {},
): void {
    const mode = options.mode ?? detectInitMode()
    if (mode === 'metadata-only') {
        // Store instance for metadata extraction by build tools
        if (options.instance) {
            ;(globalThis as any).__hypercolorEffectInstance__ = options.instance
        }
        return
    }

    if (document.readyState === 'complete' || document.readyState === 'interactive') {
        initFunction()
        options.onReady?.()
        return
    }

    window.addEventListener('DOMContentLoaded', () => {
        if ((window as Record<string, unknown>).__HYPERCOLOR_METADATA_ONLY__) return
        initFunction()
        options.onReady?.()
    }, { once: true })
}
