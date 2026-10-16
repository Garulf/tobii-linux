//! tobii_linux.prp_session: the PRP client session codecs.
//!
//! Messages cross the boundary as tuples in dataclass field order; the Python module wraps
//! them in its dataclasses. Values the Python codecs range-checked are refused with the
//! same ProtocolError messages.

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyTuple};
use tobii_service::prp::DEFAULT_MAX_PAYLOAD;
use tobii_service::prp_session::{
    self as session, ClientHello, CommandResponse, ConnectRequest, ConnectResponse, HandshakeInfo, PropertyResponse,
    ResponseBody, ResponseKind,
};

use crate::convert::bytes_like;
use crate::errors::IntoPyResult;
use crate::prp::{i32_arg, masked_u32, prp_error, ranged, u32_arg};

fn bytes<'py>(py: Python<'py>, data: &[u8]) -> Bound<'py, PyBytes> {
    PyBytes::new(py, data)
}

fn ints<'py>(py: Python<'py>, values: &[i32]) -> PyResult<Bound<'py, PyTuple>> {
    PyTuple::new(py, values)
}

fn int_list(object: &Bound<'_, PyAny>) -> PyResult<Vec<i32>> {
    object.try_iter()?.map(|value| i32_arg(&value?)).collect()
}

fn strings(object: &Bound<'_, PyAny>) -> PyResult<Vec<String>> {
    object.try_iter()?.map(|value| value?.extract()).collect()
}

// Handshake ----------------------------------------------------------------------------

#[pyfunction]
fn server_pipe_names(url: &str) -> PyResult<(String, String)> {
    session::server_pipe_names(url).py()
}

#[pyfunction]
fn client_pipe_name(server_pipe: &str, pid: &Bound<'_, PyAny>, counter: &Bound<'_, PyAny>) -> PyResult<String> {
    let counter: u64 = ranged(counter, "counter is outside unsigned 64-bit range")?;
    Ok(session::client_pipe_name(server_pipe, u32_arg(pid)?, counter))
}

#[pyfunction]
fn encode_client_hello<'py>(py: Python<'py>, hello: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyBytes>> {
    let reply_pipe: String = hello.getattr("reply_pipe")?.extract()?;
    let hello = ClientHello {
        version: u32_arg(&hello.getattr("version")?)?,
        channel: u32_arg(&hello.getattr("channel")?)?,
        reply_pipe,
    };
    Ok(bytes(py, &session::encode_client_hello(&hello).py()?))
}

#[pyfunction]
fn decode_client_hello(message: &Bound<'_, PyAny>) -> PyResult<(u32, String, u32)> {
    let hello = session::decode_client_hello(&bytes_like(message)?).py()?;
    Ok((hello.channel, hello.reply_pipe, hello.version))
}

#[pyfunction]
fn encode_handshake_ack(py: Python<'_>) -> Bound<'_, PyBytes> {
    bytes(py, &session::encode_handshake_ack())
}

#[pyfunction]
fn check_handshake_ack(message: &Bound<'_, PyAny>) -> PyResult<()> {
    session::check_handshake_ack(&bytes_like(message)?).py()
}

#[pyfunction]
fn encode_client_pid<'py>(py: Python<'py>, pid: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyBytes>> {
    Ok(bytes(py, &session::encode_client_pid(u32_arg(pid)?)))
}

#[pyfunction]
fn decode_client_pid(message: &Bound<'_, PyAny>) -> PyResult<u32> {
    session::decode_client_pid(&bytes_like(message)?).py()
}

#[pyfunction]
fn encode_handshake_info<'py>(py: Python<'py>, info: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyBytes>> {
    let subscription_url: String = info.getattr("subscription_url")?.extract()?;
    let timesync_url: String = info.getattr("timesync_url")?.extract()?;
    let info = HandshakeInfo { client_id: i32_arg(&info.getattr("client_id")?)?, subscription_url, timesync_url };
    Ok(bytes(py, &session::encode_handshake_info(&info).py()?))
}

#[pyfunction]
fn decode_handshake_info(message: &Bound<'_, PyAny>) -> PyResult<(i32, String, String)> {
    let info = session::decode_handshake_info(&bytes_like(message)?).py()?;
    Ok((info.client_id, info.subscription_url, info.timesync_url))
}

// Framing ------------------------------------------------------------------------------

#[pyfunction]
fn pipe_chunks<'py>(py: Python<'py>, packet: &Bound<'py, PyAny>) -> PyResult<Vec<Bound<'py, PyBytes>>> {
    Ok(session::pipe_chunks(&bytes_like(packet)?).into_iter().map(|chunk| bytes(py, chunk)).collect())
}

#[pyfunction]
#[pyo3(signature = (stream, max_payload=DEFAULT_MAX_PAYLOAD))]
fn split_packets<'py>(
    py: Python<'py>,
    stream: &Bound<'py, PyAny>,
    max_payload: u64,
) -> PyResult<(Vec<Bound<'py, PyBytes>>, Bound<'py, PyBytes>)> {
    let stream = bytes_like(stream)?;
    let (packets, tail) = session::split_packets(&stream, max_payload).py()?;
    Ok((packets.into_iter().map(|packet| bytes(py, packet)).collect(), bytes(py, tail)))
}

