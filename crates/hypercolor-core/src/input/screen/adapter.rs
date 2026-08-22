#[cfg(test)]
mod tests;

pub trait CapturePublicationEpoch: PartialEq {
    fn source_generation(&self) -> u64;

    fn activity_generation(&self) -> u64;
}

pub struct CapturePublication<E, T> {
    source_generation: u64,
    activity_generation: u64,
    pub(super) active: Option<E>,
    pub(super) latest: Option<T>,
}

impl<E, T> Default for CapturePublication<E, T> {
    fn default() -> Self {
        Self {
            source_generation: 0,
            activity_generation: 0,
            active: None,
            latest: None,
        }
    }
}

impl<E, T> CapturePublication<E, T>
where
    E: CapturePublicationEpoch,
{
    pub fn activate(&mut self, active: E) -> bool {
        if active.source_generation() != self.source_generation
            || active.activity_generation() != self.activity_generation
        {
            return false;
        }
        if self.active.as_ref() != Some(&active) {
            self.latest = None;
            self.active = Some(active);
        }
        true
    }

    pub fn fence_source(&mut self, source_generation: u64) {
        self.source_generation = source_generation;
        self.clear();
    }

    pub fn fence_activity(&mut self, activity_generation: u64) {
        self.activity_generation = activity_generation;
        self.clear();
    }

    pub fn clear(&mut self) {
        self.active = None;
        self.latest = None;
    }

    pub fn publish(&mut self, active: &E, value: T) -> bool {
        if self.active.as_ref() != Some(active) {
            return false;
        }
        self.latest = Some(value);
        true
    }
}
