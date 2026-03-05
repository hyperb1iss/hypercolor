import 'reflect-metadata'
import {
    ComboboxControl,
    Effect,
    NumberControl,
    WebGLEffect,
    getControlValue,
    initializeEffect,
    normalizeSpeed,
} from '@hypercolor/sdk'

import fragmentShader from './fragment.glsl'

interface CascadeControls {
    speed: number
    intensity: number
    smoothing: number
    barWidth: number
    palette: number
    glow: number
}

const PALETTES = ['SilkCircuit', 'Aurora', 'Cyberpunk', 'Fire', 'Sunset', 'Ice']

@Effect({
    name: 'Frequency Cascade',
    description: 'Audio-reactive waterfall spectrogram with smooth band interpolation',
    author: 'Hypercolor',
    audioReactive: true,
})
class FrequencyCascade extends WebGLEffect<CascadeControls> {
    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 5, tooltip: 'Scroll speed' })
    speed!: number

    @NumberControl({ label: 'Intensity', min: 0, max: 100, default: 75, tooltip: 'Brightness' })
    intensity!: number

    @NumberControl({ label: 'Smoothing', min: 0, max: 100, default: 50, tooltip: 'Temporal smoothing' })
    smoothing!: number

    @NumberControl({ label: 'Bar Width', min: 0, max: 100, default: 40, tooltip: 'Frequency bar width' })
    barWidth!: number

    @ComboboxControl({
        label: 'Palette',
        values: PALETTES,
        default: 'SilkCircuit',
        tooltip: 'Color palette',
    })
    palette!: string

    @NumberControl({ label: 'Glow', min: 0, max: 100, default: 40, tooltip: 'Bloom intensity' })
    glow!: number

    constructor() {
        super({ fragmentShader, audioReactive: true })
    }

    protected initializeControls(): void {
        this.speed = getControlValue('speed', 5)
        this.intensity = getControlValue('intensity', 75)
        this.smoothing = getControlValue('smoothing', 50)
        this.barWidth = getControlValue('barWidth', 40)
        this.palette = getControlValue('palette', 'SilkCircuit')
        this.glow = getControlValue('glow', 40)
    }

    protected getControlValues(): CascadeControls {
        return {
            speed: normalizeSpeed(getControlValue('speed', 5)),
            intensity: getControlValue('intensity', 75),
            smoothing: getControlValue('smoothing', 50),
            barWidth: getControlValue('barWidth', 40),
            palette: PALETTES.indexOf(getControlValue('palette', 'SilkCircuit')),
            glow: getControlValue('glow', 40),
        }
    }

    protected createUniforms(): void {
        this.registerUniform('iSpeed', 1.0)
        this.registerUniform('iIntensity', 75)
        this.registerUniform('iSmoothing', 50)
        this.registerUniform('iBarWidth', 40)
        this.registerUniform('iPalette', 0)
        this.registerUniform('iGlow', 40)
    }

    protected updateUniforms(controls: CascadeControls): void {
        this.setUniform('iSpeed', controls.speed)
        this.setUniform('iIntensity', controls.intensity)
        this.setUniform('iSmoothing', controls.smoothing)
        this.setUniform('iBarWidth', controls.barWidth)
        this.setUniform('iPalette', controls.palette)
        this.setUniform('iGlow', controls.glow)
    }
}

const effect = new FrequencyCascade()
initializeEffect(() => effect.initialize(), { instance: effect })