// Connect request ----------------------------------------------------------------------

fn connect_request_arg(request: &Bound<'_, PyAny>, extended: bool) -> PyResult<ConnectRequest> {
    let get = |name: &str| request.getattr(name);
    let licenses: Vec<Vec<u8>> =
        get("licenses")?.try_iter()?.map(|license| bytes_like(&license?)).collect::<PyResult<_>>()?;
    let client_version = get("client_version")?;
    let version_items: Vec<Bound<'_, PyAny>> = client_version.try_iter()?.collect::<PyResult<_>>()?;
    if licenses.len() > session::MAX_LICENSES || version_items.len() != 4 {
        return Err(prp_error("connect request has too many licenses or a bad version tuple"));
    }
    Ok(ConnectRequest {
        subscription_client_id: i32_arg(&get("subscription_client_id")?)?,
        licenses,
        signer_public_key: bytes_like(&get("signer_public_key")?)?,
        client_version: version_items.iter().map(i32_arg).collect::<PyResult<_>>()?,
        process_id: u32_arg(&get("process_id")?)?,
        image_directory: bytes_like(&get("image_directory")?)?,
        image_name: bytes_like(&get("image_name")?)?,
        field_of_use: if extended { i32_arg(&get("field_of_use")?)? } else { 0 },
    })
}

fn connect_request_tuple<'py>(py: Python<'py>, request: &ConnectRequest) -> PyResult<Bound<'py, PyTuple>> {
    let licenses = PyTuple::new(py, request.licenses.iter().map(|license| bytes(py, license)))?;
    (
        request.subscription_client_id,
        licenses,
        bytes(py, &request.signer_public_key),
        ints(py, &request.client_version)?,
        request.process_id,
        bytes(py, &request.image_directory),
        bytes(py, &request.image_name),
        request.field_of_use,
    )
        .into_pyobject(py)
}

#[pyfunction]
#[pyo3(signature = (tag, request, extended=true))]
fn encode_connect_request<'py>(
    py: Python<'py>,
    tag: &Bound<'py, PyAny>,
    request: &Bound<'py, PyAny>,
    extended: bool,
) -> PyResult<Bound<'py, PyBytes>> {
    let request = connect_request_arg(request, extended)?;
    Ok(bytes(py, &session::encode_connect_request(u32_arg(tag)?, &request, extended).py()?))
}

#[pyfunction]
fn encode_stream_request<'py>(
    py: Python<'py>,
    tag: &Bound<'py, PyAny>,
    command: &Bound<'py, PyAny>,
    stream: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyBytes>> {
    let command: u32 = ranged(command, "not a stream command")?;
    Ok(bytes(py, &session::encode_stream_request(u32_arg(tag)?, command, i32_arg(stream)?).py()?))
}

#[pyfunction]
fn encode_timesync_request<'py>(py: Python<'py>, tag: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyBytes>> {
    Ok(bytes(py, &session::encode_timesync_request(u32_arg(tag)?)))
}

type CommandRequestTuple<'py> = (u32, u32, Option<i32>, Option<Bound<'py, PyTuple>>);

/// `(tag, command, stream, connect request tuple)`.
#[pyfunction]
#[pyo3(signature = (packet, extended=true))]
fn decode_command_request<'py>(
    py: Python<'py>,
    packet: &Bound<'py, PyAny>,
    extended: bool,
) -> PyResult<CommandRequestTuple<'py>> {
    let request = session::decode_command_request(&bytes_like(packet)?, extended).py()?;
    let connect = request.connect.as_ref().map(|connect| connect_request_tuple(py, connect)).transpose()?;
    Ok((request.tag, request.command, request.stream, connect))
}

