/**
 * Hypercolor Runtime API — type declarations for the engine contract.
 *
 * The Hypercolor daemon (Servo renderer) exposes these globals to effects
 * running inside the embedded browser. This is the bridge between effects
 * and the host environment.
 */

/**
 * Audio analysis data from the Hypercolor audio pipeline.
 */
interface HypercolorAudio {
    /** Audio level in decibels (-100 to 0, where 0 is loudest) */
    level: number
    /** Tone density (0-1, 0=pure tone, 1=white noise) */
    density: number
    /** Stereo width (0-1) */
    width: number
    /** FFT frequency data (200 elements) */
    freq: ArrayLike<number>
}

/**
 * Screen zone color sampling from a 28x20 grid (560 points).
 */
interface HypercolorZone {
    /** Grid width in zones. */
    width: number
    /** Grid height in zones. */
    height: number
    /** Hue values (0-360) for each sample point */
    hue: ArrayLike<number>
    /** Saturation values (0-100) for each sample point */
    saturation: ArrayLike<number>
    /** Lightness values (0-100) for each sample point */
    lightness: ArrayLike<number>
}

/**
 * A single sensor reading from the system monitor pipeline.
 */
interface HypercolorSensorReading {
    /** Current value (e.g., 65.5 for temperature, 42 for load %). */
    value: number
    /** Expected minimum value. */
    min: number
    /** Expected maximum value. */
    max: number
    /** Unit symbol (e.g., "°C", "%", "MB", "RPM", "W", "MHz"). */
    unit: string
}

/**
 * Hypercolor engine — central access point for all runtime data.
 *
 * The daemon injects audio, zone, and sensor data every frame via the
 * LightScript runtime. Effects and faces access it through this global.
 */
interface HypercolorEngine {
    /** Audio analysis data */
    audio: HypercolorAudio
    /** Screen zone color data */
    zone: HypercolorZone

    // ── Sensor / Meter API ─────────────────────────────────────────
    // Injected by LightscriptRuntime::sensor_update_script() every frame.

    /** All sensor readings keyed by label (e.g., "cpu_temp", "gpu_load"). */
    sensors: Record<string, HypercolorSensorReading>
    /** Ordered list of available sensor labels. */
    sensorList: string[]
    /** Fetch a sensor reading by label. Returns null if unavailable. */
    getSensorValue(name: string): HypercolorSensorReading | null
    /** Programmatically set a sensor value (for testing/custom meters). */
    setSensorValue(name: string, value: number, min: number, max: number, unit: string): void

    // ── Canvas dimensions ──────────────────────────────────────────
    // Set by the daemon to match the render canvas or display resolution.

    /** Canvas width in pixels. */
    width: number
    /** Canvas height in pixels. */
    height: number
}

/**
 * Window contract between effects and the Hypercolor runtime.
 */
declare global {
    /** Hypercolor engine — provides audio, vision, and screen data */
    var engine: HypercolorEngine

    interface Window {
        /** Called by Hypercolor when any control value changes */
        update?: (force?: boolean) => void

        /** Active effect instance reference */
        effectInstance?: {
            stop: () => void
        }

        /** Current animation frame ID */
        currentAnimationFrame?: number

        /** Number of registered controls */
        controlsCount?: number

        /** Build-time metadata extraction flag */
        __HYPERCOLOR_METADATA_ONLY__?: boolean

        /** Force WebGL effects to preserve the backbuffer between frames */
        __hypercolorPreserveDrawingBuffer?: boolean

        /** Dynamic control values indexed by control ID */
        [controlId: string]: unknown
    }
}

export {}
