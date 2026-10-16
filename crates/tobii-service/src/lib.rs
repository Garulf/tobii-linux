//! The game service: the wire formats and state behind the PRP, SESP and tet-tcp routes to
//! the game DLL, and the service clock that follows Wine's QueryPerformanceCounter.
//!
//! Codecs are pure and checked byte for byte against the original builders by
//! `tools/prp_encoder_oracle.py`, `tools/prp_session_oracle.py` and `tools/sesp_oracle.py`
//! through the Python bindings. The asyncio servers that carry these messages stay in the
//! Python package; this crate holds what they decide and encode.

pub mod error;
pub mod prp;
pub mod prp_service;
pub mod prp_session;
pub mod prp_streams;
pub mod qpc_clock;
pub mod sesp;
pub mod sesp_service;
pub mod tet_tcp;

pub use error::{Result, ServiceError};

#[cfg(test)]
pub(crate) mod test_util;