// Command responses --------------------------------------------------------------------

pub fn connect_response_arg(response: &Bound<'_, PyAny>) -> PyResult<ConnectResponse> {
    let get = |name: &str| response.getattr(name);
    let groups = get("groups")?
        .try_iter()?
        .map(|group| {
            let group = group?;
            Ok((i32_arg(&group.get_item(0)?)?, int_list(&group.get_item(1)?)?))
        })
        .collect::<PyResult<_>>()?;
    Ok(ConnectResponse {
        build_version: get("build_version")?.extract()?,
        firmware_upgrade_in_progress: get("firmware_upgrade_in_progress")?.is_truthy()?,
        list_a: int_list(&get("list_a")?)?,
        list_b: int_list(&get("list_b")?)?,
        list_c: int_list(&get("list_c")?)?,
        list_d: int_list(&get("list_d")?)?,
        streams: int_list(&get("streams")?)?,
        supported_commands: int_list(&get("supported_commands")?)?,
        groups,
        strings: strings(&get("strings")?)?,
        words: int_list(&get("words")?)?,
        license_results: int_list(&get("license_results")?)?,
        mode: i32_arg(&get("mode")?)?,
        list_g: int_list(&get("list_g")?)?,
        extended_strings: strings(&get("extended_strings")?)?,
    })
}

/// ConnectResponse fields in dataclass order.
pub fn connect_response_tuple<'py>(py: Python<'py>, response: &ConnectResponse) -> PyResult<Bound<'py, PyTuple>> {
    let groups = response
        .groups
        .iter()
        .map(|(kind, values)| (*kind, ints(py, values)?).into_pyobject(py))
        .collect::<PyResult<Vec<_>>>()?;
    let items: Vec<Bound<'py, PyAny>> = vec![
        response.build_version.clone().into_pyobject(py)?.into_any(),
        pyo3::types::PyBool::new(py, response.firmware_upgrade_in_progress).to_owned().into_any(),
        ints(py, &response.list_a)?.into_any(),
        ints(py, &response.list_b)?.into_any(),
        ints(py, &response.list_c)?.into_any(),
        ints(py, &response.list_d)?.into_any(),
        ints(py, &response.streams)?.into_any(),
        ints(py, &response.supported_commands)?.into_any(),
        PyTuple::new(py, groups)?.into_any(),
        PyTuple::new(py, &response.strings)?.into_any(),
        ints(py, &response.words)?.into_any(),
        ints(py, &response.license_results)?.into_any(),
        response.mode.into_pyobject(py)?.into_any(),
        ints(py, &response.list_g)?.into_any(),
        PyTuple::new(py, &response.extended_strings)?.into_any(),
    ];
    PyTuple::new(py, items)
}

#[pyfunction]
#[pyo3(signature = (build_version, license_count, streams))]
fn minimal_connect_response<'py>(
    py: Python<'py>,
    build_version: &str,
    license_count: usize,
    streams: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyTuple>> {
    connect_response_tuple(py, &session::minimal_connect_response(build_version, license_count, &int_list(streams)?))
}

fn command_response_arg(response: &Bound<'_, PyAny>) -> PyResult<CommandResponse> {
    let tag = u32_arg(&response.getattr("tag")?)?;
    let status = i32_arg(&response.getattr("status")?)?;
    let command = response.getattr("command")?;
    let command: u32 = ranged(&command, "command is outside the head-pose session subset")?;
    let value = response.getattr("value")?;
    let body = match session::response_kind(command).py()? {
        ResponseKind::Connect => {
            let connect = response.getattr("connect")?;
            let connect = if connect.is_none() { ConnectResponse::default() } else { connect_response_arg(&connect)? };
            ResponseBody::Connect(Box::new(connect))
        }
        ResponseKind::Word => ResponseBody::Word(if value.is_none() { 0 } else { i32_arg(&value)? }),
        ResponseKind::Timesync => ResponseBody::Timesync(if value.is_none() {
            0
        } else {
            ranged(&value, "timesync value is outside unsigned 64-bit range")?
        }),
    };
    Ok(CommandResponse { tag, command, status, body })
}

