"""Load the researched IR4/IR6 graphs into native OpenVINO operations.

The translation is crates/tobii-inference/src/legacy_ir.rs: each observed layer type
becomes the opset1 operations, attributes and constants the Python opset loader built,
written as an OpenVINO IR v11 document that OpenVINO reads from memory. Unknown
operations, incompatible tensor shapes, hash mismatches and blob extents outside the
weights fail rather than being guessed. This performs graph inference only; camera
normalization, crop selection and pose fitting are not implemented here.

Optional dependency: openvino, for the returned openvino.Model.
"""
from ._native import legacy_ir as _legacy_ir


def load_model(directory):
    import openvino as ov
    _, xml, weights = _legacy_ir.translate(directory)
    return ov.Core().read_model(xml.encode(), weights)
