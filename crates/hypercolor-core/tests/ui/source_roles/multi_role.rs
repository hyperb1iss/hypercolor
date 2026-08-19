use hypercolor_core::input::{
    AudioSource, AudioSourceRole, InputData, ManagedSource, ScreenSource, SourceRoleBinding,
};

struct Source;

impl ManagedSource for Source {
    fn name(&self) -> &'static str {
        "multi-role"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn stop(&mut self) {}

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(InputData::None)
    }

    fn is_running(&self) -> bool {
        false
    }
}

impl SourceRoleBinding for Source {
    type Role = AudioSourceRole;
}

impl AudioSource for Source {}
impl ScreenSource for Source {}

fn main() {}
