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

interface ShockwaveControls {
    speed: number
    intensity: number
    ringCount: number
    decay: number
    palette: number
    scene: number
}

const PALETTES = ['SilkCircuit', 'Cyberpunk', 'Fire', 'Aurora', 'Ice']
const SCENES = ['Core Burst', 'Twin Burst', 'Prism Grid']

@Effect({
    name: 'Bass Shockwave',
    description: 'Crisp burst-driven shockwave rings with scene-selectable compositions',
    author: 'Hypercolor',
    audioReactive: true,
})
class BassShockwave extends WebGLEffect<ShockwaveControls> {
    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 6, tooltip: 'Ring expansion speed' })
    speed!: number

    @NumberControl({ label: 'Intensity', min: 0, max: 100, default: 78, tooltip: 'Ring brightness and burst punch' })
    intensity!: number

    @NumberControl({ label: 'Ring Count', min: 0, max: 100, default: 58, tooltip: 'Number of active expanding rings' })
    ringCount!: number

    @NumberControl({ label: 'Decay', min: 0, max: 100, default: 52, tooltip: 'How quickly older rings fade' })
    decay!: number

    @ComboboxControl({ label: 'Palette', values: PALETTES, default: 'SilkCircuit', tooltip: 'Color palette' })
    palette!: string

    @ComboboxControl({ label: 'Scene', values: SCENES, default: 'Core Burst', tooltip: 'Shockwave composition mode' })
    scene!: string

    constructor() {
        super({
            id: 'bass-shockwave',
            name: 'Bass Shockwave',
            fragmentShader,
            audioReactive: true,
        })
    }

    protected initializeControls(): void {
        this.speed = getControlValue('speed', 6)
        this.intensity = getControlValue('intensity', 78)
        this.ringCount = getControlValue('ringCount', 58)
        this.decay = getControlValue('decay', 52)
        this.palette = getControlValue('palette', 'SilkCircuit')
        this.scene = getControlValue('scene', 'Core Burst')
    }

    protected getControlValues(): ShockwaveControls {
        return {
            speed: normalizeSpeed(getControlValue('speed', 6)),
            intensity: getControlValue('intensity', 78),
            ringCount: getControlValue('ringCount', 58),
            decay: getControlValue('decay', 52),
            palette: comboboxValueToIndex(getControlValue('palette', 'SilkCircuit'), PALETTES, 0),
            scene: comboboxValueToIndex(getControlValue('scene', 'Core Burst'), SCENES, 0),
        }
    }

    protected createUniforms(): void {
        this.registerUniform('iSpeed', 1.0)
        this.registerUniform('iIntensity', 78)
        this.registerUniform('iRingCount', 58)
        this.registerUniform('iDecay', 52)
        this.registerUniform('iPalette', 0)
        this.registerUniform('iScene', 0)
    }

    protected updateUniforms(c: ShockwaveControls): void {
        this.setUniform('iSpeed', c.speed)
        this.setUniform('iIntensity', c.intensity)
        this.setUniform('iRingCount', c.ringCount)
        this.setUniform('iDecay', c.decay)
        this.setUniform('iPalette', c.palette)
        this.setUniform('iScene', c.scene)
    }
}

const effect = new BassShockwave()
initializeEffect(() => effect.initialize(), { instance: effect })
