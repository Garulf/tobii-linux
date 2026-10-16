"""SHA-256 of every file in the models/tobii-et5 layout.

The table is crates/tobii-models/src/digests.rs.
"""
from ._native import model_digests as _digests

EXPECTED_SHA256 = dict(_digests.EXPECTED_SHA256)
