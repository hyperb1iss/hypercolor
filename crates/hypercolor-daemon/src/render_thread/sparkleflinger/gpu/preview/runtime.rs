use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

use anyhow::{Context, Result};
use hypercolor_core::types::canvas::PublishedSurface;

use super::super::GpuSparkleFlinger;
use super::{PendingPreviewMap, PendingPreviewReadback};
use crate::render_thread::sparkleflinger::PreviewSurfaceRequest;

impl GpuSparkleFlinger {
    pub(crate) fn resolve_preview_surface(&mut self) -> Result<Option<PublishedSurface>> {
        self.submit_pending_preview_work()?;

        if self.pending_preview_map.is_some() {
            if let Some(surface) = self.try_finish_pending_preview_map()? {
                return Ok(Some(surface));
            }
            return Ok(None);
        }

        if self.pending_preview_readback().is_some() {
            let Some(submission_index) = self.pending_preview_submission() else {
                return Ok(None);
            };
            if !self.preview_submission_ready(submission_index.clone())? {
                return Ok(None);
            }
            let mut frame = self
                .frame_in_flight
                .take()
                .expect("submitted preview frame should remain staged until mapping begins");
            let Some(pending_preview_readback) = frame.take_preview_readback() else {
                return Ok(None);
            };
            if self.pending_preview_map.is_some() {
                self.discard_pending_preview_map();
            }
            self.begin_pending_preview_map(pending_preview_readback, Some(submission_index))?;
            return self.try_finish_pending_preview_map();
        }

        if let Some(surface) = self.ready_preview_surface.take() {
            return Ok(Some(surface));
        }
        self.try_finish_pending_preview_map()
    }

    pub(crate) fn submit_pending_preview_work(&mut self) -> Result<()> {
        if self.pending_preview_readback().is_none() {
            return Ok(());
        }
        #[cfg(all(target_os = "macos", feature = "screen-capture"))]
        let mut native_screen_leases = Vec::new();
        let submission_index = {
            let frame_in_flight = &mut self.frame_in_flight;
            let submission_index = frame_in_flight
                .as_mut()
                .and_then(|frame| frame.submit(&self.queue));
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            if submission_index.is_some()
                && let Some(frame) = frame_in_flight.as_mut()
            {
                native_screen_leases = frame.take_native_screen_leases();
            }
            submission_index
        };
        if let Some(submission_index) = submission_index {
            self.finish_pending_uploads(submission_index.clone());
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            self.retire_native_screen_leases(submission_index, native_screen_leases);
            self.release_retired_uniform_slots();
        }
        if self.pending_preview_map.is_some() {
            return Ok(());
        }
        let mut frame = self
            .frame_in_flight
            .take()
            .expect("pending preview readback should have a frame owner");
        let submission_index = frame
            .submission_index()
            .context("pending preview frame should be submitted before mapping")?;
        let pending_preview_readback = frame
            .take_preview_readback()
            .expect("pending preview readback should exist before GPU preview submit");
        self.begin_pending_preview_map(pending_preview_readback, Some(submission_index))?;
        Ok(())
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn clear_superseded_preview_outputs(
        &mut self,
    ) {
        drop(self.supersede_frame_in_flight("preview outputs superseded"));
        self.ready_preview_surface = None;
        self.discard_pending_uploads();
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn discard_superseded_preview_work(
        &mut self,
    ) {
        self.clear_superseded_preview_outputs();
        self.discard_pending_preview_map();
    }

    pub(crate) fn discard_preview_work(&mut self) {
        self.discard_superseded_preview_work();
    }

    fn preview_submission_ready(
        &mut self,
        submission_index: wgpu::SubmissionIndex,
    ) -> Result<bool> {
        #[cfg(test)]
        if std::mem::take(&mut self.defer_preview_resolve_once) {
            return Ok(false);
        }

        match self.device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission_index),
            timeout: Some(Duration::ZERO),
        }) {
            Ok(_) => Ok(true),
            Err(wgpu::PollError::Timeout) => Ok(false),
            Err(error) => Err(error).context("GPU preview readiness poll failed"),
        }
    }

    #[cfg(test)]
    pub(in crate::render_thread::sparkleflinger::gpu) fn defer_next_preview_map_resolve(&mut self) {
        self.defer_preview_map_resolve_once = true;
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn begin_pending_preview_map(
        &mut self,
        pending_preview_readback: PendingPreviewReadback,
        submission_index: Option<wgpu::SubmissionIndex>,
    ) -> Result<()> {
        let PendingPreviewReadback::PreviewBuffer { request, slot, .. } = &pending_preview_readback;
        let preview_surfaces = self
            .preview_surfaces
            .as_ref()
            .context("GPU scaled preview map requested before preview surfaces existed")?;
        let used_bytes =
            u64::from(preview_surfaces.padded_bytes_per_row) * u64::from(request.height);
        let slice = preview_surfaces.readback(*slot).slice(..used_bytes);
        let (sender, receiver) = mpsc::channel::<std::result::Result<(), wgpu::BufferAsyncError>>();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.pending_preview_map = Some(PendingPreviewMap {
            readback: pending_preview_readback,
            submission_index,
            used_bytes,
            receiver,
        });
        Ok(())
    }

    fn try_finish_pending_preview_map(&mut self) -> Result<Option<PublishedSurface>> {
        let Some(pending_preview_map) = self.pending_preview_map.as_ref() else {
            return Ok(None);
        };

        let poll_result =
            if let Some(submission_index) = pending_preview_map.submission_index.clone() {
                self.device.poll(wgpu::PollType::Wait {
                    submission_index: Some(submission_index),
                    timeout: Some(Duration::from_millis(1)),
                })
            } else {
                self.device.poll(wgpu::PollType::Poll)
            };
        match poll_result {
            Ok(_) | Err(wgpu::PollError::Timeout) => {}
            Err(error) => return Err(error).context("GPU preview map poll failed"),
        }

        #[cfg(test)]
        if std::mem::take(&mut self.defer_preview_map_resolve_once) {
            return Ok(None);
        }

        match pending_preview_map.receiver.try_recv() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                self.discard_pending_preview_map();
                return Err(error).context("GPU preview buffer mapping failed");
            }
            Err(TryRecvError::Empty) => return Ok(None),
            Err(TryRecvError::Disconnected) => {
                self.discard_pending_preview_map();
                anyhow::bail!("GPU preview channel closed before map completion");
            }
        }

        let pending_preview_map = self
            .pending_preview_map
            .take()
            .expect("GPU preview map should remain pending until completion");
        self.finish_mapped_preview_surface(
            pending_preview_map.readback,
            pending_preview_map.used_bytes,
        )
        .map(Some)
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn has_pending_or_ready_preview_for(
        &self,
        request: PreviewSurfaceRequest,
    ) -> bool {
        self.ready_preview_surface.as_ref().is_some_and(|surface| {
            surface.width() == request.width && surface.height() == request.height
        }) || self
            .pending_preview_readback()
            .is_some_and(|pending| pending.matches_request(request))
            || self
                .pending_preview_map
                .as_ref()
                .is_some_and(|pending| pending.readback.matches_request(request))
    }
}
