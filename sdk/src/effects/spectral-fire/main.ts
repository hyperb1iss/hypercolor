import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Spectral Fire', shader, {
    speed:       [1, 10, 6],
    flameHeight: [20, 100, 78],
    turbulence:  [0, 100, 62],
    intensity:   [20, 100, 84],
    palette:     ['Bonfire', 'Forge', 'Spellfire', 'Sulfur', 'Ashfall'],
    emberAmount: [0, 100, 60],
    scene:       ['Classic', 'Inferno', 'Torch', 'Wildfire'],
}, {
    description: 'Layered fire tongues with embers and optional audio lift',
    audio: true,
})
