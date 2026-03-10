#![cfg(target_os = "linux")]

use std::error::Error;
use std::time::Duration;

use hypercolor_hal::database::{ProtocolDatabase, TransportType};
use hypercolor_hal::drivers::razer::{
    PID_BASILISK_V3, PID_BLADE_15_LATE_2021_ADVANCED, PID_SEIREN_V3_CHROMA, RAZER_VENDOR_ID,
    RazerReport, razer_crc,
};
use zerocopy::{FromZeros, IntoBytes};
use hypercolor_hal::protocol::{Protocol, ProtocolCommand, ResponseStatus};
use hypercolor_hal::transport::Transport;
use hypercolor_hal::transport::control::UsbControlTransport;
use hypercolor_hal::transport::hidraw::UsbHidRawTransport;

const HARDWARE_TEST_ENV: &str = "HYPERCOLOR_TEST_RAZER_HARDWARE";
const RESPONSE_TIMEOUT: Duration = Duration::from_millis(1_000);
const IDENTIFY_FLASH_MS: Duration = Duration::from_millis(150);

fn hardware_tests_enabled() -> bool {
    std::env::var(HARDWARE_TEST_ENV)
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            matches!(normalized.as_str(), "1" | "true" | "yes")
        })
        .unwrap_or(false)
}

async fn run_commands(
    protocol: &dyn Protocol,
    transport: &dyn Transport,
    commands: Vec<ProtocolCommand>,
) -> Result<(), Box<dyn Error>> {
    for command in commands {
        if command.expects_response {
            let response = if command.response_delay.is_zero() {
                transport
                    .send_receive(&command.data, RESPONSE_TIMEOUT)
                    .await?
            } else {
                transport.send(&command.data).await?;
                tokio::time::sleep(command.response_delay).await;
                transport.receive(RESPONSE_TIMEOUT).await?
            };
            let parsed = protocol.parse_response(&response)?;
            assert_eq!(
                parsed.status,
                ResponseStatus::Ok,
                "device returned non-OK response for command {:02X?}",
                command.data
            );
        } else {
            transport.send(&command.data).await?;
        }

        if !command.post_delay.is_zero() {
            tokio::time::sleep(command.post_delay).await;
        }
    }

    Ok(())
}

fn usb_path(usb: &nusb::DeviceInfo) -> String {
    let ports = usb
        .port_chain()
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(".");

    if ports.is_empty() {
        usb.bus_id().to_owned()
    } else {
        format!("{}-{ports}", usb.bus_id())
    }
}

fn raw_razer_packet(
    transaction_id: u8,
    data_size: u8,
    command_class: u8,
    command_id: u8,
    args: &[u8],
) -> Vec<u8> {
    let mut report = RazerReport::new_zeroed();
    report.transaction_id = transaction_id;
    report.data_size = data_size;
    report.command_class = command_class;
    report.command_id = command_id;
    report.args[..args.len()].copy_from_slice(args);
    report.crc = razer_crc(&report);
    report.as_bytes().to_vec()
}

#[tokio::test]
#[ignore = "requires a connected Blade 15 (Late 2021 Advanced) and HYPERCOLOR_TEST_RAZER_HARDWARE=1"]
async fn blade_identify_sequence_writes_without_protocol_errors() -> Result<(), Box<dyn Error>> {
    if !hardware_tests_enabled() {
        eprintln!("skipping hardware smoke test; set {HARDWARE_TEST_ENV}=1 to enable it");
        return Ok(());
    }

    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_BLADE_15_LATE_2021_ADVANCED)
        .expect("Blade 15 descriptor should exist");
    let (interface, report_id) = match descriptor.transport {
        TransportType::UsbControl {
            interface,
            report_id,
        } => (interface, report_id),
        other => panic!("expected control transport for Blade 15, got {other:?}"),
    };

    let usb = nusb::list_devices()
        .await?
        .find(|device| {
            device.vendor_id() == RAZER_VENDOR_ID
                && device.product_id() == PID_BLADE_15_LATE_2021_ADVANCED
        })
        .expect("Blade 15 USB device should be present");
    let transport = UsbControlTransport::new(usb.open().await?, interface, report_id).await?;
    let protocol = (descriptor.protocol.build)();
    let total_leds = usize::try_from(protocol.total_leds()).expect("LED count should fit usize");
    let purple = vec![[0x80, 0x10, 0xD8]; total_leds];
    let clear = vec![[0, 0, 0]; total_leds];

    let result = async {
        run_commands(protocol.as_ref(), &transport, protocol.init_sequence()).await?;
        run_commands(
            protocol.as_ref(),
            &transport,
            protocol.encode_frame(&purple),
        )
        .await?;
        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok::<(), Box<dyn Error>>(())
    }
    .await;

    let _ = run_commands(protocol.as_ref(), &transport, protocol.encode_frame(&clear)).await;
    let _ = run_commands(protocol.as_ref(), &transport, protocol.shutdown_sequence()).await;
    let _ = transport.close().await;

    result
}

