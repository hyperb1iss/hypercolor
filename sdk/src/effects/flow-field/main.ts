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

interface FlowControls {
    speed: number
    particles: number
    trailFade: number
    noiseScale: number
    palette: number
}

const PALETTES = ['SilkCircuit', 'Cyberpunk', 'Aurora', 'Fire', 'Ice']

@Effect({
    name: 'Flow Field',
    description: 'Perlin noise vector field with streaming particle trails',
    author: 'Hypercolor',
})
class FlowField extends WebGLEffect<FlowControls> {
    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 5, tooltip: 'Flow speed' })
    speed!: number

    @NumberControl({ label: 'Particles', min: 10, max: 100, default: 60, tooltip: 'Particle count' })
    particles!: number

    @NumberControl({ label: 'Trail', min: 10, max: 100, default: 50, tooltip: 'Trail length' })
    trailFade!: number

    @NumberControl({ label: 'Scale', min: 10, max: 100, default: 50, tooltip: 'Noise scale' })
    noiseScale!: number

    @ComboboxControl({ label: 'Palette', values: PALETTES, default: 'SilkCircuit', tooltip: 'Color palette' })
    palette!: string

    constructor() {
        super({ fragmentShader })
    }

    protected initializeControls(): void {
        this.speed = getControlValue('speed', 5)
        this.particles = getControlValue('particles', 60)
        this.trailFade = getControlValue('trailFade', 50)
        this.noiseScale = getControlValue('noiseScale', 50)
        this.palette = getControlValue('palette', 'SilkCircuit')
    }

    protected getControlValues(): FlowControls {
        return {
            speed: normalizeSpeed(getControlValue('speed', 5)),
            particles: getControlValue('particles', 60),
            trailFade: getControlValue('trailFade', 50),
            noiseScale: getControlValue('noiseScale', 50),
            palette: PALETTES.indexOf(getControlValue('palette', 'SilkCircuit')),
        }
    }

    protected createUniforms(): void {
        this.registerUniform('iSpeed', 1.0)
        this.registerUniform('iParticles', 60)
        this.registerUniform('iTrailFade', 50)
        this.registerUniform('iNoiseScale', 50)
        this.registerUniform('iPalette', 0)
    }

    protected updateUniforms(c: FlowControls): void {
        this.setUniform('iSpeed', c.speed)
        this.setUniform('iParticles', c.particles)
        this.setUniform('iTrailFade', c.trailFade)
        this.setUniform('iNoiseScale', c.noiseScale)
        this.setUniform('iPalette', c.palette)
    }
}

const effect = new FlowField()
initializeEffect(() => effect.initialize(), { instance: effect })
