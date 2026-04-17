import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'Hyperspace',
    shader,
    {
        density: num('Star Density', [0, 160], 74, {
            group: 'Density',
            step: 1,
            tooltip: 'How many star lanes fill the hyperspace corridor. Push past 100 for a denser overdrive field.',
        }),
        palette: combo('Theme', ['Classic', 'Cyberpunk', 'Phantom Gate', 'Solar Wind', 'Void', 'Warp Core'], {
            default: 'Cyberpunk',
            group: 'Color',
            tooltip: 'Choose the tunnel tint. Cyberpunk gives the strongest first-run contrast.',
        }),
        speed: num('Velocity', [1, 10], 6, {
            group: 'Motion',
            step: 0.5,
            tooltip: 'Forward travel speed through the tunnel.',
        }),
        streak: num('Trail Length', [0, 160], 84, {
            group: 'Density',
            step: 1,
            tooltip: 'Length and visual weight of the speed lines. Push past 100 for extended overdrive trails.',
        }),
        warp: num('Tunnel Twist', [0, 100], 62, {
            group: 'Motion',
            step: 1,
            tooltip: 'Amount of spiral distortion around the center.',
        }),
    },
    {
        description:
            'Punch through the light barrier. Star lanes streak into infinite trails as the tunnel warps and twists around you.',
        presets: [
            {
                controls: {
                    density: 145,
                    palette: 'Cyberpunk',
                    speed: 9.5,
                    streak: 155,
                    warp: 35,
                },
                description:
                    'Full-throttle smuggler sprint. Maxed star density, screaming velocity, a cyberpunk corridor blurring past.',
                name: 'Kessel Run',
            },
            {
                controls: {
                    density: 40,
                    palette: 'Phantom Gate',
                    speed: 3,
                    streak: 130,
                    warp: 78,
                },
                description:
                    'Spectral afterimages of dead stars. Ghostly trails dissipate through a haunted gate between dimensions.',
                name: 'Phantom Drift',
            },
            {
                controls: {
                    density: 110,
                    palette: 'Solar Wind',
                    speed: 7.5,
                    streak: 95,
                    warp: 20,
                },
                description:
                    'Riding the shockwave of a coronal mass ejection. Gold and amber debris hurtle past the cockpit.',
                name: 'Solar Ejection',
            },
            {
                controls: {
                    density: 18,
                    palette: 'Void',
                    speed: 1.5,
                    streak: 45,
                    warp: 92,
                },
                description:
                    'Sensory deprivation at light speed. Minimal starfield spirals inward through absolute darkness.',
                name: 'Void Meditation',
            },
            {
                controls: {
                    density: 160,
                    palette: 'Warp Core',
                    speed: 10,
                    streak: 160,
                    warp: 100,
                },
                description:
                    'Containment failure. The tunnel collapses into an overdrive vortex of classic blue-white streaks.',
                name: 'Warp Core Breach',
            },
            {
                controls: {
                    density: 85,
                    palette: 'Classic',
                    speed: 4,
                    streak: 110,
                    warp: 55,
                },
                description:
                    "Lean back in the captain's chair and watch the Milky Way unspool. Steady cruise through a blue-white star corridor.",
                name: 'Starliner Cruise',
            },
            {
                controls: {
                    density: 6,
                    palette: 'Solar Wind',
                    speed: 2,
                    streak: 25,
                    warp: 10,
                },
                description: 'A lone amber filament drifts across an empty sky. The last photon escaping a dying sun.',
                name: 'Last Light of Andromeda',
            },
        ],
    },
)