#[tokio::test]
#[ignore = "requires a connected Basilisk V3 and HYPERCOLOR_TEST_RAZER_HARDWARE=1"]
async fn basilisk_v3_custom_mode_init_and_frame_write_without_mode_command()
-> Result<(), Box<dyn Error>> {
    if !hardware_tests_enabled() {
        eprintln!("skipping hardware smoke test; set {HARDWARE_TEST_ENV}=1 to enable it");
        return Ok(());
    }

    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_BASILISK_V3)
        .expect("Basilisk descriptor should exist");
    let (interface, report_id, report_mode, usage_page, usage) = match descriptor.transport {
        TransportType::UsbHidRaw {
            interface,
            report_id,
            report_mode,
            usage_page,
            usage,
        } => (interface, report_id, report_mode, usage_page, usage),
        other => panic!("expected hidraw transport for Basilisk V3, got {other:?}"),
    };

    let usb = nusb::list_devices()
        .await?
        .find(|device| {
            device.vendor_id() == RAZER_VENDOR_ID && device.product_id() == PID_BASILISK_V3
        })
        .expect("Basilisk USB device should be present");
    let usb_device_path = usb_path(&usb);
    let transport = UsbHidRawTransport::open(
        RAZER_VENDOR_ID,
        PID_BASILISK_V3,
        interface,
        report_id,
        report_mode,
        usb.serial_number(),
        Some(&usb_device_path),
        usage_page,
        usage,
    )
    .await?;
    let protocol = (descriptor.protocol.build)();
    assert!(
        protocol.init_sequence().is_empty(),
        "Basilisk should apply custom mode per frame"
    );
    let total_leds = usize::try_from(protocol.total_leds()).expect("LED count should fit usize");
    let white = vec![[255, 255, 255]; total_leds];
    let clear = vec![[0, 0, 0]; total_leds];

    let result = async {
        run_commands(protocol.as_ref(), &transport, protocol.init_sequence()).await?;
        run_commands(protocol.as_ref(), &transport, protocol.encode_frame(&white)).await?;
        tokio::time::sleep(IDENTIFY_FLASH_MS).await;
        Ok::<(), Box<dyn Error>>(())
    }
    .await;

    let _ = run_commands(protocol.as_ref(), &transport, protocol.encode_frame(&clear)).await;
    let _ = transport.close().await;

    result
}

#[tokio::test]
#[ignore = "requires a connected Basilisk V3 and HYPERCOLOR_TEST_RAZER_HARDWARE=1; streams frames to reproduce long-session instability"]
async fn basilisk_v3_sustained_frame_stream_stays_stable() -> Result<(), Box<dyn Error>> {
    if !hardware_tests_enabled() {
        eprintln!("skipping hardware smoke test; set {HARDWARE_TEST_ENV}=1 to enable it");
        return Ok(());
    }

    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_BASILISK_V3)
        .expect("Basilisk descriptor should exist");
    let (interface, report_id, report_mode, usage_page, usage) = match descriptor.transport {
        TransportType::UsbHidRaw {
            interface,
            report_id,
            report_mode,
            usage_page,
            usage,
        } => (interface, report_id, report_mode, usage_page, usage),
        other => panic!("expected hidraw transport for Basilisk V3, got {other:?}"),
    };

    let usb = nusb::list_devices()
        .await?
        .find(|device| {
            device.vendor_id() == RAZER_VENDOR_ID && device.product_id() == PID_BASILISK_V3
        })
        .expect("Basilisk USB device should be present");
    let usb_device_path = usb_path(&usb);
    let transport = UsbHidRawTransport::open(
        RAZER_VENDOR_ID,
        PID_BASILISK_V3,
        interface,
        report_id,
        report_mode,
        usb.serial_number(),
        Some(&usb_device_path),
        usage_page,
        usage,
    )
    .await?;
    let protocol = (descriptor.protocol.build)();
    let total_leds = usize::try_from(protocol.total_leds()).expect("LED count should fit usize");
    let purple = vec![[0x50, 0x10, 0xD5]; total_leds];
    let teal = vec![[0x10, 0xA0, 0x80]; total_leds];
    let clear = vec![[0, 0, 0]; total_leds];

    let result = async {
        run_commands(protocol.as_ref(), &transport, protocol.init_sequence()).await?;

        let started = tokio::time::Instant::now();
        let stream_duration = Duration::from_secs(40);
        let frame_period = Duration::from_millis(50);
        let mut use_purple = true;

        while started.elapsed() < stream_duration {
            let frame = if use_purple { &purple } else { &teal };
            run_commands(protocol.as_ref(), &transport, protocol.encode_frame(frame)).await?;
            use_purple = !use_purple;
            tokio::time::sleep(frame_period).await;
        }

        Ok::<(), Box<dyn Error>>(())
    }
    .await;

    let _ = run_commands(protocol.as_ref(), &transport, protocol.encode_frame(&clear)).await;
    let _ = transport.close().await;

    result
}

