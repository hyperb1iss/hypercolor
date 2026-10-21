//! Input a transport decodes from the device it drives.
//!
//! Some hardware a driver lights can also be played. A transport that reads
//! that input decodes it into [`DeviceInputEdge`]s and hands them to the
//! [`TransportInputSink`] its opener supplied. The sink belongs to the host,
//! which attributes the edges to the device and decides when they count; the
//! transport reports what it saw, and when its view of the device's input
//! broke or ended, so the host can cancel holds whose lift it will never see.

use hypercolor_types::device_input::DeviceInputEdge;

/// Host-side receiver for the input one transport decodes.
///
/// Calls arrive on whatever thread the transport reads input on, often a
/// native callback thread, so implementations return promptly and never
/// block on device I/O.
pub trait TransportInputSink: Send + Sync {
    /// Publish edges in the order the device reported them.
    fn publish(&self, edges: &[DeviceInputEdge]);

    /// Some input may have been lost, so a lift may have gone unseen.
    ///
    /// The host cancels whatever the device held. Input keeps flowing, and
    /// edges published afterward start from nothing held.
    fn interrupted(&self);

    /// The transport stopped reading input for good.
    ///
    /// The host cancels whatever the device held and ignores anything
    /// published afterward.
    fn closed(&self);
}
