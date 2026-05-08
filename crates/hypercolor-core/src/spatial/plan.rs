use hypercolor_types::spatial::{EdgeBehavior, NormalizedPosition, SamplingMode};

#[derive(Debug, Clone)]
pub struct PreparedZonePlan {
    pub plan_generation: u64,
    pub zone_id: String,
    pub sampling_mode: SamplingMode,
    pub edge_behavior: EdgeBehavior,
    pub sample_positions: Vec<NormalizedPosition>,
    pub has_attenuation: bool,
    pub prepared_canvas_width: u32,
    pub prepared_canvas_height: u32,
    pub prepared_samples: PreparedZoneSamples,
}

#[derive(Debug, Clone)]
pub enum PreparedZoneSamples {
    Nearest(Vec<PreparedNearestSample>),
    Bilinear(Vec<PreparedBilinearSample>),
    Area(Vec<PreparedAreaSample>),
    Gaussian(PreparedGaussianSamples),
}

impl PreparedZoneSamples {
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Nearest(samples) => samples.len(),
            Self::Bilinear(samples) => samples.len(),
            Self::Area(samples) => samples.len(),
            Self::Gaussian(samples) => samples.samples.len(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PreparedNearestSample {
    pub offset: u32,
    pub attenuation: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct PreparedBilinearSample {
    pub offsets: [u32; 4],
    pub x_upper_weight: u8,
    pub y_upper_weight: u8,
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

#[derive(Debug, Clone)]
pub struct PreparedGaussianSamples {
    pub samples: Vec<PreparedGaussianSample>,
    pub weights: Vec<u16>,
    pub weight_sum: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct PreparedGaussianSample {
    pub center_x: i32,
    pub center_y: i32,
    pub radius: i32,
    pub canvas_width: i32,
    pub canvas_height: i32,
    pub attenuation: u16,
}
