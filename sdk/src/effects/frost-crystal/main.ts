import { effect, combo } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Frost Crystal', shader, {
    speed:    [1, 10, 5],
    scale:    [10, 100, 56],
    edgeGlow: [0, 100, 74],
    growth:   [0, 100, 68],
    palette:  combo('Palette', ['SilkCircuit', 'Ice', 'Frost', 'Aurora', 'Cyberpunk'], { default: 'Ice' }),
    scene:    ['Lattice', 'Shardfield', 'Prism', 'Signal'],
}, {
    description: 'Sharp community-style crystal lattice with crisp geometric edge motifs',
})
