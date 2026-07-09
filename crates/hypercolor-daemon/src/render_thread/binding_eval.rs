use std::f32::consts::TAU;
use std::time::Duration;

use hypercolor_types::audio::AudioData;
use hypercolor_types::layer::{
    AudioBand, BindingMap, BindingSource, LayerAdjust, LayerBinding, LayerParameter, LayerSource,
    LayerTransform, SceneLayer, TimeWave,
};
use hypercolor_types::sensor::SystemSnapshot;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LayerRuntime {
    opacity: f32,
    transform: LayerTransform,
    adjust: LayerAdjust,
    playback_speed: f32,
}

impl LayerRuntime {
    fn from_layer(layer: &SceneLayer) -> Self {
        let playback_speed = match &layer.source {
            LayerSource::Media { playback, .. } => playback.speed,
            _ => 1.0,
        };
        Self {
            opacity: layer.opacity,
            transform: layer.transform,
            adjust: layer.adjust,
            playback_speed,
        }
    }

    pub(crate) fn apply_to_layer(self, layer: &SceneLayer) -> SceneLayer {
        let mut layer = layer.clone();
        layer.opacity = self.opacity;
        layer.transform = self.transform;
        layer.adjust = self.adjust;
        if let LayerSource::Media { playback, .. } = &mut layer.source {
            playback.speed = self.playback_speed;
        }
        layer
    }
}

pub(crate) fn evaluate_layer_runtime(
    layer: &SceneLayer,
    audio: &AudioData,
    sensors: &SystemSnapshot,
    elapsed_ms: u64,
) -> LayerRuntime {
    let mut runtime = LayerRuntime::from_layer(layer);
    for binding in &layer.bindings {
        let value = binding_source_value(&binding.source, audio, sensors, elapsed_ms);
        let mapped = map_binding_value(value, &binding.map);
        apply_binding(&mut runtime, binding, mapped);
    }
    runtime
}

fn binding_source_value(
    source: &BindingSource,
    audio: &AudioData,
    sensors: &SystemSnapshot,
    elapsed_ms: u64,
) -> f32 {
    match source {
        BindingSource::AudioBand { band } => audio_band_value(*band, audio),
        BindingSource::Sensor { name } => {
            sensors.reading(name).map_or(0.0, |reading| reading.value)
        }
        BindingSource::Time { rate_hz, wave } => time_wave_value(*rate_hz, *wave, elapsed_ms),
        BindingSource::Constant { value } => *value,
    }
}

fn audio_band_value(band: AudioBand, audio: &AudioData) -> f32 {
    match band {
        AudioBand::Bass => audio.bass(),
        AudioBand::Mid => audio.mid(),
        AudioBand::Treble => audio.treble(),
        AudioBand::Rms => audio.rms_level,
        AudioBand::Peak => audio.peak_level,
        AudioBand::BeatPulse => audio.beat_pulse,
        AudioBand::OnsetPulse => audio.onset_pulse,
    }
}

