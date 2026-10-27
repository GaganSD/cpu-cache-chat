//! Frame codec, application payloads, and stop-and-wait link.

mod codec;
mod frame;
mod link;

pub use codec::{decode_frame, encode_frame};
pub use frame::{AppMessage, Frame, FrameType, GameCtrl};
pub use link::{new_session_id, Link, LinkEvent, Outbound};

use std::time::Duration;

use ccc_core::Error;

/// Byte-blob wire used by the link. Implemented by physical and sim channels.
pub trait Wire: Send {
    /// Transmit the complete blob within the supplied time budget.
    fn send_blob(&mut self, bytes: &[u8], timeout: Duration) -> Result<(), Error>;
    /// Receive a complete blob, waiting at most `timeout` for this call.
    /// A streaming physical wire retains valid partial receive progress when
    /// returning `Error::Timeout`, so bounded UI polls do not discard frames.
    fn recv_blob(&mut self, timeout: Duration) -> Result<Vec<u8>, Error>;
    /// True while a recognized incoming physical frame is being received. The
    /// link must not transmit until that frame completes or is abandoned.
    /// Idle listening alone is not a frame in progress.
    fn receive_in_progress(&self) -> bool {
        false
    }
    fn interrupt(&mut self);
}

impl Wire for Box<dyn Wire> {
    fn send_blob(&mut self, bytes: &[u8], timeout: Duration) -> Result<(), Error> {
        (**self).send_blob(bytes, timeout)
    }
    fn recv_blob(&mut self, timeout: Duration) -> Result<Vec<u8>, Error> {
        (**self).recv_blob(timeout)
    }
    fn receive_in_progress(&self) -> bool {
        (**self).receive_in_progress()
    }
    fn interrupt(&mut self) {
        (**self).interrupt();
    }
}
