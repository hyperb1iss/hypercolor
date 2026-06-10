/**
 * Face runtime context — what the setup and update functions receive.
 */

import { getAudioData } from '../audio'
import type { AudioData } from '../audio/types'
import type { Rect } from '../layout'

/** A single sensor reading from the Hypercolor sensor pipeline. */
export interface SensorReading {
    value: number
    min: number
    max: number
    unit: string
}

/** Convenience wrapper around engine.sensors for face update functions. */
export interface SensorAccessor {
    /** Read a sensor by label. Returns null if unavailable. */
    read(label: string): SensorReading | null

    /** All available sensor labels. */
    list(): string[]

    /**
     * Read a sensor and normalize its value to [0, 1] based on min/max.
     * Returns 0 if the sensor is unavailable.
     */
    normalized(label: string): number

    /**
     * Formatted display string with appropriate precision and unit.
     * Returns '--' if the sensor is unavailable.
     *
     * @example
     * sensors.formatted('cpu_temp')  // "65°C"
     * sensors.formatted('ram_used')  // "62%"
     * sensors.formatted('gpu_load')  // "78%"
     */
    formatted(label: string): string
}

/** Broad display shape a face adapts its layout to. */
export type FaceDisplayShape = 'round' | 'square' | 'tall' | 'wide'

/** Device family hint for layout idiom selection. */
export type FaceDisplayClass = 'panel' | 'pump-lcd' | 'strip'

/** Device truth about the surface the face renders on. */
export interface FaceDisplayInfo {
    shape: FaceDisplayShape
    class: FaceDisplayClass
    /** Device truth when the daemon injected a descriptor, otherwise the
     *  author's declaration. */
    circular: boolean
    /** Width over height. */
    aspect: number
    /** Largest rect free of physical clipping (inscribed square on round
     *  panels, the full surface otherwise). */
    safeArea: Rect
}

/** Shape of `window.hypercolor.display` injected by the daemon (v1). */
export interface InjectedDisplayDescriptor {
    apiVersion: number
    width: number
    height: number
    circular: boolean
    shape: string
    class: string
    safeArea: { x: number; y: number; width: number; height: number }
    targetFps: number
    pixelFormat: string
}

const WIDE_ASPECT_THRESHOLD = 2

/** The daemon-injected display descriptor, when present. */
export function injectedDisplayDescriptor(): InjectedDisplayDescriptor | undefined {
    if (typeof globalThis !== 'object' || globalThis === null) return undefined
    const hypercolor = (globalThis as Record<string, unknown>).hypercolor as Record<string, unknown> | undefined
    const display = hypercolor?.display as InjectedDisplayDescriptor | undefined
    if (!display || typeof display.width !== 'number' || typeof display.height !== 'number') {
        return undefined
    }
    return display
}

function deriveShape(width: number, height: number, circular: boolean): FaceDisplayShape {
    if (circular) return 'round'
    const aspect = width / Math.max(height, 1)
    if (aspect >= WIDE_ASPECT_THRESHOLD) return 'wide'
    if (aspect <= 1 / WIDE_ASPECT_THRESHOLD) return 'tall'
    return 'square'
}

function defaultClass(shape: FaceDisplayShape): FaceDisplayClass {
    if (shape === 'round') return 'pump-lcd'
    if (shape === 'square') return 'panel'
    return 'strip'
}

function deriveSafeArea(width: number, height: number, shape: FaceDisplayShape): Rect {
    if (shape !== 'round') return { height, width, x: 0, y: 0 }
    const side = Math.floor(Math.min(width, height) / Math.SQRT2)
    return {
        height: side,
        width: side,
        x: Math.floor((width - side) / 2),
        y: Math.floor((height - side) / 2),
    }
}

function isFaceDisplayShape(value: string): value is FaceDisplayShape {
    return value === 'round' || value === 'square' || value === 'tall' || value === 'wide'
}

function isFaceDisplayClass(value: string): value is FaceDisplayClass {
    return value === 'panel' || value === 'pump-lcd' || value === 'strip'
}

/**
 * Resolve display truth for a face: the injected descriptor wins; without
 * one (bare-browser authoring) the same derivation runs over the measured
 * viewport plus the author's `circular` declaration.
 */
