/**
 * face() — declarative display face API.
 *
 * Mirrors the pattern of effect() and canvas() but targets LCD displays.
 * Produces DOM-aware faces with sensor data access, canvas overlay for
 * custom drawing, and automatic display-resolution scaling.
 *
 * @example
 * ```typescript
 * import { face, sensor, color, arcGauge, palette } from '@hypercolor/sdk'
 *
 * export default face('System Monitor', {
 *     cpuSensor: sensor('CPU Sensor', 'cpu_temp'),
 *     accent:    color('Accent', palette.neonCyan),
 * }, {
 *     description: 'Animated system dashboard',
 *     designBasis: { width: 480, height: 480 },
 * }, (ctx) => {
 *     // Setup: create DOM, initialize state
 *     return (time, controls, sensors) => {
 *         // Update: draw each frame
 *         const temp = sensors.read(controls.cpuSensor as string)
 *         arcGauge(ctx.ctx, { cx: 240, cy: 240, radius: 100, ... })
 *     }
 * })
 * ```
 */

import { getControlValue } from '../controls/helpers'
import type { ControlMap } from '../controls/infer'
import { inferControl } from '../controls/infer'
import { deriveLabel } from '../controls/names'
import { isControlSpec } from '../controls/specs'
import type { DesignBasis } from '../math/scale'
import type { FaceContext, FaceUpdateFn } from './context'
import { buildSensorAccessor } from './context'

// ── Types ───────────────────────────────────────────────────────────────

/** Options for the face() declaration. */
export interface FaceOptions {
    description?: string
    author?: string
    /** Design resolution the face is authored against (default: 480x480). */
    designBasis?: DesignBasis
    /** Whether this face is designed for circular displays. */
    circular?: boolean
    /** Named presets with control overrides. */
    presets?: FacePresetDef[]
}

export interface FacePresetDef {
    name: string
    description?: string
    controls: Record<string, unknown>
}

/** Face setup function — receives context, returns update function. */
type FaceSetupFn = (ctx: FaceContext) => FaceUpdateFn

// ── Resolved control ────────────────────────────────────────────────────

interface ResolvedFaceControl {
    key: string
    spec: import('../controls/specs').ControlSpec
}

function resolveFaceControls(controls: ControlMap): ResolvedFaceControl[] {
    const resolved: ResolvedFaceControl[] = []
    for (const [key, value] of Object.entries(controls)) {
        const spec = isControlSpec(value) ? value : inferControl(key, value, deriveLabel(key))
        resolved.push({ key, spec })
    }
    return resolved
}

function resolveControlValues(controls: ResolvedFaceControl[]): Record<string, unknown> {
    const result: Record<string, unknown> = {}
    for (const ctrl of controls) {
        result[ctrl.key] = getControlValue(ctrl.key, ctrl.spec.defaultValue)
    }
    return result
}

// ── Metadata ────────────────────────────────────────────────────────────

interface FaceDef {
    type: 'face'
    name: string
    controls: ControlMap
    resolvedControls: ResolvedFaceControl[]
    description?: string
    author?: string
    designBasis?: DesignBasis
    circular?: boolean
    presets?: FacePresetDef[]
}

function storeFaceMetadata(def: FaceDef): void {
    // Store in the same global as effects — the build script discriminates
    // by `type: 'face'` vs `type: 'canvas'` / `type: 'webgl'`.
    const g = globalThis as Record<string, unknown>
    const defs = (g.__hypercolorEffectDefs__ as unknown[]) ?? []
    defs.push(def)
    g.__hypercolorEffectDefs__ = defs
}

// ── Runtime ─────────────────────────────────────────────────────────────

function createFaceContext(
    container: HTMLDivElement,
    canvas: HTMLCanvasElement,
    designBasis: DesignBasis,
    circular: boolean,
): FaceContext {
    const width = container.clientWidth || designBasis.width
    const height = container.clientHeight || designBasis.height
    const dpr = typeof devicePixelRatio !== 'undefined' ? devicePixelRatio : 1
    const scale = Math.min(width / designBasis.width, height / designBasis.height)

    container.style.position = 'relative'
    container.style.background = 'transparent'
    container.style.display = 'flex'
    container.style.alignItems = 'center'
    container.style.justifyContent = 'center'
    container.style.overflow = 'hidden'
    canvas.style.position = 'absolute'
    canvas.style.inset = '0'
    canvas.style.width = '100%'
    canvas.style.height = '100%'
    canvas.style.pointerEvents = 'none'
    canvas.style.zIndex = '2'

    // Size canvas to match container at device pixel ratio
    canvas.width = width * dpr
    canvas.height = height * dpr
    const ctx2d = canvas.getContext('2d')
    if (!ctx2d) throw new Error('Failed to create canvas 2D context for face')
    ctx2d.scale(dpr, dpr)

    return {
        canvas,
        circular,
        container,
        ctx: ctx2d,
        dpr,
        height,
        scale,
        width,
    }
}

