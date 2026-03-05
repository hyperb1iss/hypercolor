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

interface FireControls {
    speed: number
    flameHeight: number
    turbulence: number
    intensity: number
    emberAmount: number
    palette: number
    scene: number
}

const PALETTES = ['Bonfire', 'Forge', 'Spellfire', 'Sulfur', 'Ashfall']
const SCENES = ['Classic', 'Inferno', 'Torch', 'Wildfire']

@Effect({
    name: 'Spectral Fire',
    description: 'Layered community-style fire tongues with embers and optional audio lift',
    author: 'Hypercolor',
    audioReactive: true,
})
class SpectralFire extends WebGLEffect<FireControls> {
    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 6, tooltip: 'Flame animation speed' })
    speed!: number

    @NumberControl({ label: 'Flame Height', min: 20, max: 100, default: 78, tooltip: 'Vertical flame reach' })
    flameHeight!: number

    @NumberControl({ label: 'Turbulence', min: 0, max: 100, default: 62, tooltip: 'Curl and breakup in flame layers' })
    turbulence!: number

    @NumberControl({ label: 'Intensity', min: 20, max: 100, default: 84, tooltip: 'Overall fire brightness and heat' })
    intensity!: number

    @ComboboxControl({ label: 'Palette', values: PALETTES, default: 'Bonfire', tooltip: 'Flame color profile' })
    palette!: string

    @NumberControl({ label: 'Ember Amount', min: 0, max: 100, default: 60, tooltip: 'Amount of rising ember particles' })
    emberAmount!: number

    @ComboboxControl({ label: 'Scene', values: SCENES, default: 'Classic', tooltip: 'Optional fire behavior mode' })
    scene!: string

    constructor() {
        super({ id: 'spectral-fire', name: 'Spectral Fire', fragmentShader, audioReactive: true })
    }

    protected initializeControls(): void {
        this.speed = getControlValue('speed', 6)
        this.flameHeight = getControlValue('flameHeight', 78)
        this.turbulence = getControlValue('turbulence', 62)
        this.intensity = getControlValue('intensity', 84)
        this.palette = getControlValue('palette', 'Bonfire')
        this.emberAmount = getControlValue('emberAmount', 60)
        this.scene = getControlValue('scene', 'Classic')
    }

    protected getControlValues(): FireControls {
        return {
            speed: normalizeSpeed(getControlValue('speed', 6)),
            flameHeight: getControlValue('flameHeight', 78),
            turbulence: getControlValue('turbulence', 62),
            intensity: getControlValue('intensity', 84),
            emberAmount: getControlValue('emberAmount', 60),
            palette: comboboxValueToIndex(getControlValue('palette', 'Bonfire'), PALETTES, 0),
            scene: comboboxValueToIndex(getControlValue('scene', 'Classic'), SCENES, 0),
        }
    }

    protected createUniforms(): void {
        this.registerUniform('iSpeed', normalizeSpeed(6))
        this.registerUniform('iFlameHeight', 78)
        this.registerUniform('iTurbulence', 62)
        this.registerUniform('iIntensity', 84)
        this.registerUniform('iEmberAmount', 60)
        this.registerUniform('iPalette', 0)
        this.registerUniform('iScene', 0)
    }

    protected updateUniforms(c: FireControls): void {
        this.setUniform('iSpeed', c.speed)
        this.setUniform('iFlameHeight', c.flameHeight)
        this.setUniform('iTurbulence', c.turbulence)
        this.setUniform('iIntensity', c.intensity)
        this.setUniform('iEmberAmount', c.emberAmount)
        this.setUniform('iPalette', c.palette)
        this.setUniform('iScene', c.scene)
    }
}

const effect = new SpectralFire()
initializeEffect(() => effect.initialize(), { instance: effect })
