import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'Arc Storm',
    shader,
    {
        branches: num('Branching', [0, 100], 68, {
            group: 'Geometry',
            step: 1,
            tooltip: 'How much the arcs split and web outward.',
        }),
        density: num('Arc Density', [0, 100], 50, {
            group: 'Geometry',
            step: 1,
            tooltip:
                'How much of the electric field resolves into visible arcs. Sparse clean bolts → dense crackling web.',
        }),
        flicker: num('Instability', [0, 100], 30, {
            group: 'Motion',
            step: 1,
            tooltip:
                'Traveling discharge pulses that zip along arcs — higher values mean deeper, more frequent intensity ripples.',
        }),
        intensity: num('Core Heat', [0, 100], 72, {
            group: 'Atmosphere',
            step: 1,
            tooltip: 'Brightness and white-hot core strength.',
        }),
        palette: combo(
            'Theme',
            ['Crimson Arc', 'Electric', 'Frozen', 'Phantom', 'Rosewire', 'SilkCircuit Storm', 'Solar Surge', 'Toxic'],
            {
                default: 'SilkCircuit Storm',
                group: 'Atmosphere',
                tooltip:
                    'Select the discharge palette. Each theme now drives the outer glow, contrast veins, accent arcs, and core tint.',
            },
        ),
        prismatic: num('Prismatic', [0, 100], 12, {
            group: 'Atmosphere',
            step: 1,
            tooltip:
                'Chromatic refraction splitting along the arcs — rainbow fringing that slowly drifts around the tendrils.',
        }),
        speed: num('Charge Rate', [1, 10], 5, {
            group: 'Motion',
            step: 0.5,
            tooltip: 'Overall motion speed of the discharge field.',
        }),
    },
    {
        description:
            'Unleash fractal lightning across a high-voltage field — white-hot cores split into chromatic tendrils that web and crackle through the dark',
        presets: [
            {
                controls: {
                    branches: 85,
                    density: 55,
                    flicker: 22,
                    intensity: 88,
                    palette: 'SilkCircuit Storm',
                    prismatic: 20,
                    speed: 4,
                },
                description:
                    'A decommissioned lab at midnight — phantom arcs still crawling the cage, ozone thick enough to taste, purple discharge painting the walls',
                name: 'Tesla Coil Museum',
            },
            {
                controls: {
                    branches: 55,
                    density: 35,
                    flicker: 50,
                    intensity: 95,
                    palette: 'Crimson Arc',
                    prismatic: 8,
                    speed: 7,
                },
                description:
                    'A defibrillator surge frozen in time — crimson lightning webbing through a chest cavity of dark space, each branch a capillary of pure voltage',
                name: 'Cardiac Arrest',
            },
            {
                controls: {
                    branches: 92,
                    density: 70,
                    flicker: 12,
                    intensity: 60,
                    palette: 'Frozen',
                    prismatic: 35,
                    speed: 2,
                },
                description:
                    'Static discharge in a cryogenic chamber — ice-blue fissures crawling across frozen surfaces, slow and inevitable as glacial time',
                name: 'Permafrost Fracture',
            },
            {
                controls: {
                    branches: 100,
                    density: 85,
                    flicker: 65,
                    intensity: 100,
                    palette: 'Solar Surge',
                    prismatic: 45,
                    speed: 10,
                },
                description:
                    'Containment breach at the solar forge — plasma tendrils lashing through ruptured conduits, the core going white-hot and unstoppable',
                name: 'Reactor Meltdown',
            },
            {
                controls: {
                    branches: 38,
                    density: 25,
                    flicker: 35,
                    intensity: 35,
                    palette: 'Phantom',
                    prismatic: 55,
                    speed: 3,
                },
                description:
                    'A dead motherboard dreaming of electricity — faint spectral discharges tracing forgotten pathways through silicon that will never wake',
                name: 'Phantom Circuit',
            },
            {
                controls: {
                    branches: 72,
                    density: 60,
                    flicker: 45,
                    intensity: 50,
                    palette: 'Rosewire',
                    prismatic: 28,
                    speed: 3.5,
                },
                description:
                    'Voltage bleeds through rose quartz veins in a cathedral wall — pink lightning illuminates stained glass nerves that pulse with devotion',
                name: 'Stained Glass Discharge',
            },
            {
                controls: {
                    branches: 15,
                    density: 10,
                    flicker: 8,
                    intensity: 78,
                    palette: 'Electric',
                    prismatic: 70,
                    speed: 1.5,
                },
                description:
                    'A single arc hangs suspended in vacuum — prismatic halos bloom around its white-hot spine like light through a prism in zero gravity',
                name: 'Lonely Filament',
            },
            {
                controls: {
                    branches: 65,
                    density: 45,
                    flicker: 80,
                    intensity: 82,
                    palette: 'Toxic',
                    prismatic: 15,
                    speed: 8,
                },
                description:
                    'Radioactive discharge cascades through a ruptured cooling tower — acid-green arcs stutter and snap across contaminated steel',
                name: 'Chernobyl Fireflies',
            },
        ],
    },
)
