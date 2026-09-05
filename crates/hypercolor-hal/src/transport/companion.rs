//! A device made of two USB functions, presented as one transport.
//!
//! Some controllers split their protocol across two USB devices that share a
//! parent hub: the Lian Li L-Wireless controller sends RF through one bulk
//! device and answers discovery polls through its RX sibling. A protocol
//! stays a pure encoder by tagging each command with the path it needs:
//! [`TransferType::Companion`] reaches the sibling, every other transfer type
//! reaches the primary device unchanged.
//!
//! Which sibling belongs to which primary is the driver's business (it knows
//! the vendor's pairing rule); this type only owns the routing.

use std::time::Duration;

use async_trait::async_trait;

use crate::protocol::TransferType;
use crate::transport::{Transport, TransportError};

/// One logical transport over a primary device and its companion.
pub struct CompanionTransport {
    name: &'static str,
    primary: Box<dyn Transport>,
    companion: Box<dyn Transport>,
}

impl CompanionTransport {
    /// Pair `primary` with `companion` under `name`.
    #[must_use]
    pub fn new(
        name: &'static str,
        primary: Box<dyn Transport>,
        companion: Box<dyn Transport>,
    ) -> Self {
        Self {
            name,
            primary,
            companion,
        }
    }

    /// The transport a transfer type reaches, and the type it sees there:
    /// the companion is addressed on its own default path.
    fn route(&self, transfer_type: TransferType) -> (&dyn Transport, TransferType) {
        match transfer_type {
            TransferType::Companion => (self.companion.as_ref(), TransferType::Primary),
            other => (self.primary.as_ref(), other),
        }
    }
}

