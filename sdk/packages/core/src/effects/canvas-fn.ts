/**
 * canvas() — declarative canvas effect API.
 *
 * Write a draw function, declare controls, ship.
 *
 * @example
 * ```typescript
 * // Stateless (called every frame)
 * export default canvas('Particles', {
 *     speed: [1, 10, 5],
 *     palette: ['SilkCircuit', 'Fire', 'Aurora'],
 * }, (ctx, time, { speed, palette }) => {
 *     ctx.clearRect(0, 0, 320, 200)
 *     ctx.fillStyle = palette(0.5)
 *     ctx.fillRect(0, 0, 320, 200)
 * })
 *
 * // Stateful (factory returns draw function)
 * export default canvas('Fireflies', controls, () => {
 *     const state = initParticles()
 *     return (ctx, time, controls) => { ... }
 * })
 * ```
 */

import type { ControlMap } from '../controls/infer'
import { inferControl } from '../controls/infer'
import { deriveLabel, hasMagicTransform, resolveControlNames } from '../controls/names'
import { isControlSpec } from '../controls/specs'
import { comboboxValueToIndex, getControlValue, normalizeSpeed } from '../controls/helpers'
import { createPaletteFn } from '../palette'
import type { PaletteFn } from '../palette'
import { initializeEffect } from '../init'
import { CanvasEffect } from './canvas-effect'

// ── Types ────────────────────────────────────────────────────────────────

export type DrawFn = (
    ctx: CanvasRenderingContext2D,
    time: number,
    controls: Record<string, unknown>,
) => void

export type FactoryFn = () => DrawFn

export interface CanvasFnOptions {
    description?: string
    author?: string
    width?: number
    height?: number
}

/** Resolved control entry used at runtime. */
interface ResolvedCanvasControl {
    key: string
    spec: import('../controls/specs').ControlSpec
    normalize: 'speed' | 'percentage' | 'none'
    isMagicTransform: boolean
    isPalette: boolean
    values?: string[]
}

// ── Control Resolution ───────────────────────────────────────────────────

function resolveCanvasControls(controls: ControlMap): ResolvedCanvasControl[] {
    const resolved: ResolvedCanvasControl[] = []

    for (const [key, value] of Object.entries(controls)) {
        const spec = isControlSpec(value)
            ? value
            : inferControl(key, value, deriveLabel(key))

        const names = resolveControlNames(key, spec)
        const isCombo = spec.__type === 'combobox'
        const values = spec.meta.values as string[] | undefined

        resolved.push({
            key,
            spec,
            normalize: names.normalize,
            isMagicTransform: hasMagicTransform(key) && isCombo,
            isPalette: key === 'palette' && isCombo,
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

        if (ctrl.normalize === 'speed' && typeof val === 'number') {
            val = normalizeSpeed(val)
        }

        // Palette → function (canvas-specific behavior)
        if (ctrl.isPalette && ctrl.values) {
            const paletteName = typeof val === 'string'
                ? val
                : (ctrl.values[0] ?? 'SilkCircuit')
            let fn = paletteFnCache.get(paletteName)
            if (!fn) {
                fn = createPaletteFn(paletteName)
                paletteFnCache.set(paletteName, fn)
            }
            result[ctrl.key] = fn
            continue
        }

        // Other combobox → keep as string (no index conversion for canvas)
        if (ctrl.isMagicTransform && !ctrl.isPalette && ctrl.values) {
            val = comboboxValueToIndex(val as string | number, ctrl.values, 0)
        }

        result[ctrl.key] = val
    }

    return result
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
            id: name.toLowerCase().replace(/\s+/g, '-'),
            name,
            canvasWidth: options.width,
            canvasHeight: options.height,
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
    width?: number
    height?: number
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

    if (
        typeof globalThis !== 'undefined' &&
        (globalThis as Record<string, unknown>).__HYPERCOLOR_METADATA_ONLY__
    ) {
        storeCanvasMetadata({
            type: 'canvas',
            name,
            controls,
            resolvedControls: resolved,
            description: opts.description,
            author: opts.author,
            width: opts.width,
            height: opts.height,
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

    if (
        typeof globalThis !== 'undefined' &&
        (globalThis as Record<string, unknown>).__HYPERCOLOR_METADATA_ONLY__
    ) {
        storeCanvasMetadata({
            type: 'canvas',
            name,
            controls,
            resolvedControls: resolved,
            description: opts.description,
            author: opts.author,
            width: opts.width,
            height: opts.height,
        })
        return
    }

    const fx = new GeneratedCanvasEffect(name, resolved, factory, true, opts)
    initializeEffect(() => fx.initialize(), { instance: fx })
}
