import { color, combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'Deep Current',
    shader,
    {
        bgColor: color('Background', '#050510', { group: 'Color' }),
        blend: num('Collision Width', [0, 100], 20, { group: 'Color' }),
        direction: combo('Flow Direction', ['Diagonal', 'Horizontal', 'Vertical'], { group: 'Scene' }),
        flow: num('Flow Strength', [0, 100], 65, { group: 'Motion' }),
        leftColor: color('Left Color', '#ff4fb4', { group: 'Color' }),
        rightColor: color('Right Color', '#ff9a3d', { group: 'Color' }),
        speed: num('Speed', [1, 10], 4, { group: 'Motion' }),
        turbulence: num('Turbulence', [0, 100], 60, { group: 'Motion' }),
    },
    {
        description:
            'Two opposing fluid currents collide — domain-warped color fields push against each other across a turbulent, ever-shifting boundary',
        presets: [
            {
                controls: {
                    bgColor: '#0c0208',
                    blend: 12,
                    direction: 'Vertical',
                    flow: 85,
                    leftColor: '#ff6a00',
                    rightColor: '#0a1a5f',
                    speed: 3,
                    turbulence: 78,
                },
                description:
                    'Superheated amber plumes erupt through deep-ocean indigo — the faintly volcanic background radiates residual heat between the crests',
                name: 'Hydrothermal Vent',
            },
            {
                controls: {
                    bgColor: '#060818',
                    blend: 60,
                    direction: 'Diagonal',
                    flow: 35,
                    leftColor: '#00ffcc',
                    rightColor: '#3d0028',
                    speed: 1.5,
                    turbulence: 30,
                },
                description:
                    'Ghostly teal drifts through a deep bioluminescent haze — dark rose pulses from the opposite direction, the two barely touching',
                name: 'Phantom Reef',
            },
            {
                controls: {
                    bgColor: '#140400',
                    blend: 8,
                    direction: 'Horizontal',
                    flow: 95,
                    leftColor: '#fffce0',
                    rightColor: '#ff1a00',
                    speed: 8,
                    turbulence: 92,
                },
                description:
                    'White-hot plasma slams into coronal red — the dark corona background swallows everything between the razor-thin collision filaments',
                name: 'Solar Flare',
            },
            {
                controls: {
                    bgColor: '#08101e',
                    blend: 75,
                    direction: 'Horizontal',
                    flow: 25,
                    leftColor: '#e0f0ff',
                    rightColor: '#40ffd0',
                    speed: 1,
                    turbulence: 18,
                },
                description:
                    'Tectonic ice sheets grind in slow motion — the steel-blue deep fills the gaps with polar cold as glacial white meets pale cyan',
                name: 'Glacial Crush',
            },
            {
                controls: {
                    bgColor: '#060c04',
                    blend: 35,
                    direction: 'Diagonal',
                    flow: 75,
                    leftColor: '#aaff00',
                    rightColor: '#5a0080',
                    speed: 5.5,
                    turbulence: 80,
                },
                description:
                    'Acid chartreuse boils against bruised violet — the murky swamp background seeps through as toxic currents churn diagonally',
                name: "Witch's Cauldron",
            },
            {
                controls: {
                    bgColor: '#0c0020',
                    blend: 20,
                    direction: 'Vertical',
                    flow: 80,
                    leftColor: '#e135ff',
                    rightColor: '#80ffea',
                    speed: 6.5,
                    turbulence: 72,
                },
                description:
                    'A dimensional tear rips open — SilkCircuit violet and neon cyan pour from opposite ends, the void-purple between them crackling with interference',
                name: 'Rift Gate',
            },
            {
                controls: {
                    bgColor: '#100006',
                    blend: 85,
                    direction: 'Horizontal',
                    flow: 20,
                    leftColor: '#d8d0f0',
                    rightColor: '#6a0020',
                    speed: 1,
                    turbulence: 15,
                },
                description:
                    'Pale moonlight silver meets deep oxblood in a ritual tide — the crimson-black abyss breathes between enormous slow-moving folds',
                name: 'Blood Moon Tide',
            },
            {
                controls: {
                    bgColor: '#020004',
                    blend: 5,
                    direction: 'Diagonal',
                    flow: 90,
                    leftColor: '#080810',
                    rightColor: '#5500cc',
                    speed: 9,
                    turbulence: 95,
                },
                description:
                    'Near-black mass devours ultraviolet plasma — the boundary barely exists, just violent fractures of purple light tearing through absolute nothing',
                name: 'Abyssal Maw',
            },
        ],
    },
)
