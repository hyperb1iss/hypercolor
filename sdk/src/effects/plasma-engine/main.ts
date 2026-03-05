import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Plasma Engine', shader, {
    bgColor: '#03020c',
    color1:  '#94ff4f',
    color2:  '#2cc8ff',
    color3:  '#ff4fd8',
    speed:   [1, 10, 5],
    bloom:   [0, 100, 68],
    spread:  [0, 100, 54],
    density: [10, 100, 60],
}, {
    description: 'Dual-flow Poison Bloom particle field with crisp additive sparks',
})
