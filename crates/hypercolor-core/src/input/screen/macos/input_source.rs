use super::{
    Arc, InputData, InputSource, MacosScreenCaptureInput, ScreenCaptureDemand, ScreenSourceRole,
    SourceRoleBinding, SourceStatusHandle, SourceStatusReporter,
};

impl InputSource for MacosScreenCaptureInput {
    fn name(&self) -> &'static str {
        "macos_screen_capture"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        if self.shell.running {
            return Ok(());
        }
        self.refresh_policy()?;
        if self.shell.adapter.settings().demand().is_active() {
            #[cfg(feature = "macos-capture-fixtures")]
            let prepared = self.stage_worker(self.prepare_worker()?)?;
            #[cfg(not(feature = "macos-capture-fixtures"))]
            let prepared = self.stage_worker(self.prepare_worker())?;
            let session = self.shell.status.begin_session()?;
            self.install_worker(prepared);
            if let Some(session) = session {
                self.shell.status_session.store(session);
            }
            self.control.set_active(true);
        }
        self.refresh_platform_status()?;
        self.shell.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.control.set_active(false);
        self.refresh_platform_status()
            .expect("live macOS screen status is not retired");
        self.shell.status_session.clear();
        self.stop_worker();
        self.shell.status.stop();
        let mut settings = self.shell.adapter.settings().lock();
        *settings.demand_mut() = ScreenCaptureDemand::Inactive;
        settings.commit();
        self.shell.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        self.refresh_platform_status()?;
        self.observe_worker_exit()?;
        if !self.shell.running || !self.shell.adapter.settings().demand().is_active() {
            return Ok(InputData::None);
        }
        #[cfg(feature = "macos-capture-fixtures")]
        {
            let data = self
                .shell
                .adapter
                .exact_state()
                .fixture_reference(self.shell.adapter.backend().worker_generation.get())
                .map_or(InputData::None, |value| value.as_ref().clone());
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
        _events: &mut Vec<hypercolor_types::event::TimedInputEvent>,
    ) -> anyhow::Result<Option<Arc<InputData>>> {
        self.refresh_platform_status()?;
        self.observe_worker_exit()?;
        if !self.shell.running || !self.shell.adapter.settings().demand().is_active() {
            return Ok(None);
        }
        #[cfg(feature = "macos-capture-fixtures")]
        {
            let data = self
                .shell
                .adapter
                .exact_state()
                .fixture_reference(self.shell.adapter.backend().worker_generation.get());
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
        self.shell.running
    }

    fn source_status_handle(&self) -> Option<SourceStatusHandle> {
        Some(self.shell.status.handle())
    }

    fn source_status_reporter(&mut self) -> Option<&mut SourceStatusReporter> {
        Some(&mut self.shell.status)
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