#[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
fn time_wave_value(rate_hz: f32, wave: TimeWave, elapsed_ms: u64) -> f32 {
    if !rate_hz.is_finite() {
        return 0.0;
    }

    let elapsed_secs = Duration::from_millis(elapsed_ms).as_secs_f64();
    let phase = (elapsed_secs * f64::from(rate_hz)).rem_euclid(1.0) as f32;
    match wave {
        TimeWave::Sine => (phase * TAU).sin(),
        TimeWave::Triangle => 1.0 - (4.0 * (phase - 0.5).abs()),
        TimeWave::Saw => (phase * 2.0) - 1.0,
        TimeWave::Square => {
            if phase < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
    }
}

fn map_binding_value(value: f32, map: &BindingMap) -> f32 {
    if !value.is_finite()
        || !map.source_min.is_finite()
        || !map.source_max.is_finite()
        || !map.target_min.is_finite()
        || !map.target_max.is_finite()
        || map.source_min == map.source_max
    {
        return map.target_min;
    }

    let t = (value - map.source_min) / (map.source_max - map.source_min);
    let t = if map.clamp { t.clamp(0.0, 1.0) } else { t };
    map.target_min + (t * (map.target_max - map.target_min))
}

fn apply_binding(runtime: &mut LayerRuntime, binding: &LayerBinding, value: f32) {
    match binding.target {
        LayerParameter::Opacity => runtime.opacity = value.clamp(0.0, 1.0),
        LayerParameter::Brightness => runtime.adjust.brightness = value.clamp(0.0, 4.0),
        LayerParameter::Saturation => runtime.adjust.saturation = value.clamp(0.0, 4.0),
        LayerParameter::HueShift => runtime.adjust.hue_shift = finite_or(value, 0.0),
        LayerParameter::TintStrength => runtime.adjust.tint_strength = value.clamp(0.0, 1.0),
        LayerParameter::Contrast => runtime.adjust.contrast = value.clamp(-1.0, 1.0),
        LayerParameter::ScaleX => runtime.transform.scale[0] = value.clamp(0.01, 16.0),
        LayerParameter::ScaleY => runtime.transform.scale[1] = value.clamp(0.01, 16.0),
        LayerParameter::Rotation => runtime.transform.rotation = finite_or(value, 0.0),
        LayerParameter::PlaybackSpeed => runtime.playback_speed = value.clamp(0.0, 4.0),
    }
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use hypercolor_types::effect::EffectId;
    use hypercolor_types::layer::{
        BindingMap, BindingSource, LayerBinding, LayerParameter, SceneLayer, SceneLayerId, TimeWave,
    };
    use hypercolor_types::sensor::SensorReading;
    use uuid::Uuid;

    use super::*;

    fn effect_layer(bindings: Vec<LayerBinding>) -> SceneLayer {
        let mut layer = SceneLayer::from_effect(
            SceneLayerId::new(),
            EffectId::from(Uuid::now_v7()),
            HashMap::new(),
            HashMap::new(),
            None,
        );
        layer.bindings = bindings;
        layer
    }

    #[test]
    fn constant_binding_overrides_opacity() {
        let layer = effect_layer(vec![LayerBinding {
            target: LayerParameter::Opacity,
            source: BindingSource::Constant { value: 0.5 },
            map: BindingMap::linear(0.0..=1.0, 0.0..=1.0),
        }]);

        let runtime =
            evaluate_layer_runtime(&layer, &AudioData::silence(), &SystemSnapshot::empty(), 0);

        assert_eq!(runtime.apply_to_layer(&layer).opacity, 0.5);
    }

    #[test]
    fn audio_binding_maps_bass_to_tint_strength() {
        let mut audio = AudioData::silence();
        audio.spectrum[..40].fill(0.75);
        let layer = effect_layer(vec![LayerBinding {
            target: LayerParameter::TintStrength,
            source: BindingSource::AudioBand {
                band: AudioBand::Bass,
            },
            map: BindingMap::linear(0.0..=1.0, 0.0..=0.8),
        }]);

        let runtime = evaluate_layer_runtime(&layer, &audio, &SystemSnapshot::empty(), 0);

        assert_eq!(runtime.apply_to_layer(&layer).adjust.tint_strength, 0.6);
    }

    #[test]
    fn sensor_binding_normalizes_matching_label() {
        let mut sensors = SystemSnapshot::empty();
        sensors.components.push(SensorReading::new(
            "GPU Temp",
            70.0,
            hypercolor_types::sensor::SensorUnit::Celsius,
            None,
            None,
            None,
        ));
        let layer = effect_layer(vec![LayerBinding {
            target: LayerParameter::Brightness,
            source: BindingSource::Sensor {
                name: "gpu_temp".into(),
            },
            map: BindingMap::linear(40.0..=80.0, 0.0..=2.0),
        }]);

        let runtime = evaluate_layer_runtime(&layer, &AudioData::silence(), &sensors, 0);

        assert_eq!(runtime.apply_to_layer(&layer).adjust.brightness, 1.5);
    }

    #[test]
    fn time_binding_uses_elapsed_frame_time() {
        let layer = effect_layer(vec![LayerBinding {
            target: LayerParameter::Rotation,
            source: BindingSource::Time {
                rate_hz: 1.0,
                wave: TimeWave::Saw,
            },
            map: BindingMap::linear(-1.0..=1.0, 0.0..=10.0),
        }]);

        let runtime =
            evaluate_layer_runtime(&layer, &AudioData::silence(), &SystemSnapshot::empty(), 250);

        assert_eq!(runtime.apply_to_layer(&layer).transform.rotation, 2.5);
    }
}
