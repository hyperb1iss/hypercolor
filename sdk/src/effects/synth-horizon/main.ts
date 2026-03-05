import { effect, combo } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Synth Horizon', shader, {
    scene:      ['Roller Grid', 'Arcade Carpet', 'Laser Lanes'],
    speed:      [1, 10, 5],
    gridDensity: [10, 100, 62],
    glow:       [10, 100, 72],
    palette:    combo('Palette', ['SilkCircuit', 'Rink Pop', 'Arcade Heat', 'Ice Neon', 'Midnight'], { default: 'Rink Pop' }),
    colorMode:  combo('Color Mode', ['Static', 'Color Cycle', 'Mono Neon'], { default: 'Color Cycle' }),
    cycleSpeed: [0, 100, 44],
}, {
    description: 'Crisp retro roller-rink geometry with arcade carpet motifs and neon horizon scenes',
})
