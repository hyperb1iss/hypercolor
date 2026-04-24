/**
 * BaseEffect — core lifecycle for all Hypercolor effects.
 *
 * Provides canvas management, animation loop, FPS capping,
 * and the control update contract.
 *
 * Canvas dimensions are adaptive: on every frame the base effect reads
 * `window.engine.width/height` (injected by the Hypercolor daemon) and
 * resizes the backing canvas to match. Effects that want to author in a
 * fixed coordinate space should set [`designBasis`] and use [`scaleContext`]
 * / [`this.scaleContext()`] to translate design coords to live pixels.
 * Nothing in the SDK treats any specific resolution as canonical.
 */

import { type DesignBasis, type ScaleContext, scaleContext } from '../math/scale'
import { createDebugLogger } from '../utils/debug'

/**
 * Initial placeholder canvas dimensions used during effect construction, before
 * the animation loop runs and [`syncCanvasSizeFromEngine`] replaces them with
 * the live engine dimensions (or the browser window in standalone preview).
 *
 * Intentionally private and modest so that no effect can anchor itself to
 * these numbers — the moment the first frame renders, they are gone.
 */
const INITIAL_PLACEHOLDER_WIDTH = 1
const INITIAL_PLACEHOLDER_HEIGHT = 1
const FPS_CAP_EPSILON_MS = 0.5

export interface EffectConfig {
    id: string
    name: string
    debug?: boolean
    /**
     * Coordinate system this effect is authored in. When set, [`scaleContext`]
     * uses it to translate design coords to live canvas pixels. Omit for
     * pure-adaptive effects that use `canvas.width/height` directly.
     */
    designBasis?: DesignBasis
}

export abstract class BaseEffect<T> {
    protected id: string
    protected name: string
    protected debug: ReturnType<typeof createDebugLogger>
    protected animationId: number | null = null
    protected canvasWidth: number
    protected canvasHeight: number
    protected canvas: HTMLCanvasElement | null = null
    protected stage: HTMLDivElement | null = null
    /**
     * Design-space coordinate system for this effect. Subclasses may set this
     * to author against a fixed grid (e.g. `{ width: 320, height: 200 }`);
     * leave undefined for pure-adaptive effects.
     */
    protected designBasis?: DesignBasis

    private fpsCapLastFrameTime = 0
    private lastControlPollTime = Number.NEGATIVE_INFINITY
    private readonly animationFrameCallback = (timestamp: number): void => {
        this.animationFrame(timestamp)
    }

    constructor(config: EffectConfig) {
        this.id = config.id
        this.name = config.name
        this.debug = createDebugLogger(this.name, config.debug ?? false)
        this.designBasis = config.designBasis
        this.canvasWidth = INITIAL_PLACEHOLDER_WIDTH
        this.canvasHeight = INITIAL_PLACEHOLDER_HEIGHT
        this.debug('info', 'Effect created', { id: this.id })
    }

    /**
     * Build a [`ScaleContext`] snapshot for the current frame, bound to the
     * live canvas size and this effect's [`designBasis`] (if any). Call this
     * inside your draw method — it's a handful of arithmetic ops, so per-frame
     * construction is the idiomatic pattern.
     */
    protected scaleContext(): ScaleContext {
        return scaleContext(this.canvas ?? { height: this.canvasHeight, width: this.canvasWidth }, this.designBasis)
    }

    /** Initialize the effect — canvas, renderer, controls, animation. */
    public async initialize(): Promise<void> {
        this.debug('info', 'Initializing...')

        try {
            this.stage = this.ensureStage()
            this.canvas = document.getElementById('exCanvas') as HTMLCanvasElement
            if (!this.canvas) {
                this.canvas = document.createElement('canvas')
                this.canvas.id = 'exCanvas'
                this.canvas.width = this.canvasWidth
                this.canvas.height = this.canvasHeight
                const canvasParent = this.stage ?? document.body
                canvasParent.appendChild(this.canvas)
            } else if (this.stage && this.canvas.parentElement !== this.stage) {
                this.stage.appendChild(this.canvas)
            }

            this.syncSurfacePresentation(this.canvasWidth, this.canvasHeight)
            this.syncCanvasSizeFromEngine()
            await this.initializeRenderer()
            this.initializeControls()
            window.update = this.update.bind(this)
            this.startAnimation()
            this.debug('success', 'Initialized')
        } catch (error) {
            this.debug('error', 'Initialization failed', error)
            this.handleInitError(error)
        }
    }

    protected startAnimation(): void {
        this.animationId = requestAnimationFrame(this.animationFrameCallback)
        window.currentAnimationFrame = this.animationId
        window.effectInstance = this
    }

