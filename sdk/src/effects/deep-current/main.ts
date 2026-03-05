import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Deep Current', shader, {
    leftColor:       '#ff4fb4',
    rightColor:      '#ffe36a',
    speed:           [1, 10, 4],
    rippleIntensity: [0, 100, 68],
    particleAmount:  [0, 100, 56],
    blend:           [0, 100, 26],
    splitMode:       ['Vertical', 'Horizontal', 'Diagonal'],
}, {
    description: 'Pink Lemonade split-field with crisp ripples and floating particles',
})
