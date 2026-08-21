use hypercolor_core::effect::EffectRenderer;
use hypercolor_types::control::{ControlDeltaBatch, ControlId, ControlValue, SetRevision};
use hypercolor_types::effect::ControlValue as EffectControlValue;

pub trait TestControlRenderer {
    fn apply_test_control(&mut self, control_id: &str, value: &EffectControlValue);
}

impl<T> TestControlRenderer for T
where
    T: EffectRenderer + ?Sized,
{
    fn apply_test_control(&mut self, control_id: &str, value: &EffectControlValue) {
        let value = ControlValue::try_from(value.clone()).expect("valid test control value");
        let changes = [(ControlId::from(control_id), value)];
        self.apply_controls(&ControlDeltaBatch::new(SetRevision::default(), 0, &changes))
            .expect("test control delivery");
    }
}
