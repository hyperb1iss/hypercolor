/**
 * canvas() — declarative canvas effect API.
 *
 * Write a draw function, declare controls, ship.
 *
 * The canvas is sized by the Hypercolor daemon (640x480 by default, user-tunable).
 * Always read `ctx.canvas.width` / `ctx.canvas.height` inside your draw function —
 * never hardcode dimensions. For effects authored against a fixed design grid,
 * pass [`CanvasFnOptions.designBasis`] and call [`scaleContext`] per frame.
 *
 * @example
 * ```typescript
 * import { canvas, scaleContext } from '@hypercolor/sdk'
 *
 * // Pure-adaptive: read canvas size directly
 * export default canvas('Particles', {
 *     speed: [1, 10, 5],
 *     palette: ['SilkCircuit', 'Fire', 'Aurora'],
 * }, (ctx, time, { palette }) => {
 *     ctx.clearRect(0, 0, ctx.canvas.width, ctx.canvas.height)
 *     ctx.fillStyle = palette(0.5)
 *     ctx.fillRect(0, 0, ctx.canvas.width, ctx.canvas.height)
 * })
 *
 * // Fixed design basis: author in 320x200, scale to whatever the daemon renders at
 * export default canvas('Aurora', controls, (ctx, time, controls) => {
 *     const s = scaleContext(ctx.canvas, { width: 320, height: 200 })
 *     ctx.fillRect(s.dx(20), s.dy(30), s.dw(60), s.dh(40))
 * }, { designBasis: { width: 320, height: 200 } })
 *
 * // Stateful (factory returns draw function)
 * export default canvas('Fireflies', controls, () => {
 *     const state = initParticles()
 *     return (ctx, time, controls) => { ... }
 * })
 * ```
 */

import { comboboxValueToIndex, getControlValue, normalizePercentage, normalizeSpeed } from '../controls/helpers'
import type { ControlMap } from '../controls/infer'
import { inferControl } from '../controls/infer'
import { deriveLabel, hasMagicTransform, resolveControlNames } from '../controls/names'
import { isControlSpec } from '../controls/specs'
import { initializeEffect } from '../init'
import type { DesignBasis } from '../math/scale'
import type { PaletteFn } from '../palette'
import { createPaletteFn } from '../palette'
import { CanvasEffect } from './canvas-effect'
import type { PresetDef } from './effect-fn'

// ── Types ────────────────────────────────────────────────────────────────

export type DrawFn = (ctx: CanvasRenderingContext2D, time: number, controls: Record<string, unknown>) => void

export type FactoryFn = () => DrawFn

export interface CanvasFnOptions {
    description?: string
    author?: string
    /**
     * Coordinate system this effect is authored in. Pass to keep the effect
     * pixel-identical at its original design resolution while automatically
     * scaling to the daemon's configured canvas size. Omit for pure-adaptive
     * effects that use `ctx.canvas.width/height` directly.
     */
    designBasis?: DesignBasis
    /** Effect-defined presets — named control snapshots bundled with the effect. */
    presets?: PresetDef[]
}

/** Resolved control entry used at runtime. */
interface ResolvedCanvasControl {
    key: string
    spec: import('../controls/specs').ControlSpec
    normalize: 'speed' | 'percentage' | 'none'
    isMagicTransform: boolean
    isPaletteFunction: boolean
    values?: string[]
}

// ── Control Resolution ───────────────────────────────────────────────────

function resolveCanvasControls(controls: ControlMap): ResolvedCanvasControl[] {
    const resolved: ResolvedCanvasControl[] = []

    for (const [key, value] of Object.entries(controls)) {
        const isExplicitSpec = isControlSpec(value)
        const spec = isExplicitSpec ? value : inferControl(key, value, deriveLabel(key))

        const names = resolveControlNames(key, spec)
        const isCombo = spec.__type === 'combobox'
        const values = spec.meta.values as string[] | undefined

        resolved.push({
            isMagicTransform: hasMagicTransform(key) && isCombo && !isExplicitSpec,
            // Preserve the legacy shorthand `palette: ['A', 'B']` -> palette function
            // while letting explicit `combo('Palette', ...)` controls stay string-valued.
            isPaletteFunction: key === 'palette' && isCombo && !isExplicitSpec,
            key,
            normalize: names.normalize,
            spec,
            values,
        })
    }

    return resolved
}

/** Resolve canvas control values — palette becomes a function. */
function resolveValues(
    controls: ResolvedCanvasControl[],
    paletteFnCache: Map<string, PaletteFn>,
): Record<string, unknown> {
    const result: Record<string, unknown> = {}

    for (const ctrl of controls) {
        let val = getControlValue(ctrl.key, ctrl.spec.defaultValue)

        if (typeof val === 'number') {
            if (ctrl.normalize === 'speed') {
                val = normalizeSpeed(val)
            } else if (ctrl.normalize === 'percentage') {
                val = normalizePercentage(val)
            }
        }

        // Palette → function (canvas-specific behavior)
        if (ctrl.isPaletteFunction && ctrl.values) {
            const paletteName = typeof val === 'string' ? val : (ctrl.values[0] ?? 'SilkCircuit')
            let fn = paletteFnCache.get(paletteName)
            if (!fn) {
                fn = createPaletteFn(paletteName)
                paletteFnCache.set(paletteName, fn)
            }
            result[ctrl.key] = fn
            continue
        }

        // Other combobox → keep as string (no index conversion for canvas)
        if (ctrl.isMagicTransform && !ctrl.isPaletteFunction && ctrl.values) {
            val = comboboxValueToIndex(val as string | number, ctrl.values, 0)
        }

        result[ctrl.key] = val
    }

    return result
}

