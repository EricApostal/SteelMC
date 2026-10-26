use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use steel_protocol::packet_traits::{CompressionInfo, EncodedPacket};
use steel_utils::locks::SyncMutex;
use text_components::TextComponent;

use crate::player::connection::NetworkConnection;

#[derive(Default)]
pub(crate) struct TestConnection {
    closed: AtomicBool,
    sent: Arc<SyncMutex<Vec<EncodedPacket>>>,
}

impl TestConnection {
    /// Returns a handle to every packet this connection sent, so tests can
    /// assert that a broadcast reached the client.
    pub(crate) fn sent_handle(&self) -> Arc<SyncMutex<Vec<EncodedPacket>>> {
        Arc::clone(&self.sent)
    }
}

impl NetworkConnection for TestConnection {
    fn compression(&self) -> Option<CompressionInfo> {
        None
    }

    fn send_encoded(&self, packet: EncodedPacket) {
        self.sent.lock().push(packet);
    }

    fn send_encoded_bundle(&self, packets: Vec<EncodedPacket>) {
        self.sent.lock().extend(packets);
    }

    fn disconnect_with_reason(&self, _reason: TextComponent) {
        self.close();
    }

    fn tick(&self) {}

    fn latency(&self) -> i32 {
        0
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    fn closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}
