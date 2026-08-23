from enum import Enum


class AudioBand(str, Enum):
    BASS = "bass"
    BEAT_PULSE = "beat_pulse"
    MID = "mid"
    ONSET_PULSE = "onset_pulse"
    PEAK = "peak"
    RMS = "rms"
    TREBLE = "treble"

    def __str__(self) -> str:
        return str(self.value)
