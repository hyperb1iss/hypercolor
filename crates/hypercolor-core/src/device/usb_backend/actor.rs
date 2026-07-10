use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use hypercolor_hal::protocol::{Protocol, ProtocolCommand, ProtocolError, ResponseStatus};
use hypercolor_hal::transport::{Transport, TransportError};
use hypercolor_types::device::DeviceId;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tracing::{debug, trace, warn};

use super::{
    MAX_RETRIES, RETRY_BACKOFF, UsbBackend, UsbDeviceCommand, UsbDisplayPayload, UsbFramePayload,
    describe_packet, format_error_chain, format_hex_preview, map_transport_error,
    record_usb_display_lane,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FrameWriteDisposition {
    Transient,
    Fatal,
}

impl UsbBackend {
    #[expect(
        clippy::too_many_arguments,
        reason = "actor bootstrap needs the transport, channels, ids, and shared error sink together"
    )]
    pub(super) fn spawn_device_actor(
        device_id: DeviceId,
        device_name: &'static str,
        protocol: Arc<dyn Protocol>,
        transport: Arc<dyn Transport>,
        active: Arc<AtomicBool>,
        lifecycle_gate: Arc<StdMutex<()>>,
        frame_tx: watch::Sender<Option<Arc<UsbFramePayload>>>,
        frame_rx: watch::Receiver<Option<Arc<UsbFramePayload>>>,
        display_rx: watch::Receiver<Option<Arc<UsbDisplayPayload>>>,
        command_rx: mpsc::UnboundedReceiver<UsbDeviceCommand>,
        last_async_error: Arc<StdMutex<Option<String>>>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let transport_name = transport.name();
            let parallel_transfer_lanes = transport.supports_parallel_transfer_lanes();

            let actor_result = if parallel_transfer_lanes {
                Self::run_parallel_device_actor(
                    device_id,
                    device_name,
                    protocol.clone(),
                    transport.clone(),
                    frame_rx,
                    display_rx,
                    command_rx,
                )
                .await
            } else {
                Self::run_device_actor(
                    device_id,
                    device_name,
                    protocol.clone(),
                    transport.clone(),
                    frame_rx,
                    display_rx,
                    command_rx,
                )
                .await
            };

            let rejection = actor_result
                .as_ref()
                .map_or_else(ToString::to_string, |()| {
                    "USB device actor stopped before transport started".to_owned()
                });
            {
                let _gate = super::lock_lifecycle_gate(&lifecycle_gate);
                active.store(false, Ordering::Release);
                if let Some(pending) = frame_tx.send_replace(None) {
                    pending.reject_pending(rejection);
                }
            }

            if let Err(error) = actor_result {
                Self::store_actor_error(&last_async_error, error.to_string());
                warn!(
                    device_id = %device_id,
                    device = device_name,
                    protocol = protocol.name(),
                    transport = transport_name,
                    parallel_transfer_lanes,
                    error = %error,
                    error_chain = %format_error_chain(&error),
                    "USB device actor failed"
                );
            }

            if let Err(error) = transport.close().await.map_err(map_transport_error) {
                Self::store_actor_error(&last_async_error, error.to_string());
                warn!(
                    device_id = %device_id,
                    device = device_name,
                    protocol = protocol.name(),
                    transport = transport_name,
                    error = %error,
                    "failed to close USB transport after actor shutdown"
                );
            }
        })
    }

    async fn run_parallel_device_actor(
        device_id: DeviceId,
        device_name: &'static str,
        protocol: Arc<dyn Protocol>,
        transport: Arc<dyn Transport>,
        frame_rx: watch::Receiver<Option<Arc<UsbFramePayload>>>,
        display_rx: watch::Receiver<Option<Arc<UsbDisplayPayload>>>,
        command_rx: mpsc::UnboundedReceiver<UsbDeviceCommand>,
    ) -> Result<()> {
        let mut control_task = tokio::spawn(Self::run_device_control_actor(
            device_id,
            device_name,
            Arc::clone(&protocol),
            Arc::clone(&transport),
            frame_rx,
            command_rx,
        ));
        let mut display_task = tokio::spawn(Self::run_device_display_actor(
            device_id, protocol, transport, display_rx,
        ));

        tokio::select! {
            result = &mut control_task => {
                display_task.abort();
                let _ = display_task.await;
                Self::flatten_actor_result(result, "USB control actor")
            }
            result = &mut display_task => {
                match Self::flatten_actor_result(result, "USB display actor") {
                    Ok(()) => debug!(
                        device_id = %device_id,
                        device = device_name,
                        "USB display actor exited; control lane remains active"
                    ),
                    Err(error) => warn!(
                        device_id = %device_id,
                        device = device_name,
                        error = %error,
                        error_chain = %format_error_chain(&error),
                        "USB display actor failed; keeping control lane active"
                    ),
                }
                Self::flatten_actor_result(control_task.await, "USB control actor")
            }
        }
    }

    #[cfg(test)]
    pub(super) async fn test_run_parallel_device_actor(
        device_id: DeviceId,
        device_name: &'static str,
        protocol: Arc<dyn Protocol>,
        transport: Arc<dyn Transport>,
        frame_rx: watch::Receiver<Option<Arc<UsbFramePayload>>>,
        display_rx: watch::Receiver<Option<Arc<UsbDisplayPayload>>>,
        command_rx: mpsc::UnboundedReceiver<UsbDeviceCommand>,
    ) -> Result<()> {
        Self::run_parallel_device_actor(
            device_id,
            device_name,
            protocol,
            transport,
            frame_rx,
            display_rx,
            command_rx,
        )
        .await
    }

    fn flatten_actor_result(
        result: std::result::Result<Result<()>, tokio::task::JoinError>,
        lane_name: &'static str,
    ) -> Result<()> {
        result.unwrap_or_else(|error| Err(anyhow!("{lane_name} task failed: {error}")))
    }

    async fn run_device_control_actor(
        device_id: DeviceId,
        device_name: &'static str,
        protocol: Arc<dyn Protocol>,
        transport: Arc<dyn Transport>,
        mut frame_rx: watch::Receiver<Option<Arc<UsbFramePayload>>>,
        mut command_rx: mpsc::UnboundedReceiver<UsbDeviceCommand>,
    ) -> Result<()> {
        let mut keepalive_interval = protocol.keepalive().map(|keepalive| {
            let mut interval = tokio::time::interval_at(
                tokio::time::Instant::now() + keepalive.interval,
                keepalive.interval,
            );
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            interval
        });
        let mut frame_commands = Vec::new();

        loop {
            tokio::select! {
                biased;
                Some(command) = command_rx.recv() => {
                    match command {
                        UsbDeviceCommand::SetBrightness {
                            brightness,
                            response_tx,
                        } => {
                            let result = Self::run_brightness_command(
                                device_id,
                                device_name,
                                protocol.as_ref(),
                                transport.as_ref(),
                                brightness,
                            )
                            .await;

                            let response =
                                result.as_ref().map_err(ToString::to_string).copied();
                            let _ = response_tx.send(response);
                            result?;
                        }
                        UsbDeviceCommand::Shutdown {
                            led_count,
                            response_tx,
                        } => {
                            let result = Self::run_shutdown_sequence(
                                device_id,
                                device_name,
                                led_count,
                                protocol.as_ref(),
                                transport.as_ref(),
                            )
                            .await;
                            let response =
                                result.as_ref().map_err(ToString::to_string).copied();
                            let _ = response_tx.send(response);
                            return result;
                        }
                    }
                }
                () = async {
                    if let Some(interval) = keepalive_interval.as_mut() {
                        interval.tick().await;
                    }
                }, if keepalive_interval.is_some() => {
                    Self::run_keepalive_commands(
                        device_id,
                        device_name,
                        protocol.as_ref(),
                        transport.as_ref(),
                    )
                    .await?;
                }
                changed = frame_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }

                    let Some(frame) = frame_rx.borrow_and_update().clone() else {
                        continue;
                    };

                    Self::run_resilient_device_frame(
                        device_id,
                        protocol.as_ref(),
                        transport.as_ref(),
                        &frame,
                        &mut frame_commands,
                    )
                    .await?;
                }
                else => break,
            }
        }

        Ok(())
    }

    async fn run_device_display_actor(
        device_id: DeviceId,
        protocol: Arc<dyn Protocol>,
        transport: Arc<dyn Transport>,
        mut display_rx: watch::Receiver<Option<Arc<UsbDisplayPayload>>>,
    ) -> Result<()> {
        let mut display_commands = Vec::new();

        loop {
            let changed = display_rx.changed().await;
            if changed.is_err() {
                break;
            }

            let Some(frame) = display_rx.borrow_and_update().clone() else {
                continue;
            };

            record_usb_display_lane(Duration::ZERO, false);
            if let Err(error) = Self::run_device_display_frame(
                device_id,
                protocol.as_ref(),
                transport.as_ref(),
                &frame,
                &mut display_commands,
            )
            .await
            {
                warn!(
                    device_id = %device_id,
                    protocol = protocol.name(),
                    transport = transport.name(),
                    error = %error,
                    error_chain = %format_error_chain(&error),
                    "USB display frame write failed; display lane will continue"
                );
            }
        }

        Ok(())
    }

    async fn run_brightness_command(
        device_id: DeviceId,
        device_name: &'static str,
        protocol: &dyn Protocol,
        transport: &dyn Transport,
        brightness: u8,
    ) -> Result<()> {
        if let Some(commands) = protocol.encode_brightness(brightness) {
            let first_packet = commands.first().map_or_else(
                || "<none>".to_owned(),
                |command| describe_packet(&command.data),
            );
            debug!(
                device_id = %device_id,
                device = device_name,
                protocol = protocol.name(),
                transport = transport.name(),
                brightness,
                command_count = commands.len(),
                first_packet = %first_packet,
                "usb brightness write requested"
            );

            Self::run_commands(protocol, transport, commands.as_slice())
                .await
                .with_context(|| format!("USB brightness write failed for device {device_id}"))
        } else {
            Err(anyhow!(
                "USB protocol does not support brightness for device {device_id}"
            ))
        }
    }

    async fn run_keepalive_commands(
        device_id: DeviceId,
        device_name: &'static str,
        protocol: &dyn Protocol,
        transport: &dyn Transport,
    ) -> Result<()> {
        let commands = protocol.keepalive_commands();
        if commands.is_empty() {
            return Ok(());
        }

        trace!(
            device_id = %device_id,
            device = device_name,
            protocol = protocol.name(),
            transport = transport.name(),
            command_count = commands.len(),
            "usb keepalive tick"
        );

        Self::run_commands(protocol, transport, commands.as_slice())
            .await
            .with_context(|| format!("USB keepalive failed for device {device_id}"))
    }

    #[expect(
        clippy::too_many_lines,
        reason = "device actor loop coordinates command, keepalive, frame, and display streams in one place"
    )]
    async fn run_device_actor(
        device_id: DeviceId,
        device_name: &'static str,
        protocol: Arc<dyn Protocol>,
        transport: Arc<dyn Transport>,
        mut frame_rx: watch::Receiver<Option<Arc<UsbFramePayload>>>,
        mut display_rx: watch::Receiver<Option<Arc<UsbDisplayPayload>>>,
        mut command_rx: mpsc::UnboundedReceiver<UsbDeviceCommand>,
    ) -> Result<()> {
        let mut keepalive_interval = protocol.keepalive().map(|keepalive| {
            let mut interval = tokio::time::interval_at(
                tokio::time::Instant::now() + keepalive.interval,
                keepalive.interval,
            );
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            interval
        });
        let mut frame_commands = Vec::new();
        let mut display_commands = Vec::new();

        loop {
            tokio::select! {
                biased;
                Some(command) = command_rx.recv() => {
                    match command {
                        UsbDeviceCommand::SetBrightness {
                            brightness,
                            response_tx,
                        } => {
                            let result = if let Some(commands) = protocol.encode_brightness(brightness) {
                                let first_packet = commands.first().map_or_else(
                                    || "<none>".to_owned(),
                                    |command| describe_packet(&command.data),
                                );
                                debug!(
                                    device_id = %device_id,
                                    device = device_name,
                                    protocol = protocol.name(),
                                    transport = transport.name(),
                                    brightness,
                                    command_count = commands.len(),
                                    first_packet = %first_packet,
                                    "usb brightness write requested"
                                );

                                Self::run_commands(
                                    protocol.as_ref(),
                                    transport.as_ref(),
                                    commands.as_slice(),
                                )
                                    .await
                                    .with_context(|| format!("USB brightness write failed for device {device_id}"))
                            } else {
                                Err(anyhow!(
                                    "USB protocol does not support brightness for device {device_id}"
                                ))
                            };

                            let response =
                                result.as_ref().map_err(ToString::to_string).copied();
                            let _ = response_tx.send(response);
                            result?;
                        }
                        UsbDeviceCommand::Shutdown {
                            led_count,
                            response_tx,
                        } => {
                            let result = Self::run_shutdown_sequence(
                                device_id,
                                device_name,
                                led_count,
                                protocol.as_ref(),
                                transport.as_ref(),
                            )
                            .await;
                            let response =
                                result.as_ref().map_err(ToString::to_string).copied();
                            let _ = response_tx.send(response);
                            return result;
                        }
                    }
                }
                () = async {
                    if let Some(interval) = keepalive_interval.as_mut() {
                        interval.tick().await;
                    }
                }, if keepalive_interval.is_some() => {
                    let commands = protocol.keepalive_commands();
                    if commands.is_empty() {
                        continue;
                    }

                    trace!(
                        device_id = %device_id,
                        device = device_name,
                        protocol = protocol.name(),
                        transport = transport.name(),
                        command_count = commands.len(),
                        "usb keepalive tick"
                    );

                    Self::run_commands(
                        protocol.as_ref(),
                        transport.as_ref(),
                        commands.as_slice(),
                    )
                        .await
                        .with_context(|| format!("USB keepalive failed for device {device_id}"))?;
                }
                changed = display_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }

                    let Some(frame) = display_rx.borrow_and_update().clone() else {
                        continue;
                    };

                    let wait_for_led_started = Instant::now();
                    let delayed_for_led = Self::run_overdue_device_frame(
                        device_id,
                        protocol.as_ref(),
                        transport.as_ref(),
                        &mut frame_rx,
                        &mut frame_commands,
                    )
                    .await?;
                    record_usb_display_lane(wait_for_led_started.elapsed(), delayed_for_led);

                    if let Err(error) = Self::run_device_display_frame(
                        device_id,
                        protocol.as_ref(),
                        transport.as_ref(),
                        &frame,
                        &mut display_commands,
                    )
                    .await
                    {
                        warn!(
                            device_id = %device_id,
                            device = device_name,
                            protocol = protocol.name(),
                            transport = transport.name(),
                            error = %error,
                            error_chain = %format_error_chain(&error),
                            "USB display frame write failed; LED lane will continue"
                        );
                    }
                }
                changed = frame_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }

                    let Some(frame) = frame_rx.borrow_and_update().clone() else {
                        continue;
                    };

                    Self::run_resilient_device_frame(
                        device_id,
                        protocol.as_ref(),
                        transport.as_ref(),
                        &frame,
                        &mut frame_commands,
                    )
                    .await?;
                }
                else => break,
            }
        }

        Ok(())
    }

    #[cfg(test)]
    pub(super) async fn test_run_device_actor(
        device_id: DeviceId,
        device_name: &'static str,
        protocol: Arc<dyn Protocol>,
        transport: Arc<dyn Transport>,
        frame_rx: watch::Receiver<Option<Arc<UsbFramePayload>>>,
        display_rx: watch::Receiver<Option<Arc<UsbDisplayPayload>>>,
        command_rx: mpsc::UnboundedReceiver<UsbDeviceCommand>,
    ) -> Result<()> {
        Self::run_device_actor(
            device_id,
            device_name,
            protocol,
            transport,
            frame_rx,
            display_rx,
            command_rx,
        )
        .await
    }

    async fn run_overdue_device_frame(
        device_id: DeviceId,
        protocol: &dyn Protocol,
        transport: &dyn Transport,
        frame_rx: &mut watch::Receiver<Option<Arc<UsbFramePayload>>>,
        commands: &mut Vec<ProtocolCommand>,
    ) -> Result<bool> {
        if !frame_rx.has_changed().unwrap_or(false) {
            return Ok(false);
        }

        let Some(frame) = frame_rx.borrow_and_update().clone() else {
            return Ok(false);
        };

        Self::run_resilient_device_frame(device_id, protocol, transport, &frame, commands)
            .await
            .map(|()| true)
    }

    async fn run_resilient_device_frame(
        device_id: DeviceId,
        protocol: &dyn Protocol,
        transport: &dyn Transport,
        frame: &UsbFramePayload,
        commands: &mut Vec<ProtocolCommand>,
    ) -> Result<()> {
        if !frame.mark_transport_started() {
            return Ok(());
        }
        let transport_started_at = Instant::now();
        match Self::run_device_frame(device_id, protocol, transport, frame, commands).await {
            Ok(()) => {
                if let Some(id) = frame.delivery_id {
                    frame.acknowledge(super::DeviceDeliveryAck::completed(
                        id,
                        frame.colors.len().saturating_mul(3),
                        transport_started_at.elapsed(),
                    ));
                }
                Ok(())
            }
            Err(error)
                if Self::classify_frame_write_error(&error) == FrameWriteDisposition::Transient =>
            {
                if let Some(id) = frame.delivery_id {
                    frame.acknowledge(super::DeviceDeliveryAck::failed(
                        id,
                        true,
                        transport_started_at.elapsed(),
                        error.to_string(),
                    ));
                }
                warn!(
                    device_id = %device_id,
                    protocol = protocol.name(),
                    transport = transport.name(),
                    error = %error,
                    error_chain = %format_error_chain(&error),
                    "transient USB frame write failed; actor will continue"
                );
                Ok(())
            }
            Err(error) => {
                if let Some(id) = frame.delivery_id {
                    frame.acknowledge(super::DeviceDeliveryAck::failed(
                        id,
                        true,
                        transport_started_at.elapsed(),
                        error.to_string(),
                    ));
                }
                Err(error)
            }
        }
    }

    pub(super) fn classify_frame_write_error(error: &anyhow::Error) -> FrameWriteDisposition {
        match error
            .chain()
            .find_map(|cause| cause.downcast_ref::<TransportError>())
        {
            Some(TransportError::IoError { detail })
                if Self::io_error_indicates_liveness_loss(detail) =>
            {
                FrameWriteDisposition::Fatal
            }
            Some(TransportError::Timeout { .. } | TransportError::IoError { .. }) => {
                FrameWriteDisposition::Transient
            }
            Some(
                TransportError::NotFound { .. }
                | TransportError::Closed
                | TransportError::PermissionDenied { .. }
                | TransportError::UnsupportedTransfer { .. },
            )
            | None => FrameWriteDisposition::Fatal,
        }
    }

    fn io_error_indicates_liveness_loss(detail: &str) -> bool {
        let detail = detail.to_ascii_lowercase();
        [
            "disconnected",
            "not connected",
            "device removed",
            "no such device",
            "permission denied",
            "access denied",
            "transport closed",
        ]
        .iter()
        .any(|marker| detail.contains(marker))
    }

    async fn run_device_frame(
        device_id: DeviceId,
        protocol: &dyn Protocol,
        transport: &dyn Transport,
        frame: &UsbFramePayload,
        commands: &mut Vec<ProtocolCommand>,
    ) -> Result<()> {
        protocol.encode_frame_into(frame.colors.as_slice(), commands);
        if tracing::enabled!(tracing::Level::TRACE) {
            let first_packet = commands.first().map_or_else(
                || "<none>".to_owned(),
                |command| describe_packet(&command.data),
            );

            trace!(
                device_id = %device_id,
                protocol = protocol.name(),
                transport = transport.name(),
                led_count = frame.colors.len(),
                command_count = commands.len(),
                first_packet = %first_packet,
                "usb frame write requested"
            );
        }

        Self::run_commands(protocol, transport, commands.as_slice())
            .await
            .with_context(|| format!("USB frame write failed for device {device_id}"))
    }

    async fn run_device_display_frame(
        device_id: DeviceId,
        protocol: &dyn Protocol,
        transport: &dyn Transport,
        frame: &UsbDisplayPayload,
        commands: &mut Vec<ProtocolCommand>,
    ) -> Result<()> {
        protocol
            .encode_display_payload_into(frame.payload.as_borrowed(), commands)
            .with_context(|| {
                format!("USB protocol does not support display output for device {device_id}")
            })?;
        if tracing::enabled!(tracing::Level::TRACE) {
            let first_packet = commands.first().map_or_else(
                || "<none>".to_owned(),
                |command| describe_packet(&command.data),
            );

            trace!(
                device_id = %device_id,
                protocol = protocol.name(),
                transport = transport.name(),
                display_format = %frame.payload.format,
                display_bytes = frame.payload.data.len(),
                command_count = commands.len(),
                first_packet = %first_packet,
                "usb display write requested"
            );
        }

        Self::run_commands(protocol, transport, commands.as_slice())
            .await
            .with_context(|| format!("USB display write failed for device {device_id}"))
    }

    async fn run_shutdown_sequence(
        device_id: DeviceId,
        device_name: &'static str,
        led_count: usize,
        protocol: &dyn Protocol,
        transport: &dyn Transport,
    ) -> Result<()> {
        if led_count > 0 {
            let black_frame = vec![[0, 0, 0]; led_count];
            let mut commands = Vec::new();
            if let Err(error) = Self::run_device_frame(
                device_id,
                protocol,
                transport,
                &UsbFramePayload {
                    colors: Arc::new(black_frame),
                    delivery_id: None,
                    delivery_observer: None,
                    delivery_tx: StdMutex::new(None),
                    delivery_state: std::sync::atomic::AtomicU8::new(super::DELIVERY_PENDING),
                },
                &mut commands,
            )
            .await
            {
                warn!(
                    device_id = %device_id,
                    device = device_name,
                    protocol = protocol.name(),
                    transport = transport.name(),
                    error = %error,
                    "USB final clear frame failed during shutdown"
                );
            }
        }

        let shutdown = protocol.shutdown_sequence();
        if shutdown.is_empty() {
            return Ok(());
        }

        if let Err(error) = Self::run_commands(protocol, transport, shutdown.as_slice()).await {
            warn!(
                device_id = %device_id,
                device = device_name,
                protocol = protocol.name(),
                transport = transport.name(),
                error = %error,
                "USB shutdown sequence failed"
            );
        }

        Ok(())
    }

    fn store_actor_error(last_async_error: &Arc<StdMutex<Option<String>>>, error: String) {
        if let Ok(mut slot) = last_async_error.lock() {
            *slot = Some(error);
        }
    }

    pub(super) async fn run_commands(
        protocol: &dyn Protocol,
        transport: &dyn Transport,
        commands: &[ProtocolCommand],
    ) -> Result<()> {
        let total_commands = commands.len();

        for (index, command) in commands.iter().enumerate() {
            let command_position = index + 1;
            Self::trace_queued_command(
                protocol,
                transport,
                command,
                command_position,
                total_commands,
            );
            Self::run_command(
                protocol,
                transport,
                command,
                command_position,
                total_commands,
            )
            .await?;
        }

        Ok(())
    }

    fn trace_queued_command(
        protocol: &dyn Protocol,
        transport: &dyn Transport,
        command: &ProtocolCommand,
        command_position: usize,
        total_commands: usize,
    ) {
        trace!(
            protocol = protocol.name(),
            transport = transport.name(),
            command_index = command_position,
            total_commands,
            expects_response = command.expects_response,
            post_delay_ms = command.post_delay.as_millis(),
            transfer_type = ?command.transfer_type,
            packet = %describe_packet(&command.data),
            "usb command queued"
        );
        trace!(
            protocol = protocol.name(),
            transport = transport.name(),
            command_index = command_position,
            total_commands,
            packet_hex = %format_hex_preview(&command.data, 32),
            "usb command bytes"
        );
    }

    async fn run_command(
        protocol: &dyn Protocol,
        transport: &dyn Transport,
        command: &ProtocolCommand,
        command_position: usize,
        total_commands: usize,
    ) -> Result<()> {
        let mut attempt = 0_u8;

        loop {
            if command.expects_response {
                if Self::run_response_command(
                    protocol,
                    transport,
                    command,
                    command_position,
                    total_commands,
                    &mut attempt,
                )
                .await?
                {
                    continue;
                }
            } else {
                trace!(
                    protocol = protocol.name(),
                    transport = transport.name(),
                    command_index = command_position,
                    total_commands,
                    attempt = attempt + 1,
                    transfer_type = ?command.transfer_type,
                    "usb send starting"
                );
                transport
                    .send_with_type(&command.data, command.transfer_type)
                    .await
                    .map_err(map_transport_error)?;
                if !command.post_delay.is_zero() {
                    tokio::time::sleep(command.post_delay).await;
                }
                return Ok(());
            }

            if !command.post_delay.is_zero() {
                tokio::time::sleep(command.post_delay).await;
            }

            return Ok(());
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "response handling keeps retry, delayed reads, parsing, and tracing in one place"
    )]
    async fn run_response_command(
        protocol: &dyn Protocol,
        transport: &dyn Transport,
        command: &ProtocolCommand,
        command_position: usize,
        total_commands: usize,
        attempt: &mut u8,
    ) -> Result<bool> {
        let response = if command.response_delay.is_zero() {
            trace!(
                protocol = protocol.name(),
                transport = transport.name(),
                command_index = command_position,
                total_commands,
                attempt = *attempt + 1,
                transfer_type = ?command.transfer_type,
                "usb send_receive starting"
            );
            transport
                .send_receive_with_type(
                    &command.data,
                    protocol.response_timeout(),
                    command.transfer_type,
                )
                .await
                .map_err(map_transport_error)?
        } else {
            trace!(
                protocol = protocol.name(),
                transport = transport.name(),
                command_index = command_position,
                total_commands,
                attempt = *attempt + 1,
                transfer_type = ?command.transfer_type,
                response_delay_us = command.response_delay.as_micros(),
                "usb send starting with delayed response read"
            );
            transport
                .send_with_type(&command.data, command.transfer_type)
                .await
                .map_err(map_transport_error)?;
            tokio::time::sleep(command.response_delay).await;
            transport
                .receive_with_type(protocol.response_timeout(), command.transfer_type)
                .await
                .map_err(map_transport_error)?
        };

        trace!(
            protocol = protocol.name(),
            transport = transport.name(),
            command_index = command_position,
            total_commands,
            response = %describe_packet(&response),
            "usb response received"
        );
        trace!(
            protocol = protocol.name(),
            transport = transport.name(),
            command_index = command_position,
            total_commands,
            response_hex = %format_hex_preview(&response, 32),
            "usb response bytes"
        );

        match protocol.parse_response(&response) {
            Ok(parsed) => {
                trace!(
                    protocol = protocol.name(),
                    transport = transport.name(),
                    command_index = command_position,
                    total_commands,
                    status = ?parsed.status,
                    parsed_data_len = parsed.data.len(),
                    parsed_data = %format_hex_preview(&parsed.data, 24),
                    "usb response parsed"
                );
                if matches!(
                    parsed.status,
                    ResponseStatus::Busy | ResponseStatus::Timeout
                ) && *attempt < MAX_RETRIES
                {
                    *attempt = attempt.saturating_add(1);
                    tokio::time::sleep(RETRY_BACKOFF).await;
                    return Ok(true);
                }

                if parsed.status == ResponseStatus::Unsupported {
                    warn!(
                        protocol = protocol.name(),
                        "command not supported by device; continuing"
                    );
                }

                Ok(false)
            }
            Err(ProtocolError::DeviceError {
                status: ResponseStatus::Busy | ResponseStatus::Timeout,
            }) if *attempt < MAX_RETRIES => {
                *attempt = attempt.saturating_add(1);
                tokio::time::sleep(RETRY_BACKOFF).await;
                Ok(true)
            }
            Err(error) => {
                warn!(
                    protocol = protocol.name(),
                    transport = transport.name(),
                    command_index = command_position,
                    total_commands,
                    attempt = *attempt + 1,
                    transfer_type = ?command.transfer_type,
                    expects_response = command.expects_response,
                    command = %describe_packet(&command.data),
                    command_hex = %format_hex_preview(&command.data, 32),
                    response_len = response.len(),
                    error = %error,
                    response = %describe_packet(&response),
                    response_hex = %format_hex_preview(&response, 32),
                    "protocol response parse failed"
                );
                Err(anyhow!("protocol response parse failed: {error}"))
            }
        }
    }
}
