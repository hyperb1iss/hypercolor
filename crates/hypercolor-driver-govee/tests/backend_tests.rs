use hypercolor_core::device::DeviceBackend;
use hypercolor_driver_govee::backend::GoveeBackend;
use hypercolor_driver_govee::{GoveeLanDevice, build_device_info};
use hypercolor_types::config::GoveeConfig;
use tokio::net::UdpSocket;
use tokio::time::{Duration, timeout};

#[tokio::test]
async fn write_colors_dedups_and_paces_lan_state_frames() {
    let socket = UdpSocket::bind(("127.0.0.1", 0))
        .await
        .expect("test UDP socket should bind");
    let target = socket.local_addr().expect("test socket has local addr");
    let device = test_device("H6163");
    let device_id = build_device_info(&device).id;
    let mut backend = GoveeBackend::new(GoveeConfig {
        lan_state_fps: 10,
        ..GoveeConfig::default()
    });
    backend.remember_device_at(device, target);

    backend
        .write_colors(&device_id, &[[255, 0, 0], [0, 0, 255]])
        .await
        .expect("first frame should send");
    let first = recv_payload(&socket).await;
    assert!(first.contains("\"colorwc\""));
    assert!(first.contains("\"r\":127"));
    assert!(first.contains("\"b\":127"));

    backend
        .write_colors(&device_id, &[[255, 0, 0], [0, 0, 255]])
        .await
        .expect("duplicate frame should be skipped");
    assert_no_payload(&socket).await;

    backend
        .write_colors(&device_id, &[[0, 255, 0]])
        .await
        .expect("paced frame should be skipped");
    assert_no_payload(&socket).await;

    tokio::time::sleep(Duration::from_millis(110)).await;
    backend
        .write_colors(&device_id, &[[0, 255, 0]])
        .await
        .expect("frame after pacing window should send");
    let second = recv_payload(&socket).await;
    assert!(second.contains("\"g\":255"));
}

fn test_device(sku: &str) -> GoveeLanDevice {
    GoveeLanDevice {
        ip: "127.0.0.1".parse().expect("valid test IP"),
        sku: sku.to_owned(),
        mac: "aabbccddeeff".to_owned(),
        name: "Test Govee".to_owned(),
        firmware_version: None,
    }
}

async fn recv_payload(socket: &UdpSocket) -> String {
    let mut buf = [0_u8; 2048];
    let (len, _) = timeout(Duration::from_millis(200), socket.recv_from(&mut buf))
        .await
        .expect("payload should arrive")
        .expect("UDP receive should succeed");
    String::from_utf8(buf[..len].to_vec()).expect("payload should be UTF-8")
}

async fn assert_no_payload(socket: &UdpSocket) {
    let mut buf = [0_u8; 2048];
    assert!(
        timeout(Duration::from_millis(50), socket.recv_from(&mut buf))
            .await
            .is_err(),
        "unexpected UDP payload"
    );
}
