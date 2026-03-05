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
    /** Hue values (0-360) for each sample point */
    hue: ArrayLike<number>
    /** Saturation values (0-100) for each sample point */
    saturation: ArrayLike<number>
    /** Lightness values (0-100) for each sample point */
    lightness: ArrayLike<number>
}

/**
 * Hypercolor engine — central access point for all runtime data.
 */
interface HypercolorEngine {
    /** Audio analysis data */
    audio: HypercolorAudio
    /** Screen zone color data */
    zone: HypercolorZone
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

        /** Dynamic control values indexed by control ID */
        [controlId: string]: unknown
    }
}

export {}