    protected animationFrame(timestamp: number): void {
        if (this.animationId === null) return

        this.animationId = requestAnimationFrame(this.animationFrameCallback)
        window.currentAnimationFrame = this.animationId

        // FPS cap support
        const fpsCap = (window as { __hypercolorFpsCap?: number }).__hypercolorFpsCap ?? 0
        if (fpsCap > 0) {
            const nextFrameTime = nextFpsCapFrameTime(timestamp, this.fpsCapLastFrameTime, fpsCap)
            if (nextFrameTime === null) return
            this.fpsCapLastFrameTime = nextFrameTime
        }

        this.syncCanvasSizeFromEngine()
        this.render(timestamp / 1000)
        this.onFrame(timestamp / 1000)
    }

    public stop(): void {
        if (this.animationId !== null) {
            cancelAnimationFrame(this.animationId)
            this.animationId = null
            window.currentAnimationFrame = undefined
        }
    }

    protected onFrame(time: number): void {
        // Poll controls on a fixed cadence without frame-rate-dependent bursts.
        if (time - this.lastControlPollTime >= 0.1) {
            this.lastControlPollTime = time
            this.update()
        }
    }

    public update(force = false): void {
        const controls = this.getControlValues()
        this.updateParameters(controls)
        if (force) this.debug('debug', 'Controls updated', controls)
    }

    protected getStageElement(): HTMLDivElement | null {
        return this.stage
    }

    protected onCanvasResize(_width: number, _height: number): void {}

    private ensureStage(): HTMLDivElement | null {
        if (typeof document === 'undefined') return null

        let stage = document.getElementById('exStage') as HTMLDivElement | null
        if (!stage) {
            stage = document.createElement('div')
            stage.id = 'exStage'
            document.body.appendChild(stage)
        }

        stage.style.position = 'relative'
        stage.style.overflow = 'hidden'
        stage.style.background = '#000'
        return stage
    }

    private syncSurfacePresentation(width: number, height: number): void {
        if (typeof document !== 'undefined') {
            document.documentElement.style.margin = '0'
            document.documentElement.style.overflow = 'hidden'
            document.body.style.margin = '0'
            document.body.style.overflow = 'hidden'
            document.body.style.background = '#000'
        }

        if (this.stage) {
            this.stage.style.width = `${width}px`
            this.stage.style.height = `${height}px`
        }

        if (this.canvas) {
            this.canvas.style.display = 'block'
            this.canvas.style.width = '100%'
            this.canvas.style.height = '100%'
        }
    }

    private syncCanvasSizeFromEngine(): void {
        if (!this.canvas) return

        const engine = (window as { engine?: { width?: unknown; height?: unknown } }).engine
        let width: number | null =
            typeof engine?.width === 'number' && Number.isFinite(engine.width)
                ? Math.max(1, Math.round(engine.width))
                : null
        let height: number | null =
            typeof engine?.height === 'number' && Number.isFinite(engine.height)
                ? Math.max(1, Math.round(engine.height))
                : null

        // Standalone preview: fill the viewport when no engine is driving dimensions
        if (width == null || height == null) {
            if (typeof window !== 'undefined' && window.innerWidth > 0 && window.innerHeight > 0) {
                width = window.innerWidth
                height = window.innerHeight
            } else {
                return
            }
        }

        if (this.canvas.width === width && this.canvas.height === height) return

        this.canvas.width = width
        this.canvas.height = height
        this.canvasWidth = width
        this.canvasHeight = height
        this.syncSurfacePresentation(width, height)
        this.onCanvasResize(width, height)
    }

    protected handleInitError(error: unknown): void {
        console.error(`Failed to initialize ${this.name}:`, error)
        try {
            if (this.canvas) {
                const ctx = this.canvas.getContext('2d')
                if (ctx) {
                    ctx.fillStyle = 'black'
                    ctx.fillRect(0, 0, this.canvas.width, this.canvas.height)
                    ctx.fillStyle = '#ff6363'
                    ctx.font = '14px monospace'
                    ctx.fillText(`Error: ${this.name}`, 20, 50)
                    ctx.fillText(String(error).substring(0, 40), 20, 70)
                }
            }
        } catch {
            // Swallow render errors during error display
        }
    }

    protected abstract initializeRenderer(): Promise<void>
    protected abstract render(time: number): void
    protected abstract initializeControls(): void
    protected abstract getControlValues(): T
    protected abstract updateParameters(controls: T): void
}

function nextFpsCapFrameTime(timestamp: number, lastFrameTime: number, fpsCap: number): number | null {
    const frameInterval = 1000 / fpsCap
    const nextDue = lastFrameTime + frameInterval
    if (timestamp + FPS_CAP_EPSILON_MS < nextDue) return null
    if (timestamp < nextDue) return nextDue

    const elapsedFrames = Math.max(1, Math.floor((timestamp - lastFrameTime) / frameInterval))
    return lastFrameTime + elapsedFrames * frameInterval
}

export const __testing = {
    nextFpsCapFrameTime,
}
