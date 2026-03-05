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

interface EmberControls {
    speed: number
    intensity: number
    emberDensity: number
    flowSpread: number
    glow: number
    palette: number
    scene: number
}

const PALETTES = ['Forge', 'Poison', 'SilkCircuit', 'Ash Bloom', 'Toxic Rust']
const SCENES = ['Updraft', 'Crosswind', 'Vortex']

@Effect({
    name: 'Ember Glow',
    description: 'Crisp ember flecks in directional poison-forge flow with selectable scene behavior',
    author: 'Hypercolor',
    audioReactive: false,
})
class EmberGlow extends WebGLEffect<EmberControls> {
    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 5, tooltip: 'Animation speed' })
    speed!: number

    @NumberControl({ label: 'Intensity', min: 0, max: 100, default: 74, tooltip: 'Heat and brightness' })
    intensity!: number

    @NumberControl({ label: 'Ember Density', min: 0, max: 100, default: 58, tooltip: 'Crisp fleck count' })
    emberDensity!: number

    @NumberControl({ label: 'Flow/Spread', min: 0, max: 100, default: 62, tooltip: 'Directional drift and turbulence' })
    flowSpread!: number

    @NumberControl({ label: 'Glow', min: 0, max: 100, default: 68, tooltip: 'Bloom around hot clusters' })
    glow!: number

    @ComboboxControl({
        label: 'Palette',
        values: PALETTES,
        default: 'Forge',
        tooltip: 'Color personality',
    })
    palette!: string

    @ComboboxControl({
        label: 'Scene',
        values: SCENES,
        default: 'Updraft',
        tooltip: 'Directional flow mode',
    })
    scene!: string

    constructor() {
        super({ id: 'ember-glow', name: 'Ember Glow', fragmentShader, audioReactive: false })
    }

    protected initializeControls(): void {
        this.speed = getControlValue('speed', 5)
        this.intensity = getControlValue('intensity', 74)
        this.emberDensity = getControlValue('emberDensity', 58)
        this.flowSpread = getControlValue('flowSpread', 62)
        this.glow = getControlValue('glow', 68)
        this.palette = getControlValue('palette', 'Forge')
        this.scene = getControlValue('scene', 'Updraft')
    }

    protected getControlValues(): EmberControls {
        return {
            speed: normalizeSpeed(getControlValue('speed', 5)),
            intensity: getControlValue('intensity', 74),
            emberDensity: getControlValue('emberDensity', 58),
            flowSpread: getControlValue('flowSpread', 62),
            glow: getControlValue('glow', 68),
            palette: comboboxValueToIndex(getControlValue('palette', 'Forge'), PALETTES, 0),
            scene: comboboxValueToIndex(getControlValue('scene', 'Updraft'), SCENES, 0),
        }
    }

    protected createUniforms(): void {
        this.registerUniform('iSpeed', normalizeSpeed(5))
        this.registerUniform('iIntensity', 74)
        this.registerUniform('iEmberDensity', 58)
        this.registerUniform('iFlowSpread', 62)
        this.registerUniform('iGlow', 68)
        this.registerUniform('iPalette', 0)
        this.registerUniform('iScene', 0)
    }

    protected updateUniforms(controls: EmberControls): void {
        this.setUniform('iSpeed', controls.speed)
        this.setUniform('iIntensity', controls.intensity)
        this.setUniform('iEmberDensity', controls.emberDensity)
        this.setUniform('iFlowSpread', controls.flowSpread)
        this.setUniform('iGlow', controls.glow)
        this.setUniform('iPalette', controls.palette)
        this.setUniform('iScene', controls.scene)
    }
}

const effect = new EmberGlow()
initializeEffect(() => effect.initialize(), { instance: effect })
