import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Starfall', shader, {
    palette:  ['Celestial', 'Aurora Rain', 'Ember Fall', 'Frozen Tears', 'Neon Rain', 'Cosmic'],
    speed:    [1, 10, 5],
    density:  [0, 100, 50],
    trails:   [0, 100, 60],
    sparkle:  [0, 100, 30],
}, {
    description: 'Luminous particles cascading through darkness with glowing comet trails',
})
