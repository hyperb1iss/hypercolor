  if (typeof window.__hypercolorApplyFramePayload !== 'function') {
    const finiteNumber = function(value, fallback) {
      const number = Number(value);
      return Number.isFinite(number) ? number : fallback;
    };
    const trueObject = function(values) {
      const object = {};
      if (!Array.isArray(values)) { return object; }
      for (let index = 0; index < values.length; index += 1) {
        const value = values[index];
        if (typeof value === 'string' && value.length > 0) { object[value] = true; }
      }
      return object;
    };
    const assignFloat32Array = function(current, values) {
      if (!Array.isArray(values)) { return current instanceof Float32Array ? current : new Float32Array(0); }
      const array = current instanceof Float32Array && current.length === values.length ? current : new Float32Array(values.length);
      for (let index = 0; index < values.length; index += 1) { array[index] = finiteNumber(values[index], 0); }
      return array;
    };
    const assignInt8Array = function(current, values) {
      if (!Array.isArray(values)) { return current instanceof Int8Array ? current : new Int8Array(0); }
      const array = current instanceof Int8Array && current.length === values.length ? current : new Int8Array(values.length);
      for (let index = 0; index < values.length; index += 1) { array[index] = finiteNumber(values[index], 0); }
      return array;
    };
    const assignInt16Array = function(current, values) {
      if (!Array.isArray(values)) { return current instanceof Int16Array ? current : new Int16Array(0); }
      const array = current instanceof Int16Array && current.length === values.length ? current : new Int16Array(values.length);
      for (let index = 0; index < values.length; index += 1) { array[index] = finiteNumber(values[index], 0); }
      return array;
    };
    const applyAudio = function(engine, audio) {
      if (typeof audio !== 'object' || audio === null) { return; }
      if (typeof engine.audio !== 'object' || engine.audio === null) { engine.audio = {}; }
      engine.audio.level = finiteNumber(audio.levelDb, 0);
      engine.audio.levelRaw = finiteNumber(audio.levelDb, 0);
      engine.audio.levelLinear = finiteNumber(audio.levelLinear, 0);
      engine.audio.levelShort = finiteNumber(audio.levelShort, 0);
      engine.audio.levelLong = finiteNumber(audio.levelLong, 0);
      engine.audio.rms = finiteNumber(audio.rawRms, 0);
      engine.audio.peak = finiteNumber(audio.peak, 0);
      engine.audio.bass = finiteNumber(audio.bass, 0);
      engine.audio.bassEnv = finiteNumber(audio.bassEnv, 0);
      engine.audio.mid = finiteNumber(audio.mid, 0);
      engine.audio.midEnv = finiteNumber(audio.midEnv, 0);
      engine.audio.treble = finiteNumber(audio.treble, 0);
      engine.audio.trebleEnv = finiteNumber(audio.trebleEnv, 0);
      engine.audio.density = finiteNumber(audio.density, 0);
      engine.audio.momentum = finiteNumber(audio.momentum, 0);
      engine.audio.swell = finiteNumber(audio.swell, 0);
      engine.audio.width = finiteNumber(audio.width, 0.5);
      engine.audio.bpm = finiteNumber(audio.bpm, 0);
      engine.audio.tempo = finiteNumber(audio.tempo, 120);
      engine.audio.beat = audio.beat === true;
      engine.audio.beatPulse = finiteNumber(audio.beatPulse, 0);
      engine.audio.beatPhase = finiteNumber(audio.beatPhase, 0);
      engine.audio.beatConfidence = finiteNumber(audio.beatConfidence, 0);
      engine.audio.confidence = engine.audio.beatConfidence;
      engine.audio.onset = audio.onset === true;
      engine.audio.onsetPulse = finiteNumber(audio.onsetPulse, 0);
      engine.audio.spectralFlux = finiteNumber(audio.spectralFlux, 0);
      engine.audio.spectralFluxBands = assignFloat32Array(engine.audio.spectralFluxBands, audio.spectralFluxBands);
      engine.audio.brightness = finiteNumber(audio.brightness, 0.5);
      engine.audio.spread = finiteNumber(audio.spread, 0.3);
      engine.audio.rolloff = finiteNumber(audio.rolloff, 0.5);
      engine.audio.roughness = finiteNumber(audio.roughness, 0.2);
      engine.audio.harmonicHue = finiteNumber(audio.harmonicHue, 0);
      engine.audio.chordMood = finiteNumber(audio.chordMood, 0);
      engine.audio.dominantPitch = finiteNumber(audio.dominantPitch, 0);
      engine.audio.dominantPitchConfidence = finiteNumber(audio.dominantPitchConfidence, 0);
      engine.audio.freq = assignInt8Array(engine.audio.freq, audio.frequencyRaw);
      engine.audio.frequencyRaw = assignInt8Array(engine.audio.frequencyRaw, audio.frequencyRaw);
      engine.audio.frequency = assignFloat32Array(engine.audio.frequency, audio.frequency);
      engine.audio.frequencyWeighted = assignFloat32Array(engine.audio.frequencyWeighted, audio.frequencyWeighted);
      engine.audio.melBands = assignFloat32Array(engine.audio.melBands, audio.melBands);
      engine.audio.melBandsNormalized = assignFloat32Array(engine.audio.melBandsNormalized, audio.melBandsNormalized);
      engine.audio.chromagram = assignFloat32Array(engine.audio.chromagram, audio.chromagram);
    };
    const applyScreen = function(engine, screen) {
      if (typeof screen !== 'object' || screen === null) { return; }
      if (typeof engine.zone !== 'object' || engine.zone === null) { engine.zone = {}; }
      engine.zone.width = finiteNumber(screen.gridWidth, engine.zone.width || 0);
      engine.zone.height = finiteNumber(screen.gridHeight, engine.zone.height || 0);
      engine.zone.hue = assignInt16Array(engine.zone.hue, screen.hue);
      engine.zone.saturation = assignInt8Array(engine.zone.saturation, screen.saturation);
      engine.zone.lightness = assignInt8Array(engine.zone.lightness, screen.lightness);
    };
    const applySensors = function(engine, sensors) {
      if (typeof sensors !== 'object' || sensors === null) { return; }
      const readings = typeof sensors.readings === 'object' && sensors.readings !== null ? sensors.readings : {};
      if (sensors.replaceSensorMap || typeof engine.sensors !== 'object' || engine.sensors === null) { engine.sensors = {}; }
      Object.assign(engine.sensors, readings);
      if (Array.isArray(sensors.sensorList)) { engine.sensorList = sensors.sensorList.slice(); }
    };
    const applyControls = function(controls) {
      if (typeof controls !== 'object' || controls === null) { return; }
      const names = Object.keys(controls);
      if (names.length === 0) { return; }
      window.__hypercolorControlsDirty = true;
      for (let index = 0; index < names.length; index += 1) {
        const name = names[index];
        const callback = 'on' + name + 'Changed';
        window[name] = controls[name];
        if (typeof window[callback] === 'function') {
          try { window[callback](); } catch (_err) {}
        }
      }
    };
    const applyInteraction = function(engine, interaction) {
      if (typeof interaction !== 'object' || interaction === null) { return; }
      if (typeof engine.keyboard !== 'object' || engine.keyboard === null) { engine.keyboard = {}; }
      if (typeof engine.mouse !== 'object' || engine.mouse === null) { engine.mouse = {}; }
      const keyboard = typeof interaction.keyboard === 'object' && interaction.keyboard !== null ? interaction.keyboard : {};
      const mouse = typeof interaction.mouse === 'object' && interaction.mouse !== null ? interaction.mouse : {};
      engine.keyboard.keys = trueObject(keyboard.keys);
      engine.keyboard.recent = Array.isArray(keyboard.recent) ? keyboard.recent.slice() : [];
      engine.mouse.x = finiteNumber(mouse.x, 0);
      engine.mouse.y = finiteNumber(mouse.y, 0);
      engine.mouse.down = mouse.down === true;
      engine.mouse.buttons = trueObject(mouse.buttons);
    };
    window.__hypercolorApplyFramePayload = function(payload) {
      if (typeof payload !== 'object' || payload === null) { return; }
      if (typeof window.engine !== 'object' || window.engine === null) { window.engine = {}; }
      const engine = window.engine;
      const timing = typeof payload.timing === 'object' && payload.timing !== null ? payload.timing : {};
      const canvas = typeof payload.canvas === 'object' && payload.canvas !== null ? payload.canvas : {};
      engine.time = finiteNumber(timing.timeSecs, engine.time || 0);
      engine.deltaTime = finiteNumber(timing.deltaSecs, engine.deltaTime || 0);
      engine.frame = finiteNumber(timing.frameNumber, engine.frame || 0);
      engine.width = finiteNumber(canvas.width, engine.width || 1);
      engine.height = finiteNumber(canvas.height, engine.height || 1);
      applyAudio(engine, payload.audio);
      applyScreen(engine, payload.screen);
      applySensors(engine, payload.sensors);
      applyControls(payload.controls);
      applyInteraction(engine, payload.interaction);
      if (typeof globalThis === 'object' && globalThis !== null) { globalThis.engine = engine; }
      if (payload.renderHostFrame && typeof window.__hypercolorRenderHostFrame === 'function') { window.__hypercolorRenderHostFrame(); }
    };
  }
