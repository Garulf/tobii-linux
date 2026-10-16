//! tobii_linux.model_digests: SHA-256 of every file of the models/tobii-et5 layout.

use pyo3::prelude::*;
use tobii_models::digests::EXPECTED_SHA256;

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("EXPECTED_SHA256", EXPECTED_SHA256.to_vec())
}