#[tokio::test]
#[ignore = "manual Basilisk V3 interface sweep for visual diagnostics"]
async fn basilisk_v3_interface_sweep_visual_diagnostic() -> Result<(), Box<dyn Error>> {
    if !hardware_tests_enabled() {
        eprintln!("skipping hardware smoke test; set {HARDWARE_TEST_ENV}=1 to enable it");
        return Ok(());
    }

    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_BASILISK_V3)
        .expect("Basilisk descriptor should exist");
    let (report_id, report_mode) = match descriptor.transport {
        TransportType::UsbHidRaw {
            report_id,
            report_mode,
            ..
        } => (report_id, report_mode),
        other => panic!("expected hidraw transport for Basilisk V3, got {other:?}"),
    };

    let usb = nusb::list_devices()
        .await?
        .find(|device| {
            device.vendor_id() == RAZER_VENDOR_ID && device.product_id() == PID_BASILISK_V3
        })
        .expect("Basilisk USB device should be present");
    let usb_device_path = usb_path(&usb);
    let protocol = (descriptor.protocol.build)();
    let total_leds = usize::try_from(protocol.total_leds()).expect("LED count should fit usize");
    let white = vec![[0xFF, 0xFF, 0xFF]; total_leds];
    let clear = vec![[0, 0, 0]; total_leds];

    for interface in [3_u8, 1, 0, 2] {
        eprintln!("basilisk interface probe: interface={interface}");

        let transport = match UsbHidRawTransport::open(
            RAZER_VENDOR_ID,
            PID_BASILISK_V3,
            interface,
            report_id,
            report_mode,
            usb.serial_number(),
            Some(&usb_device_path),
            None,
            None,
        )
        .await
        {
            Ok(transport) => transport,
            Err(error) => {
                eprintln!(
                    "basilisk interface probe open failed: interface={interface} error={error}"
                );
                continue;
            }
        };

        let result = async {
            run_commands(protocol.as_ref(), &transport, protocol.init_sequence()).await?;
            run_commands(protocol.as_ref(), &transport, protocol.encode_frame(&white)).await?;
            tokio::time::sleep(Duration::from_secs(2)).await;
            Ok::<(), Box<dyn Error>>(())
        }
        .await;

        let _ = run_commands(protocol.as_ref(), &transport, protocol.encode_frame(&clear)).await;
        let _ = transport.close().await;

        match result {
            Ok(()) => eprintln!("basilisk interface probe completed: interface={interface}"),
            Err(error) => eprintln!(
                "basilisk interface probe send failed: interface={interface} error={error}"
            ),
        }

        tokio::time::sleep(Duration::from_millis(750)).await;
    }

    Ok(())
}