export function resolveDisplayInfo(
    width: number,
    height: number,
    authorCircular: boolean,
    injected: InjectedDisplayDescriptor | undefined = injectedDisplayDescriptor(),
): FaceDisplayInfo {
    if (injected) {
        const derived = deriveShape(injected.width, injected.height, injected.circular)
        const shape = isFaceDisplayShape(injected.shape) ? injected.shape : derived
        return {
            aspect: injected.width / Math.max(injected.height, 1),
            circular: injected.circular,
            class: isFaceDisplayClass(injected.class) ? injected.class : defaultClass(shape),
            safeArea: injected.safeArea ?? deriveSafeArea(injected.width, injected.height, shape),
            shape,
        }
    }

    const shape = deriveShape(width, height, authorCircular)
    return {
        aspect: width / Math.max(height, 1),
        circular: authorCircular,
        class: defaultClass(shape),
        safeArea: deriveSafeArea(width, height, shape),
        shape,
    }
}

/** Runtime context passed to the face setup function. */
export interface FaceContext {
    /** Full-display DOM container. Append child elements here. */
    container: HTMLDivElement
    /** Canvas overlay — same size as container, z-indexed above DOM children.
     *  Use for custom drawing (gauges, sparklines, graphics). */
    canvas: HTMLCanvasElement
    /** Canvas 2D rendering context (from the overlay canvas). */
    ctx: CanvasRenderingContext2D
    /** Display width in CSS pixels. */
    width: number
    /** Display height in CSS pixels. */
    height: number
    /** Whether the display is circular (e.g., some AIO LCDs). */
    circular: boolean
    /** Scale factor from designBasis to actual display dimensions.
     *  1.0 when the display matches designBasis exactly. Computed from
     *  min(width, height) so wide strips keep readable type. */
    scale: number
    /** Device pixel ratio (always 1 in Servo, but correct for browser testing). */
    dpr: number
    /** Device truth: shape, class, aspect, and the unclipped safe area. */
    display: FaceDisplayInfo
}

/** Convenience wrapper around engine.audio for face update functions. */
export interface AudioAccessor {
    /** Current frame's audio analysis. Silent data when audio is absent. */
    data(): AudioData

    /** Whether live audio data is being injected by the host. */
    available(): boolean
}

/** Signature of the update function returned by a face's setup function. */
export type FaceUpdateFn = (
    time: number,
    controls: Record<string, unknown>,
    sensors: SensorAccessor,
    audio: AudioAccessor,
) => void

// ── SensorAccessor implementation ──────────────────────────────────────

function formatSensorValue(reading: SensorReading): string {
    const { value, unit } = reading
    // Temperature — no decimal for whole numbers
    if (unit === '°C' || unit === '°F') {
        return `${Math.round(value)}${unit}`
    }
    // Percentage
    if (unit === '%') {
        return `${Math.round(value)}%`
    }
    // Megabytes — one decimal
    if (unit === 'MB') {
        return value >= 1024 ? `${(value / 1024).toFixed(1)} GB` : `${Math.round(value)} MB`
    }
    // RPM, Watts, MHz — whole numbers with unit
    if (unit === 'RPM' || unit === 'W' || unit === 'MHz') {
        return `${Math.round(value)} ${unit}`
    }
    // Fallback
    return `${value.toFixed(1)} ${unit}`
}

/** Build an AudioAccessor over the current engine.audio state. */
export function buildAudioAccessor(): AudioAccessor {
    return {
        available(): boolean {
            return typeof engine !== 'undefined' && Boolean(engine?.audio)
        },
        data(): AudioData {
            return getAudioData()
        },
    }
}

/** Build a SensorAccessor from the current engine.sensors state. */
export function buildSensorAccessor(): SensorAccessor {
    return {
        formatted(label: string): string {
            const reading = this.read(label)
            if (!reading) return '--'
            return formatSensorValue(reading)
        },

        list(): string[] {
            if (typeof engine === 'undefined') return []
            return engine.sensorList ?? Object.keys(engine.sensors ?? {})
        },

        normalized(label: string): number {
            const reading = this.read(label)
            if (!reading) return 0
            const range = reading.max - reading.min
            if (range <= 0) return 0
            return Math.max(0, Math.min(1, (reading.value - reading.min) / range))
        },
        read(label: string): SensorReading | null {
            if (typeof engine === 'undefined') return null
            if (typeof engine.getSensorValue === 'function') {
                return engine.getSensorValue(label)
            }
            return engine.sensors?.[label] ?? null
        },
    }
}
