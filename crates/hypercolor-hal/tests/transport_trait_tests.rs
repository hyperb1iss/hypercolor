use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use hypercolor_hal::protocol::TransferType;
use hypercolor_hal::transport::{Transport, TransportError};

type SendLog = Arc<Mutex<Vec<(TransferType, Vec<u8>)>>>;

#[derive(Clone, Default)]
struct RecordingTransport {
    sends: SendLog,
}

#[async_trait]
impl Transport for RecordingTransport {
    fn name(&self) -> &'static str {
        "recording"
    }

    async fn send(&self, data: &[u8]) -> Result<(), TransportError> {
        self.send_with_type(data, TransferType::Primary).await
    }

    async fn send_with_type(
        &self,
        data: &[u8],
        transfer_type: TransferType,
    ) -> Result<(), TransportError> {
        self.sends
            .lock()
            .expect("send log lock should not be poisoned")
            .push((transfer_type, data.to_vec()));
        Ok(())
    }

    async fn receive(&self, _timeout: Duration) -> Result<Vec<u8>, TransportError> {
        Ok(Vec::new())
    }

    async fn close(&self) -> Result<(), TransportError> {
        Ok(())
    }
}

#[tokio::test]
async fn send_owned_with_type_uses_transport_specific_routing() {
    let transport = RecordingTransport::default();

    transport
        .send_owned_with_type(vec![0xDE, 0xAD, 0xBE, 0xEF], TransferType::Bulk)
        .await
        .expect("owned send should delegate to send_with_type");

    let sends = transport
        .sends
        .lock()
        .expect("send log lock should not be poisoned");
    assert_eq!(sends.len(), 1);
    assert_eq!(sends[0].0, TransferType::Bulk);
    assert_eq!(sends[0].1, vec![0xDE, 0xAD, 0xBE, 0xEF]);
}
