import 'reflect-metadata'
import {
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

interface BorealisControls {
    speed: number
    intensity: number
    warpStrength: number
    starBrightness: number
    curtainHeight: number
    palette: number
}

const PALETTES = ['Northern Lights', 'SilkCircuit', 'Cyberpunk', 'Sunset', 'Ice', 'Fire', 'Vaporwave', 'Phosphor']

@Effect({
    name: 'Borealis',
    description: 'Aurora borealis — layered curtains of light with domain-warped fBm noise',
    author: 'Hypercolor',
    audioReactive: false,
})
class Borealis extends WebGLEffect<BorealisControls> {
    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 5, tooltip: 'Animation speed' })
    speed!: number

    @NumberControl({ label: 'Intensity', min: 0, max: 100, default: 82, tooltip: 'Aurora brightness' })
    intensity!: number

    @NumberControl({ label: 'Warp', min: 0, max: 100, default: 62, tooltip: 'Domain warping strength' })
    warpStrength!: number

    @NumberControl({ label: 'Stars', min: 0, max: 100, default: 40, tooltip: 'Star brightness' })
    starBrightness!: number

    @NumberControl({ label: 'Height', min: 20, max: 90, default: 55, tooltip: 'Aurora vertical position' })
    curtainHeight!: number

    @ComboboxControl({
        label: 'Palette',
        values: PALETTES,
        default: 'Northern Lights',
        tooltip: 'Color palette',
    })
    palette!: string

    constructor() {
        super({ fragmentShader })
    }

    protected initializeControls(): void {
        this.speed = getControlValue('speed', 5)
        this.intensity = getControlValue('intensity', 82)
        this.warpStrength = getControlValue('warpStrength', 62)
        this.starBrightness = getControlValue('starBrightness', 40)
        this.curtainHeight = getControlValue('curtainHeight', 55)
        this.palette = getControlValue('palette', 'Northern Lights')
    }

    protected getControlValues(): BorealisControls {
        return {
            speed: normalizeSpeed(getControlValue('speed', 5)),
            intensity: getControlValue('intensity', 82),
            warpStrength: getControlValue('warpStrength', 62),
            starBrightness: getControlValue('starBrightness', 40),
            curtainHeight: getControlValue('curtainHeight', 55),
            palette: comboboxValueToIndex(getControlValue('palette', 'Northern Lights'), PALETTES, 0),
        }
    }

    protected createUniforms(): void {
        this.registerUniform('iSpeed', 1.0)
        this.registerUniform('iIntensity', 82)
        this.registerUniform('iWarpStrength', 62)
        this.registerUniform('iStarBrightness', 40)
        this.registerUniform('iCurtainHeight', 55)
        this.registerUniform('iPalette', 0)
    }

    protected updateUniforms(controls: BorealisControls): void {
        this.setUniform('iSpeed', controls.speed)
        this.setUniform('iIntensity', controls.intensity)
        this.setUniform('iWarpStrength', controls.warpStrength)
        this.setUniform('iStarBrightness', controls.starBrightness)
        this.setUniform('iCurtainHeight', controls.curtainHeight)
        this.setUniform('iPalette', controls.palette)
    }
}

const effect = new Borealis()
initializeEffect(() => effect.initialize(), { instance: effect })
