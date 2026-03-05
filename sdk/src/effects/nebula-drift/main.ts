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

interface NebulaControls {
    speed: number
    density: number
    starSize: number
    glow: number
    direction: number
    background: number
    starMode: number
    palette: number
}

const DIRECTION_MODES = ['Forward', 'Reverse', 'Left', 'Right', 'Up', 'Down']
const BACKGROUND_MODES = ['Deep Space', 'Cockpit', 'Wormhole', 'Grid Void']
const STAR_MODES = ['Needles', 'Comets', 'Shards', 'Mixed']
const PALETTES = ['SilkCircuit', 'Hyperblue', 'Solar', 'Aurora', 'Monochrome']

@Effect({
    name: 'Nebula Drift',
    description: 'Hyperspace warp tunnel with crisp streak stars and directional flight modes',
    author: 'Hypercolor',
})
class NebulaDrift extends WebGLEffect<NebulaControls> {
    @ComboboxControl({
        label: 'Direction Mode',
        values: DIRECTION_MODES,
        default: 'Forward',
        tooltip: 'Flight direction for the warp stream',
    })
    direction!: string

    @ComboboxControl({
        label: 'Background Mode',
        values: BACKGROUND_MODES,
        default: 'Deep Space',
        tooltip: 'Backdrop style behind stars',
    })
    background!: string

    @ComboboxControl({
        label: 'Star Mode',
        values: STAR_MODES,
        default: 'Needles',
        tooltip: 'Streak shape profile',
    })
    starMode!: string

    @ComboboxControl({
        label: 'Color Mode',
        values: PALETTES,
        default: 'SilkCircuit',
        tooltip: 'Palette for stars and tunnel glow',
    })
    palette!: string

    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 6, tooltip: 'Warp velocity' })
    speed!: number

    @NumberControl({ label: 'Density', min: 10, max: 100, default: 56, tooltip: 'Star count' })
    density!: number

    @NumberControl({ label: 'Star Size', min: 5, max: 100, default: 45, tooltip: 'Streak thickness and tail length' })
    starSize!: number

    @NumberControl({ label: 'Glow', min: 0, max: 100, default: 68, tooltip: 'Bloom around star streaks' })
    glow!: number

    constructor() {
        super({ fragmentShader })
    }

    protected initializeControls(): void {
        this.direction = getControlValue('direction', 'Forward')
        this.background = getControlValue('background', 'Deep Space')
        this.starMode = getControlValue('starMode', 'Needles')
        this.palette = getControlValue('palette', 'SilkCircuit')
        this.speed = getControlValue('speed', 6)
        this.density = getControlValue('density', 56)
        this.starSize = getControlValue('starSize', 45)
        this.glow = getControlValue('glow', 68)
    }

    protected getControlValues(): NebulaControls {
        return {
            speed: normalizeSpeed(getControlValue('speed', 6)),
            density: getControlValue('density', 56),
            starSize: getControlValue('starSize', 45),
            glow: getControlValue('glow', 68),
            direction: comboboxValueToIndex(getControlValue('direction', 'Forward'), DIRECTION_MODES, 0),
            background: comboboxValueToIndex(getControlValue('background', 'Deep Space'), BACKGROUND_MODES, 0),
            starMode: comboboxValueToIndex(getControlValue('starMode', 'Needles'), STAR_MODES, 0),
            palette: comboboxValueToIndex(getControlValue('palette', 'SilkCircuit'), PALETTES, 0),
        }
    }

    protected createUniforms(): void {
        this.registerUniform('iSpeed', 1.0)
        this.registerUniform('iDensity', 56)
        this.registerUniform('iStarSize', 45)
        this.registerUniform('iGlow', 68)
        this.registerUniform('iDirection', 0)
        this.registerUniform('iBackground', 0)
        this.registerUniform('iStarMode', 0)
        this.registerUniform('iPalette', 0)
    }

    protected updateUniforms(c: NebulaControls): void {
        this.setUniform('iSpeed', c.speed)
        this.setUniform('iDensity', c.density)
        this.setUniform('iStarSize', c.starSize)
        this.setUniform('iGlow', c.glow)
        this.setUniform('iDirection', c.direction)
        this.setUniform('iBackground', c.background)
        this.setUniform('iStarMode', c.starMode)
        this.setUniform('iPalette', c.palette)
    }
}

const effect = new NebulaDrift()
initializeEffect(() => effect.initialize(), { instance: effect })
