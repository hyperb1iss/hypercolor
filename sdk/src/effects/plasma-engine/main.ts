import 'reflect-metadata'
import {
    ColorControl,
    Effect,
    NumberControl,
    WebGLEffect,
    getControlValue,
    initializeEffect,
    normalizeSpeed,
} from '@hypercolor/sdk'

import fragmentShader from './fragment.glsl'

interface PlasmaControls {
    bgColor: [number, number, number]
    color1: [number, number, number]
    color2: [number, number, number]
    color3: [number, number, number]
    speed: number
    bloom: number
    spread: number
    density: number
}

const DEFAULT_BG = '#03020c'
const DEFAULT_COLOR_1 = '#94ff4f'
const DEFAULT_COLOR_2 = '#2cc8ff'
const DEFAULT_COLOR_3 = '#ff4fd8'

@Effect({
    name: 'Plasma Engine',
    description: 'Dual-flow Poison Bloom particle field with crisp additive sparks',
    author: 'Hypercolor',
})
class PlasmaEngine extends WebGLEffect<PlasmaControls> {
    @ColorControl({ label: 'Background color', default: DEFAULT_BG, tooltip: 'Background tone' })
    bgColor!: string

    @ColorControl({ label: 'Color 1', default: DEFAULT_COLOR_1, tooltip: 'Primary particle stream color' })
    color1!: string

    @ColorControl({ label: 'Color 2', default: DEFAULT_COLOR_2, tooltip: 'Secondary counter-flow color' })
    color2!: string

    @ColorControl({ label: 'Color 3', default: DEFAULT_COLOR_3, tooltip: 'Bloom accent color' })
    color3!: string

    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 5, tooltip: 'Animation speed' })
    speed!: number

    @NumberControl({ label: 'Bloom', min: 0, max: 100, default: 68, tooltip: 'Additive glow strength' })
    bloom!: number

    @NumberControl({ label: 'Spread', min: 0, max: 100, default: 54, tooltip: 'Flow curl and orbit spread' })
    spread!: number

    @NumberControl({ label: 'Density', min: 10, max: 100, default: 60, tooltip: 'Particle count' })
    density!: number

    constructor() {
        super({ id: 'plasma-engine', name: 'Plasma Engine', fragmentShader })
    }

    protected initializeControls(): void {
        this.bgColor = getControlValue('bgColor', DEFAULT_BG)
        this.color1 = getControlValue('color1', DEFAULT_COLOR_1)
        this.color2 = getControlValue('color2', DEFAULT_COLOR_2)
        this.color3 = getControlValue('color3', DEFAULT_COLOR_3)
        this.speed = getControlValue('speed', 5)
        this.bloom = getControlValue('bloom', 68)
        this.spread = getControlValue('spread', 54)
        this.density = getControlValue('density', 60)
    }

    protected getControlValues(): PlasmaControls {
        return {
            bgColor: this.hexToVec3(getControlValue('bgColor', DEFAULT_BG), DEFAULT_BG),
            color1: this.hexToVec3(getControlValue('color1', DEFAULT_COLOR_1), DEFAULT_COLOR_1),
            color2: this.hexToVec3(getControlValue('color2', DEFAULT_COLOR_2), DEFAULT_COLOR_2),
            color3: this.hexToVec3(getControlValue('color3', DEFAULT_COLOR_3), DEFAULT_COLOR_3),
            speed: normalizeSpeed(getControlValue('speed', 5)),
            bloom: getControlValue('bloom', 68),
            spread: getControlValue('spread', 54),
            density: getControlValue('density', 60),
        }
    }

    protected createUniforms(): void {
        this.registerUniform('iBackgroundColor', this.hexToVec3(DEFAULT_BG, DEFAULT_BG))
        this.registerUniform('iColor1', this.hexToVec3(DEFAULT_COLOR_1, DEFAULT_COLOR_1))
        this.registerUniform('iColor2', this.hexToVec3(DEFAULT_COLOR_2, DEFAULT_COLOR_2))
        this.registerUniform('iColor3', this.hexToVec3(DEFAULT_COLOR_3, DEFAULT_COLOR_3))
        this.registerUniform('iSpeed', 1.0)
        this.registerUniform('iBloom', 68)
        this.registerUniform('iSpread', 54)
        this.registerUniform('iDensity', 60)
    }

    protected updateUniforms(controls: PlasmaControls): void {
        this.setUniform('iBackgroundColor', controls.bgColor)
        this.setUniform('iColor1', controls.color1)
        this.setUniform('iColor2', controls.color2)
        this.setUniform('iColor3', controls.color3)
        this.setUniform('iSpeed', controls.speed)
        this.setUniform('iBloom', controls.bloom)
        this.setUniform('iSpread', controls.spread)
        this.setUniform('iDensity', controls.density)
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

const effect = new PlasmaEngine()
initializeEffect(() => effect.initialize(), { instance: effect })