export const __testing = {
    resolveCanvasControls,
    resolveValues,
}

// ── Generated Canvas Effect ──────────────────────────────────────────────

class GeneratedCanvasEffect extends CanvasEffect<Record<string, unknown>> {
    private resolvedControls: ResolvedCanvasControl[]
    private drawFn: DrawFn | null = null
    private factoryFn: FactoryFn | null = null
    private directDrawFn: DrawFn | null = null
    private currentControls: Record<string, unknown> = {}
    private paletteFnCache = new Map<string, PaletteFn>()

    constructor(
        name: string,
        resolvedControls: ResolvedCanvasControl[],
        renderFn: DrawFn | FactoryFn,
        isFactory: boolean,
        options: CanvasFnOptions,
    ) {
        super({
            designBasis: options.designBasis,
            id: name.toLowerCase().replace(/\s+/g, '-'),
            name,
        })
        this.resolvedControls = resolvedControls
        if (isFactory) {
            this.factoryFn = renderFn as FactoryFn
        } else {
            this.directDrawFn = renderFn as DrawFn
        }
    }

    protected async initializeRenderer(): Promise<void> {
        await super.initializeRenderer()

        // If factory, run setup to get the draw function
        if (this.factoryFn) {
            this.drawFn = this.factoryFn()
        } else {
            this.drawFn = this.directDrawFn
        }
    }

    protected initializeControls(): void {
        this.currentControls = resolveValues(this.resolvedControls, this.paletteFnCache)
    }

    protected getControlValues(): Record<string, unknown> {
        return resolveValues(this.resolvedControls, this.paletteFnCache)
    }

    protected applyControls(controls: Record<string, unknown>): void {
        this.currentControls = controls
    }

    protected draw(time: number, _deltaTime: number): void {
        if (!this.drawFn || !this.ctx) return
        this.drawFn(this.ctx, time, this.currentControls)
    }

    // Override clearCanvas to not auto-clear — let the draw function decide
    protected clearCanvas(): void {
        // Canvas effects manage their own clearing for trail/persistence effects
    }
}

// ── Metadata ─────────────────────────────────────────────────────────────

interface CanvasDef {
    type: 'canvas'
    name: string
    controls: ControlMap
    resolvedControls: ResolvedCanvasControl[]
    description?: string
    author?: string
    designBasis?: DesignBasis
    presets?: PresetDef[]
}

function storeCanvasMetadata(def: CanvasDef): void {
    const g = globalThis as Record<string, unknown>
    const defs = (g.__hypercolorEffectDefs__ as unknown[]) ?? []
    defs.push(def)
    g.__hypercolorEffectDefs__ = defs
}

// ── Public API ───────────────────────────────────────────────────────────

/**
 * Define a canvas effect.
 *
 * Detection: if the 3rd argument has `.length >= 1`, it's a stateless draw function.
 * If `.length === 0`, it's a stateful factory (runs once, returns draw function).
 * Use `canvas.stateful()` to bypass arity detection.
 */
export function canvas(
    name: string,
    controls: ControlMap,
    renderFn: DrawFn | FactoryFn,
    options?: CanvasFnOptions,
): void {
    const resolved = resolveCanvasControls(controls)
    const opts = options ?? {}
    const isFactory = renderFn.length === 0

    if (typeof globalThis !== 'undefined' && (globalThis as Record<string, unknown>).__HYPERCOLOR_METADATA_ONLY__) {
        storeCanvasMetadata({
            author: opts.author,
            controls,
            description: opts.description,
            designBasis: opts.designBasis,
            name,
            presets: opts.presets,
            resolvedControls: resolved,
            type: 'canvas',
        })
        return
    }

    const fx = new GeneratedCanvasEffect(name, resolved, renderFn, isFactory, opts)
    initializeEffect(() => fx.initialize(), { instance: fx })
}

/** Explicit stateful factory — bypasses arity detection. */
canvas.stateful = function stateful(
    name: string,
    controls: ControlMap,
    factory: FactoryFn,
    options?: CanvasFnOptions,
): void {
    const resolved = resolveCanvasControls(controls)
    const opts = options ?? {}

    if (typeof globalThis !== 'undefined' && (globalThis as Record<string, unknown>).__HYPERCOLOR_METADATA_ONLY__) {
        storeCanvasMetadata({
            author: opts.author,
            controls,
            description: opts.description,
            designBasis: opts.designBasis,
            name,
            presets: opts.presets,
            resolvedControls: resolved,
            type: 'canvas',
        })
        return
    }

    const fx = new GeneratedCanvasEffect(name, resolved, factory, true, opts)
    initializeEffect(() => fx.initialize(), { instance: fx })
}
