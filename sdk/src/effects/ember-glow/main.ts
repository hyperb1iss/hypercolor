import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Ember Glow', shader, {
    speed:       [1, 10, 5],
    intensity:   [0, 100, 74],
    emberDensity: [0, 100, 58],
    flowSpread:  [0, 100, 62],
    glow:        [0, 100, 68],
    palette:     ['Forge', 'Poison', 'SilkCircuit', 'Ash Bloom', 'Toxic Rust'],
    scene:       ['Updraft', 'Crosswind', 'Vortex'],
}, {
    description: 'Crisp ember flecks in directional poison-forge flow with selectable scene behavior',
})
