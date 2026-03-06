import { color, effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Plasma Engine', shader, {
    theme:   ['Custom', 'Poison', 'Cyberpunk', 'Inferno', 'Aurora', 'Arcade', 'Tropical', 'Oceanic'],
    bgColor: color('Background Color', '#03020c', { uniform: 'iBackgroundColor' }),
    color1:  '#8dff4f',
    color2:  '#16d1d9',
    color3:  '#7d49ff',
    speed:   [1, 10, 6],
    bloom:   [0, 100, 48],
    spread:  [0, 100, 58],
    density: [10, 100, 54],
}, {
    description: 'Smooth demoscene plasma with contour bands and rich theme palettes',
})
