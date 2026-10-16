//! Eye Tracker 5 USB access on Linux through `nusb`: descriptor probing, the bulk
//! transport on interface 0, vendor channels and the continuous channel reader.
//!
//! No vendor commands beyond channel open (0x41) and close (0x42) are sent here; the
//! backend never detaches kernel drivers or changes the device configuration.

pub mod channel;
pub mod descriptors;
pub mod error;
pub mod nusb_backend;
pub mod traffic;
pub mod transport;

pub use channel::{ReadMode, UsbChannel};
pub use descriptors::{Alternate, Configuration, DeviceDescriptor, Endpoint, ProbedDevice, bulk_pair};
pub use error::{Error, Result, UsbError};
pub use nusb_backend::{BulkTransport, open_transport, probe};
pub use transport::{ContinuousSource, Transport};

pub const VID: u16 = 0x2104;
pub const PID: u16 = 0x0313;
pub const MAX_TRANSFER: usize = 1024 * 1024;
