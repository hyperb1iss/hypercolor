/**
 * Face runtime context — what the setup and update functions receive.
 */

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
     *  1.0 when the display matches designBasis exactly. */
    scale: number
    /** Device pixel ratio (always 1 in Servo, but correct for browser testing). */
    dpr: number
}

/** Signature of the update function returned by a face's setup function. */
export type FaceUpdateFn = (
    time: number,
    controls: Record<string, unknown>,
    sensors: SensorAccessor,
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

/** Build a SensorAccessor from the current engine.sensors state. */
export function buildSensorAccessor(): SensorAccessor {
    return {
        read(label: string): SensorReading | null {
            if (typeof engine === 'undefined') return null
            if (typeof engine.getSensorValue === 'function') {
                return engine.getSensorValue(label)
            }
            return engine.sensors?.[label] ?? null
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

        formatted(label: string): string {
            const reading = this.read(label)
            if (!reading) return '--'
            return formatSensorValue(reading)
        },
    }
}
