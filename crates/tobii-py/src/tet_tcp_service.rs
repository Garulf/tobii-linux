//! tobii_linux.tet_tcp_service: answers to game requests from the device's captured replies,
//! and stream restamping. The asyncio endpoint stays in Python.

use std::collections::{BTreeSet, HashMap};

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};
use tobii_service::tet_tcp::{self as tet, Answer};

use crate::convert::bytes_like;
use crate::errors::IntoPyResult;
use crate::ttp::{PyMessage, message_arg};

/// Stream payload with the int64 of column 1 replaced; every other byte unchanged.
#[pyfunction]
fn restamp<'py>(py: Python<'py>, payload: &Bound<'py, PyAny>, timestamp_us: i64) -> PyResult<Bound<'py, PyBytes>> {
    Ok(PyBytes::new(py, &tet::restamp(&bytes_like(payload)?, timestamp_us).py()?))
}

#[pyfunction]
fn reply_to(py: Python<'_>, message: &Bound<'_, PyAny>, cached: &Bound<'_, PyAny>) -> PyResult<PyMessage> {
    Ok(PyMessage::from_core(py, &tet::reply_to(&message_arg(message)?, &message_arg(cached)?)))
}

#[pyfunction]
fn refusal(py: Python<'_>, message: &Bound<'_, PyAny>) -> PyResult<PyMessage> {
    Ok(PyMessage::from_core(py, &tet::refusal(&message_arg(message)?)))
}

/// `(action, reply, stream)`: action is "ignore", "reply", "refuse", "refuse-stream",
/// "subscribe" or "unsubscribe"; `replies` maps commands to the device's replies and
/// `served` holds the wire streams subscribed on the device.
#[pyfunction]
fn answer(
    py: Python<'_>,
    message: &Bound<'_, PyAny>,
    replies: &Bound<'_, PyDict>,
    served: &Bound<'_, PyAny>,
) -> PyResult<(&'static str, Option<PyMessage>, Option<u32>)> {
    let replies: HashMap<u32, tobii_protocol::Message> = replies
        .iter()
        .map(|(command, reply)| Ok((command.extract()?, message_arg(&reply)?)))
        .collect::<PyResult<_>>()?;
    let served: BTreeSet<u32> = served.try_iter()?.map(|stream| stream?.extract()).collect::<PyResult<_>>()?;
    let reply = |message: &tobii_protocol::Message| Some(PyMessage::from_core(py, message));
    Ok(match tet::answer(&message_arg(message)?, &replies, &served) {
        Answer::Ignore => ("ignore", None, None),
        Answer::Reply(message) => ("reply", reply(&message), None),
        Answer::Refuse { reply: message, stream, subscription: false } => ("refuse", reply(&message), stream),
        Answer::Refuse { reply: message, stream, subscription: true } => ("refuse-stream", reply(&message), stream),
        Answer::Subscribe { reply: message, stream, subscribe } => {
            (if subscribe { "subscribe" } else { "unsubscribe" }, reply(&message), Some(stream))
        }
    })
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    for function in [
        wrap_pyfunction!(restamp, m)?,
        wrap_pyfunction!(reply_to, m)?,
        wrap_pyfunction!(refusal, m)?,
        wrap_pyfunction!(answer, m)?,
    ] {
        m.add_function(function)?;
    }
    m.add("GAZE_STREAM", tet::GAZE_STREAM)?;
    m.add("PRESENCE_STREAM", tet::PRESENCE_STREAM)?;
    m.add("CAPTURED_GETTERS", tet::CAPTURED_GETTERS.to_vec())?;
    m.add("REPLY_WORD2", tet::REPLY_WORD2)?;
    m.add("MAX_PENDING_BYTES", tet::MAX_PENDING_BYTES)?;
    m.add("TIMESTAMP_COLUMN", tet::TIMESTAMP_COLUMN)?;
    Ok(())
}
