import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'Starfall',
    shader,
    {
        density: num('Density', [0, 100], 50, { group: 'Atmosphere' }),
        palette: combo('Palette', ['Aurora Rain', 'Celestial', 'Cosmic', 'Ember Fall', 'Frozen Tears', 'Neon Rain'], {
            group: 'Color',
        }),
        tailMode: combo('Tail Mode', ['Palette', 'Rainbow', 'Ghostly', 'Electric'], {
            group: 'Color',
        }),
        sparkle: num('Sparkle', [0, 100], 30, { group: 'Color' }),
        speed: num('Speed', [1, 10], 5, { group: 'Motion' }),
        angle: num('Angle', [-60, 60], 0, {
            group: 'Motion',
            tooltip: 'Fall angle in degrees — negative drifts left, positive drifts right',
        }),
        size: num('Size', [0, 100], 30, { group: 'Shape', tooltip: 'Star size — larger values create bold meteors' }),
        trails: num('Trails', [0, 100], 60, { group: 'Atmosphere' }),
    },
    {
        description:
            'Luminous particles cascade through infinite black — each one trailing a comet glow as it falls through the dark',
        presets: [
            {
                controls: {
                    density: 85,
                    palette: 'Cosmic',
                    tailMode: 'Palette',
                    sparkle: 60,
                    speed: 8,
                    angle: 0,
                    size: 30,
                    trails: 90,
                },
                description: 'Peak Perseid night in the high desert — blazing streaks tear across a moonless sky',
                name: 'Meteor Shower',
            },
            {
                controls: {
                    density: 40,
                    palette: 'Frozen Tears',
                    tailMode: 'Palette',
                    sparkle: 90,
                    speed: 2,
                    angle: 0,
                    size: 20,
                    trails: 75,
                },
                description:
                    'Ice crystals fall through the stratosphere — impossibly slow, catching starlight as they descend',
                name: 'Frozen Requiem',
            },
            {
                controls: {
                    density: 100,
                    palette: 'Neon Rain',
                    tailMode: 'Palette',
                    sparkle: 20,
                    speed: 9,
                    angle: -15,
                    size: 15,
                    trails: 45,
                },
                description: 'Rain hammers a Tokyo alley at 3AM — every droplet carries the glow of a thousand signs',
                name: 'Neon Monsoon',
            },
            {
                controls: {
                    density: 25,
                    palette: 'Celestial',
                    tailMode: 'Palette',
                    sparkle: 70,
                    speed: 3,
                    angle: 0,
                    size: 45,
                    trails: 95,
                },
                description: 'Constellation fragments cross the meridian in silence — stately, ancient, unhurried',
                name: 'Celestial Procession',
            },
            {
                controls: {
                    density: 60,
                    palette: 'Ember Fall',
                    tailMode: 'Palette',
                    sparkle: 40,
                    speed: 5,
                    angle: 10,
                    size: 35,
                    trails: 55,
                },
                description: 'Campfire sparks lift into a cold mountain night, then fall back to earth as dying light',
                name: 'Ember Descent',
            },
            {
                controls: {
                    density: 10,
                    palette: 'Aurora Rain',
                    tailMode: 'Palette',
                    sparkle: 100,
                    speed: 1,
                    angle: 0,
                    size: 60,
                    trails: 100,
                },
                description:
                    'A single luminous thread unspools from the aurora — one solitary particle descends forever, dragging the entire sky behind it',
                name: 'Last Light of Thule',
            },
            {
                controls: {
                    density: 100,
                    palette: 'Cosmic',
                    tailMode: 'Palette',
                    sparkle: 0,
                    speed: 10,
                    angle: 0,
                    size: 10,
                    trails: 0,
                },
                description:
                    'The sky collapses — a hundred thousand particles plummet without trails, pure velocity, a white-noise waterfall of falling stars',
                name: 'Extinction Event',
            },
            // ── New presets showcasing angle, size, and tail modes ──
            {
                controls: {
                    density: 50,
                    palette: 'Celestial',
                    tailMode: 'Rainbow',
                    sparkle: 50,
                    speed: 4,
                    angle: -25,
                    size: 50,
                    trails: 80,
                },
                description:
                    'Prismatic streaks cut diagonally through the void — each trail a spectrum unraveling behind it',
                name: 'Prism Wind',
            },
            {
                controls: {
                    density: 30,
                    palette: 'Frozen Tears',
                    tailMode: 'Ghostly',
                    sparkle: 80,
                    speed: 2,
                    angle: 5,
                    size: 55,
                    trails: 90,
                },
                description:
                    'Spectral wisps drift through a frozen cathedral — barely there, flickering at the edge of perception',
                name: 'Phantom Veil',
            },
            {
                controls: {
                    density: 70,
                    palette: 'Neon Rain',
                    tailMode: 'Electric',
                    sparkle: 35,
                    speed: 7,
                    angle: 0,
                    size: 25,
                    trails: 50,
                },
                description:
                    'High-voltage discharge rains from a shattered grid — every particle crackles with raw energy',
                name: 'Arc Storm',
            },
            {
                controls: {
                    density: 15,
                    palette: 'Cosmic',
                    tailMode: 'Palette',
                    sparkle: 45,
                    speed: 3,
                    angle: 0,
                    size: 100,
                    trails: 85,
                },
                description:
                    'Massive golden fireballs descend in slow motion — ancient light burning through the atmosphere',
                name: 'Bolide',
            },
            {
                controls: {
                    density: 80,
                    palette: 'Aurora Rain',
                    tailMode: 'Rainbow',
                    sparkle: 60,
                    speed: 6,
                    angle: 35,
                    size: 20,
                    trails: 70,
                },
                description:
                    'A diagonal deluge of chromatic rain — the aurora shattered into a thousand falling shards',
                name: 'Shattered Aurora',
            },
            {
                controls: {
                    density: 45,
                    palette: 'Ember Fall',
                    tailMode: 'Electric',
                    sparkle: 70,
                    speed: 5,
                    angle: -40,
                    size: 40,
                    trails: 60,
                },
                description:
                    'Windblown sparks streak sideways from an unseen forge — crackling orange veins against the dark',
                name: 'Forge Wind',
            },
        ],
    },
)
