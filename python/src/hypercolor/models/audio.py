"""Audio and spectrum models."""

from __future__ import annotations

import msgspec


class AudioLevels(msgspec.Struct, kw_only=True):
    """Aggregated audio analysis levels."""

    level: float
    bass: float
    mid: float
    treble: float
    beat: bool
    beat_confidence: float


class FrequencyRange(msgspec.Struct, kw_only=True):
    """Frequency range in Hz."""

    min: int
    max: int


class SpectrumSnapshot(msgspec.Struct, kw_only=True):
    """Current audio spectrum data."""

    timestamp: str
    levels: AudioLevels
    bins: list[float]
    bin_count: int
    frequency_range: FrequencyRange
    bpm: float | None = None


class AudioInput(msgspec.Struct, kw_only=True):
    """Audio input configuration and state."""

    id: str
    type: str
    name: str
    enabled: bool
    status: str
    device_name: str


class AudioDeviceInfo(msgspec.Struct, kw_only=True):
    """An available audio input device."""

    id: str
    name: str
    description: str


class AudioDevices(msgspec.Struct, kw_only=True):
    """Available audio devices and the currently selected one."""

    current: str
    devices: list[AudioDeviceInfo] = msgspec.field(default_factory=list)
