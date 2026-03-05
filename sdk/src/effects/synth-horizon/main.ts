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

interface SynthControls {
    speed: number
    gridDensity: number
    sunSize: number
    glow: number
    palette: number
}

const PALETTES = ['SilkCircuit', 'Synthwave', 'Cyberpunk', 'Fire', 'Aurora']

@Effect({
    name: 'Synth Horizon',
    description: 'Retrowave perspective grid with setting sun, audio-reactive horizon',
    author: 'Hypercolor',
    audioReactive: true,
})
class SynthHorizon extends WebGLEffect<SynthControls> {
    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 5, tooltip: 'Grid scroll speed' })
    speed!: number

    @NumberControl({ label: 'Grid', min: 10, max: 100, default: 50, tooltip: 'Grid density' })
    gridDensity!: number

    @NumberControl({ label: 'Sun', min: 10, max: 100, default: 60, tooltip: 'Sun size' })
    sunSize!: number

    @NumberControl({ label: 'Glow', min: 10, max: 100, default: 70, tooltip: 'Overall glow' })
    glow!: number

    @ComboboxControl({ label: 'Palette', values: PALETTES, default: 'Synthwave', tooltip: 'Color palette' })
    palette!: string

    constructor() {
        super({ fragmentShader, audioReactive: true })
    }

    protected initializeControls(): void {
        this.speed = getControlValue('speed', 5)
        this.gridDensity = getControlValue('gridDensity', 50)
        this.sunSize = getControlValue('sunSize', 60)
        this.glow = getControlValue('glow', 70)
        this.palette = getControlValue('palette', 'Synthwave')
    }

    protected getControlValues(): SynthControls {
        return {
            speed: normalizeSpeed(getControlValue('speed', 5)),
            gridDensity: getControlValue('gridDensity', 50),
            sunSize: getControlValue('sunSize', 60),
            glow: getControlValue('glow', 70),
            palette: PALETTES.indexOf(getControlValue('palette', 'Synthwave')),
        }
    }

    protected createUniforms(): void {
        this.registerUniform('iSpeed', 1.0)
        this.registerUniform('iGridDensity', 50)
        this.registerUniform('iSunSize', 60)
        this.registerUniform('iGlow', 70)
        this.registerUniform('iPalette', 1)
    }

    protected updateUniforms(c: SynthControls): void {
        this.setUniform('iSpeed', c.speed)
        this.setUniform('iGridDensity', c.gridDensity)
        this.setUniform('iSunSize', c.sunSize)
        this.setUniform('iGlow', c.glow)
        this.setUniform('iPalette', c.palette)
    }
}

const effect = new SynthHorizon()
initializeEffect(() => effect.initialize(), { instance: effect })
