use hypercolor_core::effect::EffectRenderer;
use hypercolor_types::control::{ControlDeltaBatch, ControlId, ControlValue, SetRevision};

pub trait TestControlRenderer {
    fn apply_test_control(&mut self, control_id: &str, value: &ControlValue);
}

impl<T> TestControlRenderer for T
where
    T: EffectRenderer + ?Sized,
{
    fn apply_test_control(&mut self, control_id: &str, value: &ControlValue) {
        let changes = [(ControlId::from(control_id), value.clone())];
        self.apply_controls(&ControlDeltaBatch::new(SetRevision::default(), 0, &changes));
    }
}
