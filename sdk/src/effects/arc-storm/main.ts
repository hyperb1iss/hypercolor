import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'Arc Storm',
    shader,
    {
        branches: num('Branching', [0, 100], 68, {
            step: 1,
            tooltip: 'How much the arcs split and web outward.',
        }),
        flicker: num('Instability', [0, 100], 56, {
            step: 1,
            tooltip: 'Electrical jitter, flash activity, and discharge chatter.',
        }),
        intensity: num('Core Heat', [0, 100], 72, {
            step: 1,
            tooltip: 'Brightness and white-hot core strength.',
        }),
        palette: combo(
            'Theme',
            ['Crimson Arc', 'Electric', 'Frozen', 'Phantom', 'Rosewire', 'SilkCircuit Storm', 'Solar Surge', 'Toxic'],
            {
                default: 'SilkCircuit Storm',
                tooltip:
                    'Select the discharge palette. Each theme now drives the outer glow, contrast veins, accent arcs, and core tint.',
            },
        ),
        speed: num('Charge Rate', [1, 10], 5, {
            step: 0.5,
            tooltip: 'Overall motion speed of the discharge field.',
        }),
    },
    {
        description:
            'High-voltage fractal lightning with chromatic cores, contrast-woven gradients, and a showcase house default',
        presets: [
            {
                name: 'Tesla Coil Museum',
                description: 'A decommissioned lab at midnight — phantom arcs still crawling the cage, ozone thick enough to taste, purple discharge painting the walls',
                controls: {
                    branches: 85,
                    flicker: 42,
                    intensity: 88,
                    palette: 'SilkCircuit Storm',
                    speed: 4,
                },
            },
            {
                name: 'Cardiac Arrest',
                description: 'A defibrillator surge frozen in time — crimson lightning webbing through a chest cavity of dark space, each branch a capillary of pure voltage',
                controls: {
                    branches: 55,
                    flicker: 78,
                    intensity: 95,
                    palette: 'Crimson Arc',
                    speed: 7,
                },
            },
            {
                name: 'Permafrost Fracture',
                description: 'Static discharge in a cryogenic chamber — ice-blue fissures crawling across frozen surfaces, slow and inevitable as glacial time',
                controls: {
                    branches: 92,
                    flicker: 18,
                    intensity: 60,
                    palette: 'Frozen',
                    speed: 2,
                },
            },
            {
                name: 'Reactor Meltdown',
                description: 'Containment breach at the solar forge — plasma tendrils lashing through ruptured conduits, the core going white-hot and unstoppable',
                controls: {
                    branches: 100,
                    flicker: 90,
                    intensity: 100,
                    palette: 'Solar Surge',
                    speed: 10,
                },
            },
            {
                name: 'Phantom Circuit',
                description: 'A dead motherboard dreaming of electricity — faint spectral discharges tracing forgotten pathways through silicon that will never wake',
                controls: {
                    branches: 38,
                    flicker: 65,
                    intensity: 35,
                    palette: 'Phantom',
                    speed: 3,
                },
            },
        ],
    },
)
