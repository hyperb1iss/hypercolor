use hypercolor_core::input::{InputData, ManagedSource, ManagedSourceRole};

struct Source;

impl ManagedSource for Source {
    fn name(&self) -> &'static str {
        "roleless"
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

fn main() {
    let _source = ManagedSourceRole::Audio(Box::new(Source));
}
