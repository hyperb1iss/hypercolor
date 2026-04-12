use hypercolor_types::canvas::SamplingMethod;
use hypercolor_types::spatial::{EdgeBehavior, NormalizedPosition};

#[derive(Debug, Clone)]
pub struct PreparedZonePlan {
    pub zone_id: String,
    pub sampling_method: SamplingMethod,
    pub edge_behavior: EdgeBehavior,
    pub sample_positions: Vec<NormalizedPosition>,
    pub prepared_canvas_width: u32,
    pub prepared_canvas_height: u32,
    pub prepared_samples: PreparedZoneSamples,
}

#[derive(Debug, Clone)]
pub enum PreparedZoneSamples {
    Nearest(Vec<PreparedNearestSample>),
    Bilinear(Vec<PreparedBilinearSample>),
    Area(Vec<PreparedAreaSample>),
}

impl PreparedZoneSamples {
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Nearest(samples) => samples.len(),
            Self::Bilinear(samples) => samples.len(),
            Self::Area(samples) => samples.len(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PreparedNearestSample {
    pub offset: usize,
    pub attenuation: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct PreparedBilinearSample {
    pub offsets: [usize; 4],
    pub x_lower_weight: u16,
    pub x_upper_weight: u16,
    pub y_lower_weight: u16,
    pub y_upper_weight: u16,
    pub attenuation: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct PreparedAreaSample {
    pub center_x: i32,
    pub center_y: i32,
    pub radius: i32,
    pub canvas_width: i32,
    pub canvas_height: i32,
    pub attenuation: u16,
}
