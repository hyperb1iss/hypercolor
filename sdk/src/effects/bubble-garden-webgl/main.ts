import { color, combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

const COLOR_MODES = ['Color Cycle', 'Palette Blend', 'Single Color', 'Triad'] as const
const THEMES = [
    'Bubblegum',
    'Citrus Pop',
    'Custom',
    'Cyber Pop',
    'Jellyfish',
    'Lagoon',
    'Lavender Fizz',
    'Neon Soda',
] as const

const controls = {
    bgColor: color('Background', '#05040a', { group: 'Scene' }),
    color: color('Color', '#ff4f9a', { group: 'Color' }),
    color2: color('Color 2', '#80ffea', { group: 'Color' }),
    color3: color('Color 3', '#6f2dff', { group: 'Color' }),
    colorMode: combo('Color Mode', COLOR_MODES, {
        default: 'Palette Blend',
        group: 'Color',
    }),
    count: num('Count', [10, 120], 30, { group: 'Scene' }),
    size: num('Size', [1, 10], 5, { group: 'Scene' }),
    speed: num('Speed', [0, 100], 10, { group: 'Motion' }),
    theme: combo('Theme', THEMES, { default: 'Cyber Pop', group: 'Color' }),
}

export default effect('Bubble Garden WebGL', shader, controls, {
    description:
        'Drift through a luminous bubble field. Glossy shader spheres rise with colored rims, catching light as they float, collide, and shimmer.',
    presets: [
        {
            controls: {
                bgColor: '#020108',
                color: '#8a7cff',
                color2: '#ff7fcf',
                color3: '#76fff1',
                colorMode: 'Palette Blend',
                count: 65,
                size: 7,
                speed: 8,
                theme: 'Jellyfish',
            },
            description:
                'Colonial organisms drift in eternal darkness; each translucent bell a separate creature chained in bioluminescent congress.',
            name: 'Bathypelagic Siphonophore',
        },
        {
            controls: {
                bgColor: '#0e0800',
                color: '#ffcc33',
                color2: '#ff7a2f',
                color3: '#ff5478',
                colorMode: 'Single Color',
                count: 120,
                size: 3,
                speed: 45,
                theme: 'Citrus Pop',
            },
            description:
                'Golden effervescence erupts from the bottle. A billion tiny spheres race upward through amber light.',
            name: 'Champagne Supernova',
        },
        {
            controls: {
                bgColor: '#040a02',
                color: '#36ff9a',
                color2: '#18e4ff',
                color3: '#ff4ed1',
                colorMode: 'Triad',
                count: 42,
                size: 6,
                speed: 18,
                theme: 'Neon Soda',
            },
            description:
                'Chemical bubbles surface through contaminated sediment, each one a pressurized capsule of fluorescent mutation.',
            name: 'Toxic Waste Lagoon',
        },
        {
            controls: {
                bgColor: '#08060e',
                color: '#9f72ff',
                color2: '#ff5ec8',
                color3: '#66d4ff',
                colorMode: 'Color Cycle',
                count: 22,
                size: 9,
                speed: 5,
                theme: 'Lavender Fizz',
            },
            description:
                'Razor-thin membranes refract white light into impossible rainbows. Each bubble, a floating physics experiment.',
            name: 'Soap Film Interference',
        },
        {
            controls: {
                bgColor: '#0a0208',
                color: '#ff4f9a',
                color2: '#ff74c5',
                color3: '#8a5cff',
                colorMode: 'Palette Blend',
                count: 85,
                size: 4,
                speed: 12,
                theme: 'Bubblegum',
            },
            description:
                'Endosomes shuttle through cellular fluid, lipid bilayer spheres ferrying molecular cargo in warm biological pink.',
            name: 'Cytoplasmic Vesicle Transport',
        },
        {
            controls: {
                bgColor: '#000810',
                color: '#46f1dc',
                color2: '#5da8ff',
                color3: '#1746ff',
                colorMode: 'Triad',
                count: 18,
                size: 10,
                speed: 3,
                theme: 'Lagoon',
            },
            description:
                'Ancient glass fishing floats drift in a midnight cove. Massive teal orbs bob on black water, each one holding a trapped sunrise.',
            name: 'Moonlit Glass Floats',
        },
        {
            controls: {
                bgColor: '#02050a',
                color: '#08f7fe',
                color2: '#ff06b5',
                color3: '#6f2dff',
                colorMode: 'Color Cycle',
                count: 100,
                size: 2,
                speed: 65,
                theme: 'Cyber Pop',
            },
            description:
                'Particle accelerator collision event. A hundred luminous fragments scatter from the impact point in cyan, magenta, and ultraviolet.',
            name: 'Hadron Splash',
        },
    ],
})
