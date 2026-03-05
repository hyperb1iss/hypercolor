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

interface LavaLampControls {
    speed: number
    blobCount: number
    blobSize: number
    viscosity: number
    glow: number
    palette: number
}

const PALETTES = ['Classic Lava', 'Neon Night', 'Candy', 'Toxic', 'Ocean']

@Effect({
    name: 'Lava Lamp Superfluid',
    description: 'Metaball lava lamp with merge/split flow, glassy bloom, and rich color scenes',
    author: 'Hypercolor',
    audioReactive: false,
})
class LavaLampSuperfluid extends WebGLEffect<LavaLampControls> {
    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 5, tooltip: 'Fluid motion speed' })
    speed!: number

    @NumberControl({ label: 'Blob Count', min: 4, max: 16, default: 9, tooltip: 'Amount of lava blobs' })
    blobCount!: number

    @NumberControl({ label: 'Blob Size', min: 20, max: 100, default: 58, tooltip: 'Average blob radius' })
    blobSize!: number

    @NumberControl({ label: 'Viscosity', min: 0, max: 100, default: 68, tooltip: 'How thick and stretchy blobs feel' })
    viscosity!: number

    @NumberControl({ label: 'Glow', min: 0, max: 100, default: 66, tooltip: 'Bloom around blob edges' })
    glow!: number

    @ComboboxControl({
        label: 'Palette',
        values: PALETTES,
        default: 'Classic Lava',
        tooltip: 'Color scene',
    })
    palette!: string

    constructor() {
        super({ fragmentShader, audioReactive: false })
    }

    protected initializeControls(): void {
        this.speed = getControlValue('speed', 5)
        this.blobCount = getControlValue('blobCount', 9)
        this.blobSize = getControlValue('blobSize', 58)
        this.viscosity = getControlValue('viscosity', 68)
        this.glow = getControlValue('glow', 66)
        this.palette = getControlValue('palette', 'Classic Lava')
    }

    protected getControlValues(): LavaLampControls {
        return {
            speed: normalizeSpeed(getControlValue('speed', 5)),
            blobCount: getControlValue('blobCount', 9),
            blobSize: getControlValue('blobSize', 58),
            viscosity: getControlValue('viscosity', 68),
            glow: getControlValue('glow', 66),
            palette: comboboxValueToIndex(getControlValue('palette', 'Classic Lava'), PALETTES, 0),
        }
    }

    protected createUniforms(): void {
        this.registerUniform('iSpeed', normalizeSpeed(5))
        this.registerUniform('iBlobCount', 9)
        this.registerUniform('iBlobSize', 58)
        this.registerUniform('iViscosity', 68)
        this.registerUniform('iGlow', 66)
        this.registerUniform('iPalette', 0)
    }

    protected updateUniforms(c: LavaLampControls): void {
        this.setUniform('iSpeed', c.speed)
        this.setUniform('iBlobCount', c.blobCount)
        this.setUniform('iBlobSize', c.blobSize)
        this.setUniform('iViscosity', c.viscosity)
        this.setUniform('iGlow', c.glow)
        this.setUniform('iPalette', c.palette)
    }
}

const effect = new LavaLampSuperfluid()
initializeEffect(() => effect.initialize(), { instance: effect })
