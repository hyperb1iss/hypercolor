/**
 * BaseEffect — core lifecycle for all Hypercolor effects.
 *
 * Provides canvas management, animation loop, FPS capping,
 * and the control update contract.
 */

import { createDebugLogger } from '../utils/debug'

/** Default canvas width used by the render pipeline (design resolution). */
export const DEFAULT_CANVAS_WIDTH = 320

/** Default canvas height used by the render pipeline (design resolution). */
export const DEFAULT_CANVAS_HEIGHT = 200

export interface EffectConfig {
    id: string
    name: string
    debug?: boolean
    canvasWidth?: number
    canvasHeight?: number
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

    private fpsCapLastFrameTime = 0
    private lastControlPollTime = Number.NEGATIVE_INFINITY
    private readonly animationFrameCallback = (timestamp: number): void => {
        this.animationFrame(timestamp)
    }

    constructor(config: EffectConfig) {
        this.id = config.id
        this.name = config.name
        this.debug = createDebugLogger(this.name, config.debug ?? false)
        this.canvasWidth = config.canvasWidth ?? DEFAULT_CANVAS_WIDTH
        this.canvasHeight = config.canvasHeight ?? DEFAULT_CANVAS_HEIGHT
        this.debug('info', 'Effect created', { id: this.id })
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
            const frameInterval = 1000 / fpsCap
            if (timestamp - this.fpsCapLastFrameTime < frameInterval) return
            this.fpsCapLastFrameTime = timestamp
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
