/**
 * BaseEffect — core lifecycle for all Hypercolor effects.
 *
 * Provides canvas management, animation loop, FPS capping,
 * and the control update contract.
 */

import { createDebugLogger } from '../utils/debug'

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

    private fpsCapLastFrameTime = 0
    private lastControlPollTime = Number.NEGATIVE_INFINITY

    constructor(config: EffectConfig) {
        this.id = config.id
        this.name = config.name
        this.debug = createDebugLogger(this.name, config.debug ?? false)
        this.canvasWidth = config.canvasWidth ?? 320
        this.canvasHeight = config.canvasHeight ?? 200
        this.debug('info', 'Effect created', { id: this.id })
    }

    /** Initialize the effect — canvas, renderer, controls, animation. */
    public async initialize(): Promise<void> {
        this.debug('info', 'Initializing...')

        try {
            this.canvas = document.getElementById('exCanvas') as HTMLCanvasElement
            if (!this.canvas) {
                this.canvas = document.createElement('canvas')
                this.canvas.id = 'exCanvas'
                this.canvas.width = this.canvasWidth
                this.canvas.height = this.canvasHeight
                document.body.appendChild(this.canvas)
            }

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
        this.animationId = requestAnimationFrame(this.animationFrame.bind(this))
        window.currentAnimationFrame = this.animationId
        window.effectInstance = this
    }

    protected animationFrame(timestamp: number): void {
        if (this.animationId === null) return

        this.animationId = requestAnimationFrame(this.animationFrame.bind(this))
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

    protected onCanvasResize(_width: number, _height: number): void {}

    private syncCanvasSizeFromEngine(): void {
        if (!this.canvas) return

        const engine = (window as { engine?: { width?: unknown; height?: unknown } }).engine
        const width = typeof engine?.width === 'number' && Number.isFinite(engine.width) ? Math.max(1, Math.round(engine.width)) : null
        const height = typeof engine?.height === 'number' && Number.isFinite(engine.height) ? Math.max(1, Math.round(engine.height)) : null

        if (width == null || height == null) return
        if (this.canvas.width === width && this.canvas.height === height) return

        this.canvas.width = width
        this.canvas.height = height
        this.canvasWidth = width
        this.canvasHeight = height
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
