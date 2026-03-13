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
    presets: [
        {
            name: 'Kessel Run',
            description: 'Full-throttle smuggler sprint — maxed star density and screaming velocity through a cyberpunk corridor',
            controls: {
                palette: 'Cyberpunk',
                speed: 9.5,
                density: 145,
                streak: 155,
                warp: 35,
            },
        },
        {
            name: 'Phantom Drift',
            description: 'Spectral afterimages of dead stars — ghostly trails dissipate through a haunted gate between dimensions',
            controls: {
                palette: 'Phantom Gate',
                speed: 3,
                density: 40,
                streak: 130,
                warp: 78,
            },
        },
        {
            name: 'Solar Ejection',
            description: 'Riding the shockwave of a coronal mass ejection — gold and amber debris hurtling past the cockpit',
            controls: {
                palette: 'Solar Wind',
                speed: 7.5,
                density: 110,
                streak: 95,
                warp: 20,
            },
        },
        {
            name: 'Void Meditation',
            description: 'Sensory deprivation at light speed — minimal starfield spiraling inward through absolute darkness',
            controls: {
                palette: 'Void',
                speed: 1.5,
                density: 18,
                streak: 45,
                warp: 92,
            },
        },
        {
            name: 'Warp Core Breach',
            description: 'Containment failure — the tunnel collapses into an overdrive vortex of classic blue-white streaks',
            controls: {
                palette: 'Warp Core',
                speed: 10,
                density: 160,
                streak: 160,
                warp: 100,
            },
        },
    ],
})