#[pyfunction]
#[pyo3(signature = (response, extended=true))]
fn encode_command_response<'py>(
    py: Python<'py>,
    response: &Bound<'py, PyAny>,
    extended: bool,
) -> PyResult<Bound<'py, PyBytes>> {
    Ok(bytes(py, &session::encode_command_response(&command_response_arg(response)?, extended).py()?))
}

/// `(tag, command, status, value, connect response tuple)`.
#[pyfunction]
#[pyo3(signature = (packet, extended=true))]
fn decode_command_response<'py>(
    py: Python<'py>,
    packet: &Bound<'py, PyAny>,
    extended: bool,
) -> PyResult<Bound<'py, PyTuple>> {
    let response = session::decode_command_response(&bytes_like(packet)?, extended).py()?;
    let (value, connect) = match &response.body {
        ResponseBody::Connect(connect) => (py.None().into_bound(py), connect_response_tuple(py, connect)?.into_any()),
        ResponseBody::Word(value) => (value.into_pyobject(py)?.into_any(), py.None().into_bound(py)),
        ResponseBody::Timesync(value) => (value.into_pyobject(py)?.into_any(), py.None().into_bound(py)),
    };
    (response.tag, response.command, response.status, value, connect).into_pyobject(py)
}

// Properties ---------------------------------------------------------------------------

type PropertyTuple<'py> = (u32, i32, u32, Bound<'py, PyBytes>, Bound<'py, PyBytes>);

pub fn property_tuple<'py>(py: Python<'py>, response: &PropertyResponse) -> PropertyTuple<'py> {
    (response.tag, response.status, response.property_id, bytes(py, &response.block), bytes(py, &response.region))
}

pub fn property_arg(response: &Bound<'_, PyAny>) -> PyResult<PropertyResponse> {
    Ok(PropertyResponse {
        tag: u32_arg(&response.getattr("tag")?)?,
        status: i32_arg(&response.getattr("status")?)?,
        property_id: masked_u32(&response.getattr("property_id")?)?,
        block: bytes_like(&response.getattr("block")?)?,
        region: bytes_like(&response.getattr("region")?)?,
    })
}

type Value<'py> = (Bound<'py, PyBytes>, Bound<'py, PyBytes>);

pub fn value_tuple<'py>(py: Python<'py>, (block, region): (Vec<u8>, Vec<u8>)) -> Value<'py> {
    (bytes(py, &block), bytes(py, &region))
}

#[pyfunction]
fn encode_property_request<'py>(
    py: Python<'py>,
    tag: &Bound<'py, PyAny>,
    property_id: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyBytes>> {
    Ok(bytes(py, &session::encode_property_request(u32_arg(tag)?, masked_u32(property_id)?)))
}

#[pyfunction]
#[pyo3(signature = (tag, property_id, command=session::COMMAND_PROPERTY_SUBSCRIBE as i64))]
fn encode_property_subscribe_request<'py>(
    py: Python<'py>,
    tag: &Bound<'py, PyAny>,
    property_id: &Bound<'py, PyAny>,
    command: i64,
) -> PyResult<Bound<'py, PyBytes>> {
    let command = session::check_property_subscription_command(command).py()?;
    let packet = session::encode_property_subscribe_request(u32_arg(tag)?, masked_u32(property_id)?, command).py()?;
    Ok(bytes(py, &packet))
}

#[pyfunction]
fn decode_property_subscribe_request(packet: &Bound<'_, PyAny>) -> PyResult<(u32, u32, u32)> {
    session::decode_property_subscribe_request(&bytes_like(packet)?).py()
}

#[pyfunction]
fn decode_property_request(packet: &Bound<'_, PyAny>) -> PyResult<(u32, u32)> {
    session::decode_property_request(&bytes_like(packet)?).py()
}

#[pyfunction]
fn encode_property_response<'py>(py: Python<'py>, response: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyBytes>> {
    Ok(bytes(py, &session::encode_property_response(&property_arg(response)?)))
}

#[pyfunction]
fn encode_property_subscribe_response<'py>(
    py: Python<'py>,
    response: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyBytes>> {
    Ok(bytes(py, &session::encode_property_subscribe_response(&property_arg(response)?)))
}

