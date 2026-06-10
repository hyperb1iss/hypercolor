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

// ── Typed data sources (engine.media / engine.net / engine.lighting) ───

/** Now-playing snapshot mirrored from `engine.media`. */
export interface MediaInfo {
    available: boolean
    playing: boolean
    track: string
    artist: string
    album: string
    /** `data:image/jpeg;base64,...` album art, or null when the track has none. */
    artDataUrl: string | null
    positionMs: number
    durationMs: number
    /** Bus identity of the tracked player (e.g. `org.mpris.MediaPlayer2.spotify`). */
    player: string
}

/** Typed reader over `engine.media`, safe when the source is absent. */
export interface MediaAccessor {
    /** Latest snapshot; the unavailable default when no source is injected. */
    state(): MediaInfo

    /** Whether a media player is currently reachable. */
    available(): boolean

    /**
     * Playback position in milliseconds, extrapolated between the host's
     * coarse position updates so progress bars glide instead of stepping.
     */
    positionMs(): number

    /** Playback progress in [0, 1]; 0 when duration is unknown. */
    progress(): number
}

/** Network throughput snapshot mirrored from `engine.net`. */
export interface NetInfo {
    /** Receive rate in bytes per second. */
    rxBps: number
    /** Transmit rate in bytes per second. */
    txBps: number
    /** Interface the rates were measured on. */
    iface: string
}

/** Typed reader over `engine.net`, zeros when the source is absent. */
export interface NetAccessor {
    state(): NetInfo
}

/** Rig lighting snapshot mirrored from `engine.lighting`. */
export interface LightingInfo {
    sceneName: string | null
    effectNames: string[]
    /** Hex `#rrggbb` strings, ready for canvas fill styles. */
    dominantColors: string[]
}

/** Typed reader over `engine.lighting`, empty when the source is absent. */
export interface LightingAccessor {
    state(): LightingInfo
}

/** All typed data sources handed to a face's update function. */
export interface FaceDataSources {
    media: MediaAccessor
    net: NetAccessor
    lighting: LightingAccessor
}

/** Signature of the update function returned by a face's setup function. */
export type FaceUpdateFn = (
    time: number,
    controls: Record<string, unknown>,
    sensors: SensorAccessor,
    audio: AudioAccessor,
    data: FaceDataSources,
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

function engineRecord(key: string): Record<string, unknown> | undefined {
    if (typeof engine === 'undefined' || engine === null) return undefined
    const value = (engine as unknown as Record<string, unknown>)[key]
    return typeof value === 'object' && value !== null ? (value as Record<string, unknown>) : undefined
}

function asString(value: unknown, fallback = ''): string {
    return typeof value === 'string' ? value : fallback
}

function asNumber(value: unknown): number {
    return typeof value === 'number' && Number.isFinite(value) ? value : 0
}

function asStringArray(value: unknown): string[] {
    if (!Array.isArray(value)) return []
    return value.filter((item): item is string => typeof item === 'string')
}

const UNAVAILABLE_MEDIA: MediaInfo = {
    album: '',
    artDataUrl: null,
    artist: '',
    available: false,
    durationMs: 0,
    player: '',
    playing: false,
    positionMs: 0,
    track: '',
}

function readMediaInfo(): MediaInfo {
    const media = engineRecord('media')
    if (!media) return UNAVAILABLE_MEDIA
    return {
        album: asString(media.album),
        artDataUrl: typeof media.artDataUrl === 'string' ? media.artDataUrl : null,
        artist: asString(media.artist),
        available: media.available === true,
        durationMs: asNumber(media.durationMs),
        player: asString(media.player),
        playing: media.playing === true,
        positionMs: asNumber(media.positionMs),
        track: asString(media.track),
    }
}

function nowMs(): number {
    return typeof performance !== 'undefined' && typeof performance.now === 'function'
        ? performance.now()
        : Date.now()
}

/** Build a MediaAccessor over the current engine.media state. */
export function buildMediaAccessor(): MediaAccessor {
    let lastRawPositionMs = -1
    let lastRawTrack = ''
    let wasPlaying = false
    let baselineAtMs = 0

    const extrapolatedPositionMs = (state: MediaInfo): number => {
        if (
            state.positionMs !== lastRawPositionMs ||
            state.track !== lastRawTrack ||
            state.playing !== wasPlaying
        ) {
            lastRawPositionMs = state.positionMs
            lastRawTrack = state.track
            wasPlaying = state.playing
            baselineAtMs = nowMs()
        }
        if (!state.playing) return state.positionMs
        const extrapolated = state.positionMs + (nowMs() - baselineAtMs)
        return state.durationMs > 0 ? Math.min(extrapolated, state.durationMs) : extrapolated
    }

    return {
        available(): boolean {
            return readMediaInfo().available
        },
        positionMs(): number {
            return extrapolatedPositionMs(readMediaInfo())
        },
        progress(): number {
            const state = readMediaInfo()
            if (state.durationMs <= 0) return 0
            return Math.max(0, Math.min(1, extrapolatedPositionMs(state) / state.durationMs))
        },
        state(): MediaInfo {
            return readMediaInfo()
        },
    }
}

/** Build a NetAccessor over the current engine.net state. */
export function buildNetAccessor(): NetAccessor {
    return {
        state(): NetInfo {
            const net = engineRecord('net')
            return {
                iface: asString(net?.iface),
                rxBps: asNumber(net?.rxBps),
                txBps: asNumber(net?.txBps),
            }
        },
    }
}

/** Build a LightingAccessor over the current engine.lighting state. */
export function buildLightingAccessor(): LightingAccessor {
    return {
        state(): LightingInfo {
            const lighting = engineRecord('lighting')
            return {
                dominantColors: asStringArray(lighting?.dominantColors),
                effectNames: asStringArray(lighting?.effectNames),
                sceneName: typeof lighting?.sceneName === 'string' ? lighting.sceneName : null,
            }
        },
    }
}

/** Build the full typed data-source bundle for a face update loop. */
export function buildFaceDataSources(): FaceDataSources {
    return {
        lighting: buildLightingAccessor(),
        media: buildMediaAccessor(),
        net: buildNetAccessor(),
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