#[async_trait]
impl Transport for CompanionTransport {
    fn name(&self) -> &'static str {
        self.name
    }

    fn supports_parallel_transfer_lanes(&self) -> bool {
        self.primary.supports_parallel_transfer_lanes()
    }

    async fn send(&self, data: &[u8]) -> Result<(), TransportError> {
        self.primary.send(data).await
    }

    async fn send_with_type(
        &self,
        data: &[u8],
        transfer_type: TransferType,
    ) -> Result<(), TransportError> {
        let (transport, transfer_type) = self.route(transfer_type);
        transport.send_with_type(data, transfer_type).await
    }

    async fn send_owned_with_type(
        &self,
        data: Vec<u8>,
        transfer_type: TransferType,
    ) -> Result<(), TransportError> {
        let (transport, transfer_type) = self.route(transfer_type);
        transport.send_owned_with_type(data, transfer_type).await
    }

    async fn receive(&self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        self.primary.receive(timeout).await
    }

    async fn receive_with_type(
        &self,
        timeout: Duration,
        transfer_type: TransferType,
    ) -> Result<Vec<u8>, TransportError> {
        let (transport, transfer_type) = self.route(transfer_type);
        transport.receive_with_type(timeout, transfer_type).await
    }

    async fn send_receive(
        &self,
        data: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, TransportError> {
        self.primary.send_receive(data, timeout).await
    }

    async fn send_receive_with_type(
        &self,
        data: &[u8],
        timeout: Duration,
        transfer_type: TransferType,
    ) -> Result<Vec<u8>, TransportError> {
        let (transport, transfer_type) = self.route(transfer_type);
        transport
            .send_receive_with_type(data, timeout, transfer_type)
            .await
    }

    async fn receive_logical(
        &self,
        timeout: Duration,
        transfer_type: TransferType,
        capacity: Option<usize>,
    ) -> Result<Vec<u8>, TransportError> {
        let (transport, transfer_type) = self.route(transfer_type);
        transport
            .receive_logical(timeout, transfer_type, capacity)
            .await
    }

    async fn send_receive_logical(
        &self,
        data: &[u8],
        timeout: Duration,
        transfer_type: TransferType,
        capacity: Option<usize>,
    ) -> Result<Vec<u8>, TransportError> {
        let (transport, transfer_type) = self.route(transfer_type);
        transport
            .send_receive_logical(data, timeout, transfer_type, capacity)
            .await
    }

    /// Both halves close; the first failure is reported after the second
    /// half has had its chance to release its handle.
    async fn close(&self) -> Result<(), TransportError> {
        let primary = self.primary.close().await;
        let companion = self.companion.close().await;
        primary.and(companion)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Records which path each call arrived on, and answers with its label.
    struct Half {
        label: &'static str,
        sends: Mutex<Vec<(Vec<u8>, TransferType)>>,
        reads: Mutex<Vec<(TransferType, Option<usize>)>>,
        closed: Mutex<bool>,
    }

    impl Half {
        fn new(label: &'static str) -> Self {
            Self {
                label,
                sends: Mutex::new(Vec::new()),
                reads: Mutex::new(Vec::new()),
                closed: Mutex::new(false),
            }
        }
    }

    #[async_trait]
    impl Transport for Half {
        fn name(&self) -> &'static str {
            self.label
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
                .expect("sends lock")
                .push((data.to_vec(), transfer_type));
            Ok(())
        }

        async fn receive(&self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
            self.receive_logical(timeout, TransferType::Primary, None)
                .await
        }

        async fn receive_logical(
            &self,
            _timeout: Duration,
            transfer_type: TransferType,
            capacity: Option<usize>,
        ) -> Result<Vec<u8>, TransportError> {
            self.reads
                .lock()
                .expect("reads lock")
                .push((transfer_type, capacity));
            Ok(self.label.as_bytes().to_vec())
        }

        async fn send_receive_logical(
            &self,
            data: &[u8],
            timeout: Duration,
            transfer_type: TransferType,
            capacity: Option<usize>,
        ) -> Result<Vec<u8>, TransportError> {
            self.send_with_type(data, transfer_type).await?;
            self.receive_logical(timeout, transfer_type, capacity).await
        }

        async fn close(&self) -> Result<(), TransportError> {
            *self.closed.lock().expect("closed lock") = true;
            Ok(())
        }
    }

    fn pair() -> (CompanionTransport, &'static Half, &'static Half) {
        let primary: &'static Half = Box::leak(Box::new(Half::new("primary")));
        let companion: &'static Half = Box::leak(Box::new(Half::new("companion")));
        let transport = CompanionTransport::new(
            "pair",
            Box::new(HalfRef(primary)),
            Box::new(HalfRef(companion)),
        );
        (transport, primary, companion)
    }

    /// Lets the test keep a handle to each half after boxing it.
    struct HalfRef(&'static Half);

    #[async_trait]
    impl Transport for HalfRef {
        fn name(&self) -> &'static str {
            self.0.name()
        }
        async fn send(&self, data: &[u8]) -> Result<(), TransportError> {
            self.0.send(data).await
        }
        async fn send_with_type(
            &self,
            data: &[u8],
            transfer_type: TransferType,
        ) -> Result<(), TransportError> {
            self.0.send_with_type(data, transfer_type).await
        }
        async fn receive(&self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
            self.0.receive(timeout).await
        }
        async fn receive_logical(
            &self,
            timeout: Duration,
            transfer_type: TransferType,
            capacity: Option<usize>,
        ) -> Result<Vec<u8>, TransportError> {
            self.0
                .receive_logical(timeout, transfer_type, capacity)
                .await
        }
        async fn send_receive_logical(
            &self,
            data: &[u8],
            timeout: Duration,
            transfer_type: TransferType,
            capacity: Option<usize>,
        ) -> Result<Vec<u8>, TransportError> {
            self.0
                .send_receive_logical(data, timeout, transfer_type, capacity)
                .await
        }
        async fn close(&self) -> Result<(), TransportError> {
            self.0.close().await
        }
    }

    #[tokio::test]
    async fn companion_transfers_reach_the_sibling_on_its_default_path() {
        let (transport, primary, companion) = pair();

        let reply = transport
            .send_receive_logical(
                &[0x10, 0x01],
                Duration::ZERO,
                TransferType::Companion,
                Some(1024),
            )
            .await
            .expect("companion send_receive");

        assert_eq!(reply, b"companion");
        assert_eq!(
            companion.sends.lock().expect("sends").as_slice(),
            &[(vec![0x10, 0x01], TransferType::Primary)],
            "the sibling sees its own default path, not the companion tag"
        );
        assert_eq!(
            companion.reads.lock().expect("reads").as_slice(),
            &[(TransferType::Primary, Some(1024))],
            "read capacity travels with the routed call"
        );
        assert!(primary.sends.lock().expect("sends").is_empty());
    }

    #[tokio::test]
    async fn every_other_transfer_type_reaches_the_primary_unchanged() {
        let (transport, primary, companion) = pair();

        transport
            .send_with_type(&[0x11], TransferType::Bulk)
            .await
            .expect("bulk send");
        transport.send(&[0x12]).await.expect("plain send");
        let reply = transport
            .receive_logical(Duration::ZERO, TransferType::Primary, Some(64))
            .await
            .expect("primary read");

        assert_eq!(reply, b"primary");
        assert_eq!(
            primary.sends.lock().expect("sends").as_slice(),
            &[
                (vec![0x11], TransferType::Bulk),
                (vec![0x12], TransferType::Primary)
            ]
        );
        assert!(companion.sends.lock().expect("sends").is_empty());
        assert!(companion.reads.lock().expect("reads").is_empty());
    }

    #[tokio::test]
    async fn closing_closes_both_halves() {
        let (transport, primary, companion) = pair();

        transport.close().await.expect("close");

        assert!(*primary.closed.lock().expect("closed"));
        assert!(*companion.closed.lock().expect("closed"));
    }
}