#[pyfunction]
fn encode_property_notification<'py>(
    py: Python<'py>,
    tag: &Bound<'py, PyAny>,
    property_id: &Bound<'py, PyAny>,
    block: &Bound<'py, PyAny>,
    region: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyBytes>> {
    let packet = session::encode_property_notification(
        u32_arg(tag)?,
        masked_u32(property_id)?,
        &bytes_like(block)?,
        &bytes_like(region)?,
    );
    Ok(bytes(py, &packet))
}

#[pyfunction]
fn decode_property_response<'py>(py: Python<'py>, packet: &Bound<'py, PyAny>) -> PyResult<PropertyTuple<'py>> {
    Ok(property_tuple(py, &session::decode_property_response(&bytes_like(packet)?).py()?))
}

#[pyfunction]
fn decode_property_subscribe_response<'py>(
    py: Python<'py>,
    packet: &Bound<'py, PyAny>,
) -> PyResult<PropertyTuple<'py>> {
    Ok(property_tuple(py, &session::decode_property_subscribe_response(&bytes_like(packet)?).py()?))
}

/// Nine floats of three corners; any other count is refused as in the Python codec.
#[pyfunction]
fn display_area_value<'py>(py: Python<'py>, corners: Vec<f64>) -> PyResult<Value<'py>> {
    Ok(value_tuple(py, session::display_area_value(&corners).py()?))
}

#[pyfunction]
fn floats_value(py: Python<'_>, values: Vec<f64>) -> Value<'_> {
    value_tuple(py, session::floats_value(&values))
}

#[pyfunction]
fn string_value<'py>(py: Python<'py>, text: &str) -> PyResult<Value<'py>> {
    Ok(value_tuple(py, session::string_value(text).py()?))
}

#[pyfunction]
fn decode_display_area(response: &Bound<'_, PyAny>) -> PyResult<Vec<f64>> {
    Ok(session::decode_display_area(&property_arg(response)?).py()?.to_vec())
}

#[pyfunction]
fn decode_string_value(response: &Bound<'_, PyAny>) -> PyResult<String> {
    session::decode_string_value(&property_arg(response)?).py()
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    for function in [
        wrap_pyfunction!(server_pipe_names, m)?,
        wrap_pyfunction!(client_pipe_name, m)?,
        wrap_pyfunction!(encode_client_hello, m)?,
        wrap_pyfunction!(decode_client_hello, m)?,
        wrap_pyfunction!(encode_handshake_ack, m)?,
        wrap_pyfunction!(check_handshake_ack, m)?,
        wrap_pyfunction!(encode_client_pid, m)?,
        wrap_pyfunction!(decode_client_pid, m)?,
        wrap_pyfunction!(encode_handshake_info, m)?,
        wrap_pyfunction!(decode_handshake_info, m)?,
        wrap_pyfunction!(pipe_chunks, m)?,
        wrap_pyfunction!(split_packets, m)?,
        wrap_pyfunction!(encode_connect_request, m)?,
        wrap_pyfunction!(encode_stream_request, m)?,
        wrap_pyfunction!(encode_timesync_request, m)?,
        wrap_pyfunction!(decode_command_request, m)?,
        wrap_pyfunction!(minimal_connect_response, m)?,
        wrap_pyfunction!(encode_command_response, m)?,
        wrap_pyfunction!(decode_command_response, m)?,
        wrap_pyfunction!(encode_property_request, m)?,
        wrap_pyfunction!(encode_property_subscribe_request, m)?,
        wrap_pyfunction!(decode_property_subscribe_request, m)?,
        wrap_pyfunction!(decode_property_request, m)?,
        wrap_pyfunction!(encode_property_response, m)?,
        wrap_pyfunction!(encode_property_subscribe_response, m)?,
        wrap_pyfunction!(encode_property_notification, m)?,
        wrap_pyfunction!(decode_property_response, m)?,
        wrap_pyfunction!(decode_property_subscribe_response, m)?,
        wrap_pyfunction!(display_area_value, m)?,
        wrap_pyfunction!(floats_value, m)?,
        wrap_pyfunction!(string_value, m)?,
        wrap_pyfunction!(decode_display_area, m)?,
        wrap_pyfunction!(decode_string_value, m)?,
    ] {
        m.add_function(function)?;
    }
    m.add("STATUS_NAMES", PyTuple::new(m.py(), session::STATUS_NAMES)?)?;
    m.add("STRING_LIMITS", PyTuple::new(m.py(), session::STRING_LIMITS)?)?;
    Ok(())
}
