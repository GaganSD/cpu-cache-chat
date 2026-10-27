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
    fn send_blob(&mut self, bytes: &[u8], timeout: Duration) -> Result<(), Error>;
    fn recv_blob(&mut self, timeout: Duration) -> Result<Vec<u8>, Error>;
    fn interrupt(&mut self);
}

impl Wire for Box<dyn Wire> {
    fn send_blob(&mut self, bytes: &[u8], timeout: Duration) -> Result<(), Error> {
        (**self).send_blob(bytes, timeout)
    }
    fn recv_blob(&mut self, timeout: Duration) -> Result<Vec<u8>, Error> {
        (**self).recv_blob(timeout)
    }
    fn interrupt(&mut self) {
        (**self).interrupt();
    }
}
