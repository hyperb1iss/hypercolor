import 'reflect-metadata'
import {
    ColorControl,
    ComboboxControl,
    Effect,
    NumberControl,
    WebGLEffect,
    comboboxValueToIndex,
    getControlValue,
    initializeEffect,
    normalizeSpeed,
} from '@hypercolor/sdk'

import fragmentShader from './fragment.glsl'

interface DeepCurrentControls {
    leftColor: [number, number, number]
    rightColor: [number, number, number]
    speed: number
    rippleIntensity: number
    particleAmount: number
    blend: number
    splitMode: number
}

const DEFAULT_LEFT_COLOR = '#ff4fb4'
const DEFAULT_RIGHT_COLOR = '#ffe36a'
const SPLIT_MODES = ['Vertical', 'Horizontal', 'Diagonal']

@Effect({
    name: 'Deep Current',
    description: 'Pink Lemonade split-field with crisp ripples and floating particles',
    author: 'Hypercolor',
    audioReactive: false,
})
class DeepCurrent extends WebGLEffect<DeepCurrentControls> {
    @ColorControl({ label: 'Left Color', default: DEFAULT_LEFT_COLOR, tooltip: 'Primary color on the left side' })
    leftColor!: string

    @ColorControl({ label: 'Right Color', default: DEFAULT_RIGHT_COLOR, tooltip: 'Primary color on the right side' })
    rightColor!: string

    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 4, tooltip: 'Animation speed' })
    speed!: number

    @NumberControl({ label: 'Ripple Intensity', min: 0, max: 100, default: 68, tooltip: 'Strength of ripple deformation' })
    rippleIntensity!: number

    @NumberControl({ label: 'Particle Amount', min: 0, max: 100, default: 56, tooltip: 'Count and brightness of floating particles' })
    particleAmount!: number

    @NumberControl({ label: 'Blend', min: 0, max: 100, default: 26, tooltip: 'Softness of the color split seam' })
    blend!: number

    @ComboboxControl({
        label: 'Split Mode',
        values: SPLIT_MODES,
        default: 'Vertical',
        tooltip: 'Direction of the split boundary',
    })
    splitMode!: string

    constructor() {
        super({ fragmentShader })
    }

    protected initializeControls(): void {
        this.leftColor = getControlValue('leftColor', DEFAULT_LEFT_COLOR)
        this.rightColor = getControlValue('rightColor', DEFAULT_RIGHT_COLOR)
        this.speed = getControlValue('speed', 4)
        this.rippleIntensity = getControlValue('rippleIntensity', 68)
        this.particleAmount = getControlValue('particleAmount', 56)
        this.blend = getControlValue('blend', 26)
        this.splitMode = getControlValue('splitMode', 'Vertical')
    }

    protected getControlValues(): DeepCurrentControls {
        return {
            leftColor: this.hexToVec3(getControlValue('leftColor', DEFAULT_LEFT_COLOR), DEFAULT_LEFT_COLOR),
            rightColor: this.hexToVec3(getControlValue('rightColor', DEFAULT_RIGHT_COLOR), DEFAULT_RIGHT_COLOR),
            speed: normalizeSpeed(getControlValue('speed', 4)),
            rippleIntensity: getControlValue('rippleIntensity', 68),
            particleAmount: getControlValue('particleAmount', 56),
            blend: getControlValue('blend', 26),
            splitMode: comboboxValueToIndex(getControlValue('splitMode', 'Vertical'), SPLIT_MODES, 0),
        }
    }

    protected createUniforms(): void {
        this.registerUniform('iLeftColor', this.hexToVec3(DEFAULT_LEFT_COLOR, DEFAULT_LEFT_COLOR))
        this.registerUniform('iRightColor', this.hexToVec3(DEFAULT_RIGHT_COLOR, DEFAULT_RIGHT_COLOR))
        this.registerUniform('iSpeed', 1.0)
        this.registerUniform('iRippleIntensity', 68)
        this.registerUniform('iParticleAmount', 56)
        this.registerUniform('iBlend', 26)
        this.registerUniform('iSplitMode', 0)
    }

    protected updateUniforms(controls: DeepCurrentControls): void {
        this.setUniform('iLeftColor', controls.leftColor)
        this.setUniform('iRightColor', controls.rightColor)
        this.setUniform('iSpeed', controls.speed)
        this.setUniform('iRippleIntensity', controls.rippleIntensity)
        this.setUniform('iParticleAmount', controls.particleAmount)
        this.setUniform('iBlend', controls.blend)
        this.setUniform('iSplitMode', controls.splitMode)
    }

    private hexToVec3(hex: string, fallback: string): [number, number, number] {
        const parseHex = (value: string): string | null => {
            const stripped = value.trim().replace(/^#/, '')
            if (/^[0-9a-fA-F]{6}$/.test(stripped)) return stripped
            if (/^[0-9a-fA-F]{3}$/.test(stripped)) {
                return stripped
                    .split('')
                    .map((digit) => `${digit}${digit}`)
                    .join('')
            }
            return null
        }

        const safeHex = parseHex(hex) ?? parseHex(fallback) ?? '000000'
        return [
            Number.parseInt(safeHex.slice(0, 2), 16) / 255,
            Number.parseInt(safeHex.slice(2, 4), 16) / 255,
            Number.parseInt(safeHex.slice(4, 6), 16) / 255,
        ]
    }
}

const effect = new DeepCurrent()
initializeEffect(() => effect.initialize(), { instance: effect })
