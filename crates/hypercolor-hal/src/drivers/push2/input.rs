//! Relay from the Push 2 MIDI input callback to the host's input sink.

use std::sync::Arc;

use hypercolor_types::device_input::DeviceInputEdge;

use super::protocol::{PUSH2_PAD_COUNT, Push2InputDecode, Push2InputDecoder};
use crate::transport::input::TransportInputSink;

/// Decodes each MIDI input message and publishes what it carried.
///
/// The relay lives inside the native MIDI input callback, so dropping it is
/// the moment input from that connection ends: the sink hears
/// [`TransportInputSink::closed`] whenever the input connection closes, the
/// transport is dropped, or the connection never opened.
pub struct Push2InputRelay {
    decoder: Push2InputDecoder,
    edges: Vec<DeviceInputEdge>,
    sink: Arc<dyn TransportInputSink>,
}

impl Push2InputRelay {
    /// Relay input to `sink`.
    #[must_use]
    pub fn new(sink: Arc<dyn TransportInputSink>) -> Self {
        Self {
            decoder: Push2InputDecoder::new(),
            // Channel pressure can move every pad at once, plus the strip.
            edges: Vec::with_capacity(PUSH2_PAD_COUNT + 1),
            sink,
        }
    }

    /// Decode one MIDI input message and publish its edges, then report an
    /// interruption when input was lost.
    ///
    /// Runs on the native input thread for every message, sysex replies
    /// included, so it only decodes and hands off. Decoding allocates
    /// nothing once warm and takes no lock the output or display paths
    /// share; publishing costs whatever the host's sink does with the edges,
    /// and only messages that carry input publish.
    pub fn receive(&mut self, message: &[u8]) {
        self.edges.clear();
        let decoded = self.decoder.decode(message, &mut self.edges);
        if !self.edges.is_empty() {
            self.sink.publish(&self.edges);
        }
        if decoded == Push2InputDecode::Lost {
            self.sink.interrupted();
        }
    }
}

impl Drop for Push2InputRelay {
    fn drop(&mut self) {
        self.sink.closed();
    }
}