#[tokio::test]
#[ignore = "manual Basilisk V3 interface 3 color probe"]
async fn basilisk_v3_interface_3_color_probe() -> Result<(), Box<dyn Error>> {
    if !hardware_tests_enabled() {
        eprintln!("skipping hardware smoke test; set {HARDWARE_TEST_ENV}=1 to enable it");
        return Ok(());
    }

    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_BASILISK_V3)
        .expect("Basilisk descriptor should exist");
    let (report_id, report_mode, usage_page, usage) = match descriptor.transport {
        TransportType::UsbHidRaw {
            report_id,
            report_mode,
            usage_page,
            usage,
            ..
        } => (report_id, report_mode, usage_page, usage),
        other => panic!("expected hidraw transport for Basilisk V3, got {other:?}"),
    };

    let usb = nusb::list_devices()
        .await?
        .find(|device| {
            device.vendor_id() == RAZER_VENDOR_ID && device.product_id() == PID_BASILISK_V3
        })
        .expect("Basilisk USB device should be present");
    let usb_device_path = usb_path(&usb);
    let transport = UsbHidRawTransport::open(
        RAZER_VENDOR_ID,
        PID_BASILISK_V3,
        3,
        report_id,
        report_mode,
        usb.serial_number(),
        Some(&usb_device_path),
        usage_page,
        usage,
    )
    .await?;
    let protocol = (descriptor.protocol.build)();
    let total_leds = usize::try_from(protocol.total_leds()).expect("LED count should fit usize");
    let red = vec![[0xFF, 0x00, 0x00]; total_leds];
    let blue = vec![[0x00, 0x00, 0xFF]; total_leds];
    let clear = vec![[0, 0, 0]; total_leds];

    let result = async {
        run_commands(protocol.as_ref(), &transport, protocol.init_sequence()).await?;
        eprintln!("basilisk color probe: red");
        run_commands(protocol.as_ref(), &transport, protocol.encode_frame(&red)).await?;
        tokio::time::sleep(Duration::from_secs(2)).await;
        eprintln!("basilisk color probe: blue");
        run_commands(protocol.as_ref(), &transport, protocol.encode_frame(&blue)).await?;
        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok::<(), Box<dyn Error>>(())
    }
    .await;

    let _ = run_commands(protocol.as_ref(), &transport, protocol.encode_frame(&clear)).await;
    let _ = transport.close().await;

    result
}

