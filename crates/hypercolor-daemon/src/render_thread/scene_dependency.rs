#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SceneDependencyKey {
    pub(crate) zones_revision: u64,
    pub(crate) dependency_generation: u64,
}

impl SceneDependencyKey {
    pub(crate) const fn new(zones_revision: u64, dependency_generation: u64) -> Self {
        Self {
            zones_revision,
            dependency_generation,
        }
    }
}
