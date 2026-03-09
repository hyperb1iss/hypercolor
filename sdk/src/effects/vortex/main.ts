import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Vortex', shader, {
    palette: ['SilkCircuit', 'Cyberpunk', 'Synthwave', 'Aurora', 'Fire', 'Ice', 'Ocean', 'Neon Flux'],
    speed:   [1, 10, 4],
    arms:    [2, 6, 3],
    twist:   [0, 100, 50],
    depth:   [0, 100, 40],
}, {
    description: 'Mesmerizing logarithmic spiral with differential rotation and vivid color drift',
})