#[tokio::test]
#[ignore = "manual Basilisk V3 packet diagnostics on connected hardware"]
async fn basilisk_v3_signalrgb_sequence_diagnostics() -> Result<(), Box<dyn Error>> {
    if !hardware_tests_enabled() {
        eprintln!("skipping hardware smoke test; set {HARDWARE_TEST_ENV}=1 to enable it");
        return Ok(());
    }

    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_BASILISK_V3)
        .expect("Basilisk descriptor should exist");
    let (interface, report_id, report_mode, usage_page, usage) = match descriptor.transport {
        TransportType::UsbHidRaw {
            interface,
            report_id,
            report_mode,
            usage_page,
            usage,
        } => (interface, report_id, report_mode, usage_page, usage),
        other => panic!("expected hidraw transport for Basilisk V3, got {other:?}"),
    };

    let usb = nusb::list_devices()
        .await?
        .find(|device| {
            device.vendor_id() == RAZER_VENDOR_ID && device.product_id() == PID_BASILISK_V3
        })
        .expect("Basilisk USB device should be present");
    let usb_device_path = usb_path(&usb);
    let transport = UsbHidRawTransport::open(
        RAZER_VENDOR_ID,
        PID_BASILISK_V3,
        interface,
        report_id,
        report_mode,
        usb.serial_number(),
        Some(&usb_device_path),
        usage_page,
        usage,
    )
    .await?;

    let transaction_ids = [0x1F_u8, 0x2F, 0x3F, 0x4F, 0x5F, 0x6F, 0x7F, 0x8F, 0x9F];
    for transaction_id in transaction_ids {
        let serial_query = raw_razer_packet(transaction_id, 0x02, 0x00, 0x82, &[]);
        let serial_response = transport
            .send_receive(&serial_query, RESPONSE_TIMEOUT)
            .await?;
        eprintln!(
            "basilisk serial response tx=0x{transaction_id:02X}: {:02X?}",
            &serial_response[..24.min(serial_response.len())]
        );
    }

    let mode_command = raw_razer_packet(0x1F, 0x02, 0x00, 0x04, &[0x03, 0x00]);
    let mode_response = transport
        .send_receive(&mode_command, RESPONSE_TIMEOUT)
        .await?;
    eprintln!(
        "basilisk mode response: {:02X?}",
        &mode_response[..12.min(mode_response.len())]
    );

    let brightness_set = raw_razer_packet(0x1F, 0x03, 0x0F, 0x04, &[0x01, 0x00, 0xFF]);
    let brightness_set_response = transport
        .send_receive(&brightness_set, RESPONSE_TIMEOUT)
        .await?;
    eprintln!(
        "basilisk brightness-set response: {:02X?}",
        &brightness_set_response[..16.min(brightness_set_response.len())]
    );

    for led in 0..4_u8 {
        let brightness_get = raw_razer_packet(0x1F, 0x03, 0x0F, 0x84, &[0x00, led]);
        let brightness_get_response = transport
            .send_receive(&brightness_get, RESPONSE_TIMEOUT)
            .await?;
        eprintln!(
            "basilisk brightness-get led={} response: {:02X?}",
            led,
            &brightness_get_response[..16.min(brightness_get_response.len())]
        );
    }

    for attempt in 0..5 {
        let query = raw_razer_packet(0x1F, 0x06, 0x0F, 0x82, &[0x00]);
        let query_response = transport.send_receive(&query, RESPONSE_TIMEOUT).await?;
        eprintln!(
            "basilisk query response attempt {}: {:02X?}",
            attempt + 1,
            &query_response[..12.min(query_response.len())]
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    for attempt in 0..5 {
        let set_effect = raw_razer_packet(0x1F, 0x06, 0x0F, 0x02, &[0x00, 0x00, 0x08, 0x00, 0x01]);
        transport.send(&set_effect).await?;
        tokio::time::sleep(Duration::from_millis(10)).await;
        let effect_response = transport.receive(RESPONSE_TIMEOUT).await?;
        eprintln!(
            "basilisk effect response attempt {}: {:02X?}",
            attempt + 1,
            &effect_response[..12.min(effect_response.len())]
        );
    }

    let legacy_effect = raw_razer_packet(0x1F, 0x02, 0x03, 0x0A, &[0x05, 0x00]);
    transport.send(&legacy_effect).await?;
    tokio::time::sleep(Duration::from_millis(10)).await;
    let legacy_effect_response = transport.receive(RESPONSE_TIMEOUT).await?;
    eprintln!(
        "basilisk legacy-effect response: {:02X?}",
        &legacy_effect_response[..12.min(legacy_effect_response.len())]
    );

    let frame_args = {
        let mut args = vec![0x00, 0x00, 0x00, 0x0A];
        for _ in 0..11 {
            args.extend_from_slice(&[0x20, 0x00, 0x00]);
        }
        args
    };
    let frame = raw_razer_packet(
        0x1F,
        u8::try_from(frame_args.len()).unwrap_or(0),
        0x0F,
        0x03,
        &frame_args,
    );
    transport.send(&frame).await?;
    tokio::time::sleep(IDENTIFY_FLASH_MS).await;

    let clear_args = {
        let mut args = vec![0x00, 0x00, 0x00, 0x0A];
        for _ in 0..11 {
            args.extend_from_slice(&[0x00, 0x00, 0x00]);
        }
        args
    };
    let clear = raw_razer_packet(
        0x1F,
        u8::try_from(clear_args.len()).unwrap_or(0),
        0x0F,
        0x03,
        &clear_args,
    );
    transport.send(&clear).await?;
    let _ = transport.close().await;

    Ok(())
}

#[tokio::test]
#[ignore = "manual Basilisk V3 control-transport diagnostics on connected hardware"]
async fn basilisk_v3_control_transport_diagnostics() -> Result<(), Box<dyn Error>> {
    if !hardware_tests_enabled() {
        eprintln!("skipping hardware smoke test; set {HARDWARE_TEST_ENV}=1 to enable it");
        return Ok(());
    }

    let usb = nusb::list_devices()
        .await?
        .find(|device| {
            device.vendor_id() == RAZER_VENDOR_ID && device.product_id() == PID_BASILISK_V3
        })
        .expect("Basilisk USB device should be present");
    let transport = UsbControlTransport::new(usb.open().await?, 3, 0x00).await?;

    let serial_query = raw_razer_packet(0x1F, 0x02, 0x00, 0x82, &[]);
    transport.send(&serial_query).await?;
    tokio::time::sleep(Duration::from_millis(10)).await;
    let serial_response = transport.receive(RESPONSE_TIMEOUT).await?;
    eprintln!(
        "basilisk control serial response: {:02X?}",
        &serial_response[..24.min(serial_response.len())]
    );

    let query = raw_razer_packet(0x1F, 0x06, 0x0F, 0x82, &[0x00]);
    transport.send(&query).await?;
    tokio::time::sleep(Duration::from_millis(10)).await;
    let query_response = transport.receive(RESPONSE_TIMEOUT).await?;
    eprintln!(
        "basilisk control modern-effect query response: {:02X?}",
        &query_response[..16.min(query_response.len())]
    );

    let set_effect = raw_razer_packet(0x1F, 0x06, 0x0F, 0x02, &[0x00, 0x00, 0x08, 0x00, 0x01]);
    transport.send(&set_effect).await?;
    tokio::time::sleep(Duration::from_millis(10)).await;
    let effect_response = transport.receive(RESPONSE_TIMEOUT).await?;
    eprintln!(
        "basilisk control set-effect response: {:02X?}",
        &effect_response[..16.min(effect_response.len())]
    );

    let legacy_effect = raw_razer_packet(0x1F, 0x02, 0x03, 0x0A, &[0x05, 0x00]);
    transport.send(&legacy_effect).await?;
    tokio::time::sleep(Duration::from_millis(10)).await;
    let legacy_effect_response = transport.receive(RESPONSE_TIMEOUT).await?;
    eprintln!(
        "basilisk control legacy-effect response: {:02X?}",
        &legacy_effect_response[..16.min(legacy_effect_response.len())]
    );

    let frame_args = {
        let mut args = vec![0x00, 0x00, 0x00, 0x0A];
        for _ in 0..11 {
            args.extend_from_slice(&[0x40, 0x00, 0x40]);
        }
        args
    };
    let frame = raw_razer_packet(
        0x1F,
        u8::try_from(frame_args.len()).unwrap_or(0),
        0x0F,
        0x03,
        &frame_args,
    );
    transport.send(&frame).await?;
    tokio::time::sleep(IDENTIFY_FLASH_MS).await;

    let clear_args = {
        let mut args = vec![0x00, 0x00, 0x00, 0x0A];
        for _ in 0..11 {
            args.extend_from_slice(&[0x00, 0x00, 0x00]);
        }
        args
    };
    let clear = raw_razer_packet(
        0x1F,
        u8::try_from(clear_args.len()).unwrap_or(0),
        0x0F,
        0x03,
        &clear_args,
    );
    transport.send(&clear).await?;
    let _ = transport.close().await;

    Ok(())
}

#[tokio::test]
#[ignore = "requires a connected Seiren V3 Chroma and HYPERCOLOR_TEST_RAZER_HARDWARE=1"]
async fn seiren_v3_chroma_init_and_frame_write() -> Result<(), Box<dyn Error>> {
    if !hardware_tests_enabled() {
        eprintln!("skipping hardware smoke test; set {HARDWARE_TEST_ENV}=1 to enable it");
        return Ok(());
    }

    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_SEIREN_V3_CHROMA)
        .expect("Seiren V3 Chroma descriptor should exist");
    let (interface, report_id, report_mode, usage_page, usage) = match descriptor.transport {
        TransportType::UsbHidRaw {
            interface,
            report_id,
            report_mode,
            usage_page,
            usage,
        } => (interface, report_id, report_mode, usage_page, usage),
        other => panic!("expected hidraw transport for Seiren V3 Chroma, got {other:?}"),
    };

    let usb = nusb::list_devices()
        .await?
        .find(|device| {
            device.vendor_id() == RAZER_VENDOR_ID && device.product_id() == PID_SEIREN_V3_CHROMA
        })
        .expect("Seiren V3 Chroma USB device should be present");
    let usb_device_path = usb_path(&usb);
    let transport = UsbHidRawTransport::open(
        RAZER_VENDOR_ID,
        PID_SEIREN_V3_CHROMA,
        interface,
        report_id,
        report_mode,
        usb.serial_number(),
        Some(&usb_device_path),
        usage_page,
        usage,
    )
    .await?;
    let protocol = (descriptor.protocol.build)();
    let total_leds = usize::try_from(protocol.total_leds()).expect("LED count should fit usize");
    let white = vec![[255, 255, 255]; total_leds];
    let clear = vec![[0, 0, 0]; total_leds];

    let result = async {
        run_commands(protocol.as_ref(), &transport, protocol.init_sequence()).await?;
        run_commands(protocol.as_ref(), &transport, protocol.encode_frame(&white)).await?;
        tokio::time::sleep(IDENTIFY_FLASH_MS).await;
        Ok::<(), Box<dyn Error>>(())
    }
    .await;

    let _ = run_commands(protocol.as_ref(), &transport, protocol.encode_frame(&clear)).await;
    let _ = transport.close().await;

    result
}
