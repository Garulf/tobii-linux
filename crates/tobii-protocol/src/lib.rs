//! Wire formats of the Eye Tracker 5 as the Windows platform runtime builds and reads them.
//!
//! Everything here is pure: no I/O, no clocks, no threads. Encoders and decoders are
//! checked byte for byte against the original builders by the emulation oracles in
//! `tools/*_oracle.py` through the Python bindings.

pub mod camera_info;
pub mod commands;
pub mod error;
pub mod fields;
pub mod frames;
pub mod framing;
pub mod gaze;
pub mod numeric;
pub mod ttp;

pub use error::{FramingError, ProtocolError};
pub use ttp::{Field, Message};

#[cfg(test)]
pub(crate) mod test_util {
    pub fn hex(text: &str) -> Vec<u8> {
        let digits: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
        digits.chunks(2).map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap()).collect()
    }
}