// ── Font Loading ────────────────────────────────────────────────────────

/** Google Fonts CSS URL for a given family list. */
function googleFontsUrl(families: Iterable<string>): string {
    const query = [...families]
        .map((family) => `family=${encodeURIComponent(family).replace(/%20/g, '+')}:wght@400;500;600;700`)
        .join('&')
    return `https://fonts.googleapis.com/css2?${query}&display=swap`
}

/**
 * Load fonts used by combobox controls that look like font pickers.
 * Injects a Google Fonts stylesheet and waits for the default font to load.
 */
async function loadFaceFonts(controls: ResolvedFaceControl[]): Promise<void> {
    const fontControls = controls.filter(
        (c) =>
            c.spec.__type === 'combobox' && (c.spec.meta.values as string[] | undefined)?.some((v) => v.includes(' ')),
    )
    if (fontControls.length === 0) return

    const families = new Set<string>()
    for (const ctrl of fontControls) {
        const values = ctrl.spec.meta.values as string[] | undefined
        if (values) {
            for (const v of values) families.add(v)
        }
    }

    if (families.size === 0) return

    // Inject Google Fonts stylesheet
    const link = document.createElement('link')
    link.rel = 'stylesheet'
    link.href = googleFontsUrl(families)
    document.head.appendChild(link)

    // Wait for the default font of each control to be ready
    if (typeof document.fonts?.load === 'function') {
        const defaultFamilies = fontControls.map((c) => String(c.spec.defaultValue))
        await Promise.allSettled(defaultFamilies.map((f) => document.fonts.load(`16px "${f}"`)))
    }
}

function startFaceLoop(ctx: FaceContext, setupFn: FaceSetupFn, resolvedControls: ResolvedFaceControl[]): void {
    const updateFn = setupFn(ctx)
    const sensorAccessor = buildSensorAccessor()

    function tick(timestamp: number): void {
        const time = timestamp / 1000

        // Clear the canvas overlay each frame
        ctx.ctx.clearRect(0, 0, ctx.width, ctx.height)

        // Read current control values (may have been updated by the daemon)
        const controls = resolveControlValues(resolvedControls)

        // Call the face's update function
        updateFn(time, controls, sensorAccessor)

        requestAnimationFrame(tick)
    }

    requestAnimationFrame(tick)
}

// ── Public API ──────────────────────────────────────────────────────────

/**
 * Define a display face.
 *
 * ```typescript
 * export default face('My Face', controls, options, setupFn)
 * ```
 *
 * The setup function receives a `FaceContext` with a DOM container and
 * canvas overlay. It returns an update function called every frame with
 * the current time, resolved controls, and a sensor accessor.
 */
export function face(name: string, controls: ControlMap, options: FaceOptions, setupFn: FaceSetupFn): void {
    const resolved = resolveFaceControls(controls)
    const designBasis = options.designBasis ?? { height: 480, width: 480 }
    const circular = options.circular ?? false

    // Build-time metadata extraction — bail before any DOM access
    if (typeof globalThis !== 'undefined' && (globalThis as Record<string, unknown>).__HYPERCOLOR_METADATA_ONLY__) {
        storeFaceMetadata({
            author: options.author,
            circular,
            controls,
            description: options.description,
            designBasis,
            name,
            presets: options.presets,
            resolvedControls: resolved,
            type: 'face',
        })
        return
    }

    // Runtime initialization
    async function init(): Promise<void> {
        const container = document.getElementById('faceContainer') as HTMLDivElement | null
        const canvas = document.getElementById('faceCanvas') as HTMLCanvasElement | null

        if (!container || !canvas) {
            console.error('[face] Missing #faceContainer or #faceCanvas in DOM')
            return
        }

        document.documentElement.style.background = 'transparent'
        document.body.style.background = 'transparent'

        // Apply circular mask if needed
        if (circular) {
            container.style.clipPath = 'circle(50%)'
        }

        // Load fonts before first render so text doesn't flash fallbacks
        await loadFaceFonts(resolved)

        const ctx = createFaceContext(container, canvas, designBasis, circular)
        startFaceLoop(ctx, setupFn, resolved)
    }

    let started = false
    const start = () => {
        if (started) return
        const container = document.getElementById('faceContainer')
        const canvas = document.getElementById('faceCanvas')
        if (!container || !canvas) return
        started = true
        void init()
    }

    // Inline face bundles are emitted after the container markup, so try to
    // start immediately. Some headless embedders lag or skip DOMContentLoaded.
    start()
    if (!started) {
        if (document.readyState === 'complete' || document.readyState === 'interactive') {
            start()
        } else {
            window.addEventListener('DOMContentLoaded', start, { once: true })
        }
    }
}
