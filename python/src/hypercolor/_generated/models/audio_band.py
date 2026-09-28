from enum import StrEnum


class AudioBand(StrEnum):
    BASS = "bass"
    BEAT_PULSE = "beat_pulse"
    MID = "mid"
    ONSET_PULSE = "onset_pulse"
    PEAK = "peak"
    RMS = "rms"
    TREBLE = "treble"

    def __str__(self) -> str:
        return str(self.value)
