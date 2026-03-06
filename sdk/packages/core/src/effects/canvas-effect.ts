/**
 * CanvasEffect — base class for Canvas 2D effects.
 */

import { BaseEffect, EffectConfig } from './base-effect'

export interface CanvasEffectConfig extends EffectConfig {
    backgroundColor?: string
}

export abstract class CanvasEffect<T> extends BaseEffect<T> {
    protected ctx: CanvasRenderingContext2D | null = null
    protected backgroundColor: string
    protected lastFrameTime = 0
    protected deltaTime = 0

    constructor(config: CanvasEffectConfig) {
        super(config)
        this.backgroundColor = config.backgroundColor || 'black'
    }

    protected async initializeRenderer(): Promise<void> {
        if (!this.canvas) throw new Error('Canvas not available')

        this.ctx = this.canvas.getContext('2d')
        if (!this.ctx) throw new Error('Could not get 2D context')

        this.ctx.imageSmoothingEnabled = true
        this.clearCanvas()
        await this.loadResources()
    }

    /** Override to load images, fonts, etc. */
    protected async loadResources(): Promise<void> {}

    protected clearCanvas(): void {
        if (!this.ctx || !this.canvas) return
        this.ctx.fillStyle = this.backgroundColor
        this.ctx.fillRect(0, 0, this.canvas.width, this.canvas.height)
    }

    protected render(time: number): void {
        if (!this.ctx || !this.canvas) return

        this.deltaTime = this.lastFrameTime === 0 ? 0 : time - this.lastFrameTime
        this.lastFrameTime = time

        this.clearCanvas()
        this.draw(time, this.deltaTime)
    }

    protected updateParameters(controls: T): void {
        this.applyControls(controls)
    }

    protected onCanvasResize(): void {
        if (!this.ctx) return
        this.ctx.imageSmoothingEnabled = true
    }

    /** Draw the effect. Called every frame after canvas is cleared. */
    protected abstract draw(time: number, deltaTime: number): void

    /** Apply control values. Called when controls change. */
    protected abstract applyControls(controls: T): void
}
