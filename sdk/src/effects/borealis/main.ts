import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Borealis', shader, {
    speed:          [1, 10, 5],
    intensity:      [0, 100, 82],
    warpStrength:   [0, 100, 62],
    starBrightness: [0, 100, 40],
    curtainHeight:  [20, 90, 55],
    saturation:     [60, 150, 118],
    contrast:       [70, 140, 104],
    banding:        [0, 100, 34],
    palette:        ['Cyberpunk', 'Fire', 'Ice', 'Northern Lights', 'Phosphor', 'SilkCircuit', 'Sunset', 'Vaporwave'],
}, {
    description: 'Aurora borealis — layered curtains of light with richer palette grading and tonal control',
})
