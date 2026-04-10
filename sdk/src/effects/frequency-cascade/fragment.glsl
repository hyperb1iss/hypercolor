#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

// Audio — only the stable, smoothed channels drive temporal motion
uniform float iAudioLevel;        // smoothed overall level
uniform float iAudioBass;         // raw bass band
uniform float iAudioMid;          // raw mid band
uniform float iAudioTreble;       // raw treble band
uniform float iAudioBeatPulse;    // decaying beat impulse
uniform float iAudioSwell;        // positive swell envelope
uniform float iAudioBrightness;   // spectral centroid 0..1
uniform float iAudioHarmonicHue;  // harmonic color 0..360

uniform float iCascadeLevel;
uniform float iCascadeBass;
uniform float iCascadeMid;
uniform float iCascadeTreble;
uniform float iCascadeSwell;
uniform float iCascadePresence;
uniform float iCascadeBeatBloom;
uniform float iCascadeFloor;

// Controls
uniform float iSpeed;
uniform float iIntensity;
uniform float iSmoothing;
uniform float iBarWidth;
uniform float iGlow;
uniform int iPalette;
uniform int iScene;

// ── Smooth value noise ──────────────────────────────────────────────
// No floor(time * N) patterns anywhere. All motion is continuous.

float hash12(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

float valueNoise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    float a = hash12(i);
    float b = hash12(i + vec2(1.0, 0.0));
    float c = hash12(i + vec2(0.0, 1.0));
    float d = hash12(i + vec2(1.0, 1.0));
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

float fbm(vec2 p) {
    float v = 0.0;
    float a = 0.5;
    for (int i = 0; i < 4; i++) {
        v += valueNoise(p) * a;
        p *= 2.03;
        a *= 0.5;
    }
    return v;
}

// ── iq cosine palettes ──────────────────────────────────────────────

vec3 iqPalette(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    return a + b * cos(6.28318 * (c * t + d));
}

vec3 paletteColor(float t, int id) {
    if (id == 0) return iqPalette(t, vec3(0.50, 0.28, 0.54), vec3(0.48, 0.45, 0.45), vec3(1.00, 0.85, 0.70), vec3(0.88, 0.18, 0.52));
    if (id == 1) return iqPalette(t, vec3(0.18, 0.50, 0.40), vec3(0.35, 0.40, 0.45), vec3(0.75, 0.70, 0.85), vec3(0.62, 0.30, 0.72));
    if (id == 2) return iqPalette(t, vec3(0.52, 0.18, 0.48), vec3(0.52, 0.45, 0.50), vec3(1.00, 1.00, 1.00), vec3(0.84, 0.10, 0.60));
    if (id == 3) return iqPalette(t, vec3(0.50, 0.22, 0.02), vec3(0.50, 0.40, 0.20), vec3(1.00, 0.72, 0.38), vec3(0.02, 0.16, 0.24));
    if (id == 4) return iqPalette(t, vec3(0.40, 0.16, 0.28), vec3(0.40, 0.26, 0.28), vec3(0.82, 0.68, 0.60), vec3(0.06, 0.24, 0.44));
    if (id == 5) return iqPalette(t, vec3(0.52, 0.60, 0.78), vec3(0.22, 0.30, 0.22), vec3(0.62, 0.82, 1.00), vec3(0.00, 0.10, 0.32));
    return iqPalette(t, vec3(0.50), vec3(0.50), vec3(1.00), vec3(0.00));
}

// ── Spectrum shaping ────────────────────────────────────────────────
// Treats bass/mid/treble as *spatial* shape, not per-bar energy.
// Temporal stability comes from effect-local smoothed envelopes.

// smoothAmt passed in so the Smoothing control governs temporal responsiveness
float spectralEnvelope(float freq, float smoothAmt) {
    float bassLobe  = exp(-pow((freq - 0.08) * 2.55, 2.0));
    float lowLobe   = exp(-pow((freq - 0.28) * 2.85, 2.0));
    float midLobe   = exp(-pow((freq - 0.52) * 2.95, 2.0));
    float highLobe  = exp(-pow((freq - 0.76) * 2.85, 2.0));
    float airLobe   = exp(-pow((freq - 0.94) * 3.20, 2.0));

    // Temper raw bands toward smoothed level — prevents beat transients from
    // spiking bar height. Raw bands still shape which frequency region lights up.
    float bassDamp = mix(0.24, 0.54, smoothAmt);
    float bandDamp = mix(0.16, 0.42, smoothAmt);
    float bass   = mix(iCascadeBass,   iCascadeLevel, bassDamp);
    float mid    = mix(iCascadeMid,    iCascadeLevel, bandDamp);
    float treble = mix(iCascadeTreble, iCascadeLevel, bandDamp);

    float lowMix  = mix(bass, mid, 0.40);
    float highMix = mix(mid, treble, 0.55);

    return
        bass   * bassLobe  * 1.05 +
        lowMix * lowLobe   * 0.92 +
        mid    * midLobe   * 1.00 +
        highMix* highLobe  * 0.95 +
        treble * airLobe   * 0.82;
}

float barEnergy(float freq, float barId, float time, float smoothAmt) {
    float spectrum = spectralEnvelope(freq, smoothAmt);

    // Neighbor-blur in the frequency domain — sample envelope at neighbors
    // and blend in according to smoothing amount. Cheap, smoothstep-clean.
    float freqStep = 1.0 / 84.0;
    float neighborLeft  = spectralEnvelope(clamp(freq - freqStep, 0.0, 1.0), smoothAmt);
    float neighborRight = spectralEnvelope(clamp(freq + freqStep, 0.0, 1.0), smoothAmt);
    float smoothed = (spectrum * 2.0 + neighborLeft + neighborRight) * 0.25;
    spectrum = mix(spectrum, smoothed, smoothAmt);

    float breath = iCascadePresence * 0.10 + iCascadeSwell * 0.06;
    float organicTime = time * mix(0.42, 0.16, smoothAmt);
    float organic = fbm(vec2(freq * 3.2 + barId * 0.035, organicTime)) - 0.5;

    float energy = spectrum * (0.86 + iCascadePresence * 0.22);
    energy += breath * 0.22;
    energy += organic * (0.10 + iCascadePresence * 0.08);

    float floor_ = iCascadeFloor + organic * 0.04;
    energy = max(energy, floor_);
    energy = 1.18 * (1.0 - exp(-energy * 1.35));

    return clamp(energy, 0.0, 1.18);
}

// ── Scene geometry ──────────────────────────────────────────────────
// Each scene produces:
//   visualY  — distance from the bar base (0 at base, >1 past the top)
//   baseTint — brightness of the scene backdrop at this pixel

struct Scene {
    float visualY;
    float lane;      // 0..1 horizontal lane for bar layout
    float backdrop;  // backdrop intensity multiplier
    float mirror;    // extra dim for mirrored/reflection regions
};

Scene sceneCascade(vec2 uv) {
    Scene s;
    s.visualY = uv.y;
    s.lane = uv.x;
    s.backdrop = pow(uv.y, 1.25);
    s.mirror = 1.0;
    return s;
}

Scene sceneMirror(vec2 uv) {
    Scene s;
    s.visualY = abs(uv.y - 0.5) * 2.0;
    s.lane = uv.x;
    s.backdrop = 1.0 - abs(uv.y - 0.5) * 1.4;
    s.backdrop = clamp(s.backdrop, 0.0, 1.0);
    s.mirror = 1.0;
    return s;
}

Scene sceneHorizon(vec2 uv) {
    const float horizon = 0.42;
    Scene s;
    if (uv.y >= horizon) {
        s.visualY = (uv.y - horizon) / (1.0 - horizon);
        s.mirror = 1.0;
    } else {
        // Reflection falls off into darkness
        float depth = (horizon - uv.y) / horizon;
        s.visualY = depth * 0.85;
        s.mirror = (1.0 - depth) * 0.55;
    }
    s.lane = uv.x;
    s.backdrop = smoothstep(horizon - 0.12, 1.0, uv.y);
    return s;
}

Scene sceneTunnel(vec2 uv, float aspect, float time) {
    Scene s;
    vec2 p = uv - 0.5;
    p.x *= aspect;
    float r = length(p);
    float ang = atan(p.y, p.x);
    s.lane = fract(ang / 6.28318 + 0.5 + time * 0.012);
    s.visualY = clamp((r - 0.08) / 0.42, 0.0, 1.0); // bars radiate outward
    s.backdrop = smoothstep(0.65, 0.08, r);
    s.mirror = 1.0;
    return s;
}

// ── Main ────────────────────────────────────────────────────────────

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution;
    float aspect = iResolution.x / max(iResolution.y, 1.0);

    float speed = max(iSpeed, 0.05);
    float time = iTime * (0.22 + speed * 0.32);
    float intensity = clamp(iIntensity * 0.01, 0.0, 1.0);
    float smoothAmt = clamp(iSmoothing * 0.01, 0.0, 1.0);
    float glow = clamp(iGlow * 0.01, 0.0, 1.0);

    Scene scn;
    if (iScene == 0) scn = sceneCascade(uv);
    else if (iScene == 1) scn = sceneMirror(uv);
    else if (iScene == 2) scn = sceneHorizon(uv);
    else scn = sceneTunnel(uv, aspect, time);

    // ── Bar layout ──────────────────────────────────────────────────
    float barCount = floor(mix(84.0, 18.0, clamp(iBarWidth * 0.01, 0.0, 1.0)));
    float barPos = scn.lane * barCount;
    float barId = floor(barPos);
    float barCell = fract(barPos);
    float freq = barId / max(barCount - 1.0, 1.0);

    // Softer bar edges — wider gap at low glow, tighter at high glow
    float halfGap = mix(0.09, 0.035, glow);
    float barMask =
        smoothstep(halfGap, halfGap + 0.065, barCell) *
        smoothstep(halfGap, halfGap + 0.065, 1.0 - barCell);

    // ── Energy ──────────────────────────────────────────────────────
    float energy = barEnergy(freq, barId, time, smoothAmt);

    energy = clamp(energy, 0.0, 1.75);

    // ── Bar height ──────────────────────────────────────────────────
    float baseline = 0.04 + iCascadeFloor * 0.12;
    float heightCurve = 1.0 - exp(-energy * mix(1.05, 1.75, intensity));
    float barHeight = clamp(baseline + heightCurve * mix(0.24, 0.78, intensity), 0.05, 0.92);

    // ── Bar body + edges ────────────────────────────────────────────
    float barTop = 1.0 - smoothstep(barHeight - 0.018, barHeight + 0.006, scn.visualY);
    float bar = barTop * barMask;

    // LED segmentation — static in Y, no time scroll
    float ledRows = mix(24.0, 10.0, smoothAmt);
    float ledCell = fract(scn.visualY * ledRows);
    float ledMask =
        smoothstep(0.02, 0.18, ledCell) *
        smoothstep(0.02, 0.18, 1.0 - ledCell);
    bar *= mix(0.82, 1.0, ledMask);

    // ── Haze above the bar (fake waterfall — spatial, not temporal) ─
    float above = max(scn.visualY - barHeight, 0.0);
    float hazeDecay = mix(30.0, 10.0, smoothAmt);
    float haze = exp(-above * hazeDecay);
    haze *= (1.0 - barTop) * barMask;
    haze *= 0.16 + 0.56 * energy;

    // ── Rim + beam glow ─────────────────────────────────────────────
    float rim = exp(-abs(scn.visualY - barHeight) * mix(85.0, 28.0, glow));
    float beam = exp(-pow((barCell - 0.5) * 2.1, 2.0) * mix(24.0, 7.0, glow));
    // Beat pulse flares the bloom, not bar height — ripple of light, not a heave
    float beatFlare = iCascadeBeatBloom * (0.26 + glow * 0.24);
    float bloom = (rim * 0.78 + haze * 0.48) * beam * (0.18 + glow * 1.02 + beatFlare * 0.24);

    // ── Color ───────────────────────────────────────────────────────
    // Palette walks with frequency so neighbors are chromatically related.
    float paletteT =
        freq * 0.75 +
        time * 0.010 +
        iAudioHarmonicHue * 0.0006 +
        (iAudioBrightness - 0.5) * 0.18;

    vec3 baseColor = paletteColor(paletteT, iPalette);
    vec3 accentColor = paletteColor(paletteT + 0.22, iPalette);
    vec3 peakColor = paletteColor(paletteT + 0.42, iPalette);

    // Deep, calm background — single smooth gradient, driven by scene backdrop
    vec3 bgLow = paletteColor(0.05 + time * 0.004, iPalette) * 0.04;
    vec3 bgHigh = paletteColor(0.46, iPalette) * 0.11;
    vec3 color = mix(bgLow, bgHigh, scn.backdrop);

    // Bar body — peaks warm toward peakColor for emphasis
    float peakBlend = smoothstep(0.45, 1.15, energy);
    vec3 barTint = mix(baseColor, peakColor, peakBlend);
    color += barTint * bar * (0.38 + energy * 1.35) * (0.55 + intensity * 1.45) * scn.mirror;

    // Rim glow — beat pulse weighted toward bass frequencies (1.0 at freq=0, falls off at highs)
    float beatRim = beatFlare * (1.0 - freq * 0.70);
    color += accentColor * rim * barMask * (0.16 + intensity * 0.88 + beatRim * 0.36) * scn.mirror;

    // Haze trail tint
    color += mix(baseColor, accentColor, 0.55) * haze * (0.30 + intensity * 0.80) * scn.mirror;

    // Bloom — the widest visual spread
    color += accentColor * bloom * scn.mirror;

    // ── Scene accents ───────────────────────────────────────────────
    if (iScene == 1) {
        // Mirror: center line glow
        float horizonLine = exp(-abs(uv.y - 0.5) * 52.0);
        color += mix(baseColor, accentColor, 0.5) *
                 horizonLine * (0.16 + iCascadePresence * 0.36 + iCascadeBeatBloom * 0.08);
    } else if (iScene == 2) {
        // Horizon: horizon line flare
        float horizonLine = exp(-abs(uv.y - 0.42) * 60.0);
        color += mix(accentColor, peakColor, 0.5) *
                 horizonLine * (0.18 + iCascadePresence * 0.38 + iCascadeBeatBloom * 0.10);
    } else if (iScene == 3) {
        // Tunnel: soft radial core
        vec2 cp = (uv - 0.5) * vec2(aspect, 1.0);
        float core = exp(-length(cp) * 4.5);
        color += accentColor * core * (0.26 + iCascadePresence * 0.42 + iCascadeBeatBloom * 0.08);
    }

    // ── Vignette ────────────────────────────────────────────────────
    vec2 centered = (uv - 0.5) * vec2(aspect, 1.0);
    float vignette = 1.0 - smoothstep(0.34, 1.18, length(centered));
    color *= vignette;

    // ── Tonemap + gamma ─────────────────────────────────────────────
    // Reinhard-extended: bright-but-never-clipped rolloff.
    color = color / (1.0 + color * 0.34);
    color = pow(max(color, 0.0), vec3(0.92));

    fragColor = vec4(clamp(color, 0.0, 1.0), 1.0);
}
