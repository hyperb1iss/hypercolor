#[cfg(feature = "macos-capture-fixtures")]
use super::lock;
use super::{
    Arc, InputData, InputSource, MacosScreenCaptureInput, ScreenCaptureDemand, ScreenSourceRole,
    SourceRoleBinding, SourceStatusHandle, SourceStatusReporter,
};

impl InputSource for MacosScreenCaptureInput {
    fn name(&self) -> &'static str {
        "macos_screen_capture"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        if self.running {
            return Ok(());
        }
        self.refresh_policy()?;
        if let Some(extent) = self.demand.requested_extent() {
            #[cfg(feature = "macos-capture-fixtures")]
            let prepared = self.stage_worker(self.prepare_worker(extent)?)?;
            #[cfg(not(feature = "macos-capture-fixtures"))]
            let prepared = self.stage_worker(self.prepare_worker(extent))?;
            let session = self.status.begin_session()?;
            self.install_worker(prepared);
            if let Some(session) = session {
                self.status_session.store(session);
            }
            self.control.set_active(true);
        }
        self.refresh_platform_status()?;
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.control.set_active(false);
        self.refresh_platform_status()
            .expect("live macOS screen status is not retired");
        self.status_session.clear();
        self.stop_worker();
        self.status.stop();
        self.demand = ScreenCaptureDemand::Inactive;
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        self.refresh_platform_status()?;
        self.observe_worker_exit()?;
        if !self.running || !self.demand.is_active() {
            return Ok(InputData::None);
        }
        #[cfg(feature = "macos-capture-fixtures")]
        {
            let data = {
                let publication = lock(self.adapter.compatibility_publication());
                publication
                    .snapshot()
                    .filter(|snapshot| snapshot.epoch == self.worker_generation)
                    .map_or(InputData::None, |snapshot| {
                        let _publication_revision = snapshot.revision;
                        snapshot.value.as_ref().clone()
                    })
            };
            if !matches!(data, InputData::None) {
                self.refresh_platform_status()?;
            }
            Ok(data)
        }
        #[cfg(not(feature = "macos-capture-fixtures"))]
        {
            Ok(InputData::None)
        }
    }

    fn sample_shared_and_drain_into(
        &mut self,
        _delta_secs: f32,
        _events: &mut Vec<crate::types::event::TimedInputEvent>,
    ) -> anyhow::Result<Option<Arc<InputData>>> {
        self.refresh_platform_status()?;
        self.observe_worker_exit()?;
        if !self.running || !self.demand.is_active() {
            return Ok(None);
        }
        #[cfg(feature = "macos-capture-fixtures")]
        {
            let data = {
                let publication = lock(self.adapter.compatibility_publication());
                publication
                    .snapshot()
                    .filter(|snapshot| snapshot.epoch == self.worker_generation)
                    .map(|snapshot| snapshot.value)
            };
            if data.is_some() {
                self.refresh_platform_status()?;
            }
            Ok(data)
        }
        #[cfg(not(feature = "macos-capture-fixtures"))]
        {
            Ok(None)
        }
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn source_status_handle(&self) -> Option<SourceStatusHandle> {
        Some(self.status.handle())
    }

    fn source_status_reporter(&mut self) -> Option<&mut SourceStatusReporter> {
        Some(&mut self.status)
    }
}

impl SourceRoleBinding for MacosScreenCaptureInput {
    type Role = ScreenSourceRole;
}

impl Drop for MacosScreenCaptureInput {
    fn drop(&mut self) {
        self.control.set_active(false);
        self.stop_worker();
    }
}
