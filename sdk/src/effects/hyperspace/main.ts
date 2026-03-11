import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Hyperspace', shader, {
    palette: combo('Theme', ['Classic', 'Cyberpunk', 'Phantom Gate', 'Solar Wind', 'Void', 'Warp Core'], {
        default: 'Cyberpunk',
        tooltip: 'Choose the tunnel tint. Cyberpunk gives the strongest first-run contrast.',
    }),
    speed: num('Velocity', [1, 10], 6, {
        step: 0.5,
        tooltip: 'Forward travel speed through the tunnel.',
    }),
    density: num('Star Density', [0, 160], 74, {
        step: 1,
        tooltip: 'How many star lanes fill the hyperspace corridor. Push past 100 for a denser overdrive field.',
    }),
    streak: num('Trail Length', [0, 160], 84, {
        step: 1,
        tooltip: 'Length and visual weight of the speed lines. Push past 100 for extended overdrive trails.',
    }),
    warp: num('Tunnel Twist', [0, 100], 62, {
        step: 1,
        tooltip: 'Amount of spiral distortion around the center.',
    }),
}, {
    description: 'Dense layered star lanes with longer trails and stronger tunnel twist for a bolder hyperspace jump',
})
