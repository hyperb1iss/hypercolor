import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Borealis', shader, {
    speed:          [1, 10, 5],
    intensity:      [0, 100, 82],
    warpStrength:   [0, 100, 62],
    starBrightness: [0, 100, 40],
    curtainHeight:  [20, 90, 55],
    saturation:     [50, 180, 125],
    contrast:       [60, 160, 108],
    banding:        [0, 100, 44],
    palette:        ['Northern Lights', 'SilkCircuit', 'Cyberpunk', 'Sunset', 'Ice', 'Fire', 'Vaporwave', 'Phosphor'],
}, {
    description: 'Aurora borealis — layered curtains of light with richer palette grading and tonal control',
})
