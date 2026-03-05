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

interface BorealisControls {
    speed: number
    intensity: number
    warpStrength: number
    starBrightness: number
    curtainHeight: number
    palette: number
}

const PALETTES = ['Aurora', 'SilkCircuit', 'Cyberpunk', 'Sunset', 'Ice', 'Fire', 'Vaporwave', 'Phosphor']

@Effect({
    name: 'Borealis',
    description: 'Aurora borealis — layered curtains of light with domain-warped fBm noise',
    author: 'Hypercolor',
})
class Borealis extends WebGLEffect<BorealisControls> {
    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 4, tooltip: 'Animation speed' })
    speed!: number

    @NumberControl({ label: 'Intensity', min: 0, max: 100, default: 70, tooltip: 'Aurora brightness' })
    intensity!: number

    @NumberControl({ label: 'Warp', min: 0, max: 100, default: 50, tooltip: 'Domain warping strength' })
    warpStrength!: number

    @NumberControl({ label: 'Stars', min: 0, max: 100, default: 50, tooltip: 'Star brightness' })
    starBrightness!: number

    @NumberControl({ label: 'Height', min: 20, max: 90, default: 65, tooltip: 'Aurora vertical position' })
    curtainHeight!: number

    @ComboboxControl({
        label: 'Palette',
        values: PALETTES,
        default: 'Aurora',
        tooltip: 'Color palette',
    })
    palette!: string

    constructor() {
        super({ fragmentShader })
    }

    protected initializeControls(): void {
        this.speed = getControlValue('speed', 4)
        this.intensity = getControlValue('intensity', 70)
        this.warpStrength = getControlValue('warpStrength', 50)
        this.starBrightness = getControlValue('starBrightness', 50)
        this.curtainHeight = getControlValue('curtainHeight', 65)
        this.palette = getControlValue('palette', 'Aurora')
    }

    protected getControlValues(): BorealisControls {
        return {
            speed: normalizeSpeed(getControlValue('speed', 4)),
            intensity: getControlValue('intensity', 70),
            warpStrength: getControlValue('warpStrength', 50),
            starBrightness: getControlValue('starBrightness', 50),
            curtainHeight: getControlValue('curtainHeight', 65),
            palette: PALETTES.indexOf(getControlValue('palette', 'Aurora')),
        }
    }

    protected createUniforms(): void {
        this.registerUniform('iSpeed', 1.0)
        this.registerUniform('iIntensity', 70)
        this.registerUniform('iWarpStrength', 50)
        this.registerUniform('iStarBrightness', 50)
        this.registerUniform('iCurtainHeight', 65)
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
