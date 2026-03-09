import { color, effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Plasma Engine', shader, {
    theme:   ['Custom', 'Poison', 'Cyberpunk', 'Inferno', 'Aurora', 'Arcade', 'Tropical', 'Oceanic'],
    bgColor: color('Background Color', '#03020c', { uniform: 'iBackgroundColor' }),
    color1:  '#16d1d9',
    color2:  '#ff4fb4',
    color3:  '#7d49ff',
    speed:   [1, 10, 4],
    bloom:   [0, 100, 16],
    spread:  [0, 100, 34],
    density: [10, 100, 32],
}, {
    description: 'Low-frequency demoscene plasma with fluid motion and saturated color drift',
})
