use hypercolor_hal::registry::{HidRawReportMode, TransportType};
use hypercolor_hal::transport::{
    HidAccessMode, HidTransportIntent, TransportError, TransportIntent, TransportPlatform,
    resolve_transport,
};

const ASUS_INTENT: TransportIntent = TransportIntent::Hid(HidTransportIntent {
    access: HidAccessMode::HostManaged,
    interface: 2,
    report_id: 0xEC,
    report_mode: HidRawReportMode::OutputReport,
    max_report_len: 65,
    usage_page: None,
    usage: None,
});

const DIRECT_HID_INTENT: TransportIntent = TransportIntent::Hid(HidTransportIntent {
    access: HidAccessMode::Direct,
    interface: 0,
    report_id: 0x03,
    report_mode: HidRawReportMode::OutputReportWithReportId,
    max_report_len: 1025,
    usage_page: Some(0xFF00),
    usage: Some(0x0001),
});

#[derive(Debug)]
struct ResolutionCase {
    name: &'static str,
    intent: TransportIntent,
    platform: TransportPlatform,
    expected: ExpectedResolution,
}

#[derive(Debug)]
enum ExpectedResolution {
    Supported(TransportType),
    Unsupported { transport: &'static str },
}

#[test]
fn transport_intents_resolve_across_the_supported_os_matrix() {
    let cases = [
        ResolutionCase {
            name: "ASUS uses hidraw on Linux",
            intent: ASUS_INTENT,
            platform: TransportPlatform::Linux,
            expected: ExpectedResolution::Supported(TransportType::UsbHidRaw {
                interface: 2,
                report_id: 0xEC,
                report_mode: HidRawReportMode::OutputReport,
                usage_page: None,
                usage: None,
            }),
        },
        ResolutionCase {
            name: "ASUS uses HIDAPI on macOS",
            intent: ASUS_INTENT,
            platform: TransportPlatform::MacOs,
            expected: ExpectedResolution::Supported(TransportType::UsbHidApi {
                interface: Some(2),
                report_id: 0xEC,
                report_mode: HidRawReportMode::OutputReport,
                max_report_len: 65,
                usage_page: None,
                usage: None,
            }),
        },
        ResolutionCase {
            name: "ASUS uses HIDAPI on Windows",
            intent: ASUS_INTENT,
            platform: TransportPlatform::Windows,
            expected: ExpectedResolution::Supported(TransportType::UsbHidApi {
                interface: Some(2),
                report_id: 0xEC,
                report_mode: HidRawReportMode::OutputReport,
                max_report_len: 65,
                usage_page: None,
                usage: None,
            }),
        },
        ResolutionCase {
            name: "direct HID claims the interface on Linux",
            intent: DIRECT_HID_INTENT,
            platform: TransportPlatform::Linux,
            expected: ExpectedResolution::Supported(TransportType::UsbHid { interface: 0 }),
        },
        ResolutionCase {
            name: "direct HID preserves the macOS HID stack",
            intent: DIRECT_HID_INTENT,
            platform: TransportPlatform::MacOs,
            expected: ExpectedResolution::Supported(TransportType::UsbHidApi {
                interface: Some(0),
                report_id: 0x03,
                report_mode: HidRawReportMode::OutputReportWithReportId,
                max_report_len: 1025,
                usage_page: Some(0xFF00),
                usage: Some(0x0001),
            }),
        },
        ResolutionCase {
            name: "direct HID preserves the Windows HID stack",
            intent: DIRECT_HID_INTENT,
            platform: TransportPlatform::Windows,
            expected: ExpectedResolution::Supported(TransportType::UsbHidApi {
                interface: Some(0),
                report_id: 0x03,
                report_mode: HidRawReportMode::OutputReportWithReportId,
                max_report_len: 1025,
                usage_page: Some(0xFF00),
                usage: Some(0x0001),
            }),
        },
        ResolutionCase {
            name: "SMBus is available on Linux",
            intent: TransportIntent::I2cSmBus { address: 0x40 },
            platform: TransportPlatform::Linux,
            expected: ExpectedResolution::Supported(TransportType::I2cSmBus { address: 0x40 }),
        },
        ResolutionCase {
            name: "SMBus is available through PawnIO on Windows",
            intent: TransportIntent::I2cSmBus { address: 0x40 },
            platform: TransportPlatform::Windows,
            expected: ExpectedResolution::Supported(TransportType::I2cSmBus { address: 0x40 }),
        },
        ResolutionCase {
            name: "SMBus is unavailable on macOS",
            intent: TransportIntent::I2cSmBus { address: 0x40 },
            platform: TransportPlatform::MacOs,
            expected: ExpectedResolution::Unsupported { transport: "SMBus" },
        },
        ResolutionCase {
            name: "HID is unavailable without a backend",
            intent: ASUS_INTENT,
            platform: TransportPlatform::Other("unsupported-os"),
            expected: ExpectedResolution::Unsupported { transport: "HID" },
        },
    ];

    for case in cases {
        let actual = resolve_transport(case.intent, case.platform);
        match case.expected {
            ExpectedResolution::Supported(expected) => {
                assert_eq!(actual.expect(case.name), expected, "{}", case.name);
            }
            ExpectedResolution::Unsupported { transport } => {
                let error = actual.expect_err(case.name);
                assert!(
                    matches!(
                        error,
                        TransportError::UnsupportedPlatform {
                            transport: actual_transport,
                            platform,
                        } if actual_transport == transport && platform == case.platform
                    ),
                    "{}: {error:?}",
                    case.name
                );
            }
        }
    }
}

#[tokio::test]
async fn hid_raw_open_reports_platform_support_without_caller_branching() {
    let request = hypercolor_hal::transport::HidRawOpenRequest {
        vendor_id: 0xFFFF,
        product_id: 0xFFFF,
        interface: 0,
        report_id: 0x00,
        report_mode: HidRawReportMode::OutputReport,
        serial: None,
        usb_path: None,
        usage_page: None,
        usage: None,
    };

    let error = hypercolor_hal::transport::open_hid_raw_transport(request)
        .await
        .err()
        .expect("a vendor id of 0xFFFF never matches a real hidraw node");

    if TransportPlatform::CURRENT == TransportPlatform::Linux {
        assert!(
            !matches!(error, TransportError::UnsupportedPlatform { .. }),
            "Linux has a hidraw backend: {error}"
        );
    } else {
        assert!(
            matches!(
                error,
                TransportError::UnsupportedPlatform {
                    transport: "hidraw",
                    platform: TransportPlatform::CURRENT,
                }
            ),
            "unexpected error: {error}"
        );
    }
}
