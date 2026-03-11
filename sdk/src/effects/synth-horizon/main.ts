import { effect, combo } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Synth Horizon', shader, {
    scene:      ['Arcade Carpet', 'Laser Lanes', 'Roller Grid'],
    speed:      [1, 10, 5],
    gridDensity: [10, 100, 62],
    glow:       [10, 100, 58],
    palette:    combo('Palette', ['Arcade Heat', 'Ice Neon', 'Midnight', 'Rink Pop', 'SilkCircuit'], { default: 'SilkCircuit' }),
    colorMode:  combo('Color Mode', ['Color Cycle', 'Mono Neon', 'Static'], { default: 'Static' }),
    cycleSpeed: [0, 100, 36],
}, {
    description: 'Crisp retro roller-rink geometry with arcade carpet motifs and neon horizon scenes',
})
