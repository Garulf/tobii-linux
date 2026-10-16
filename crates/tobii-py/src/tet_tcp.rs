//! tobii_linux.tet_tcp: the recording proxy's read-only command policy. The proxy's
//! sockets, threads and router stay in Python.

use pyo3::prelude::*;
use pyo3::types::PyBytes;
use tobii_service::tet_tcp as tet;

use crate::ttp::message_arg;

/// Wire stream id of a SUBSCRIBE_STREAM or UNSUBSCRIBE_STREAM request, if readable.
#[pyfunction]
fn subscribed_stream(message: &Bound<'_, PyAny>) -> PyResult<Option<u32>> {
    Ok(tet::subscribed_stream(&message_arg(message)?))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(subscribed_stream, m)?)?;
    m.add("READ_COMMANDS", tet::READ_COMMANDS.to_vec())?;
    m.add("CHANNEL_PORTS", tet::CHANNEL_PORTS.to_vec())?;
    m.add("COMMAND_PORT", tet::COMMAND_PORT)?;
    m.add("TIMESYNC_UDP_PORT", tet::TIMESYNC_UDP_PORT)?;
    m.add("SERVICE_UNLOCK_KEY", PyBytes::new(m.py(), tet::SERVICE_UNLOCK_KEY))?;
    m.add("UNSUPPORTED_OPERATION", tet::UNSUPPORTED_OPERATION)?;
    m.add("DISPLAY_AREA_WRITABLE", tet::DISPLAY_AREA_WRITABLE)?;
    m.add("PERSISTENT_FILE_ENUMERATE", tet::PERSISTENT_FILE_ENUMERATE)?;
    m.add("PERSISTENT_FILE_READ", tet::PERSISTENT_FILE_READ)?;
    Ok(())
}
