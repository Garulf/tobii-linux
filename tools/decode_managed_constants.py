#!/usr/bin/env python3
"""Supplement REA member evidence with this build's ECMA-335 enum constants.

Execution-free. Uses REA's artifact identity, metadata table counts, and first
CustomAttribute row to anchor the immediately preceding Constant table. Refuses
other binaries and wide heaps; this is intentionally not a general CLI parser.
"""
import hashlib
import json
from pathlib import Path
import struct


def main():
    root = Path(__file__).resolve().parents[1]
    evidence = root / "evidence/private"
    data = (evidence / "binaries/Tobii.StreamEngine.Net.dll").read_bytes()
    digest = hashlib.sha256(data).hexdigest()
    expected = "51ec6fd76bd126fd97afe86c920b83750f9700b40d3c0b2e07459b5dff69042a"
    if digest != expected:
        raise ValueError("Decoder anchors apply only to the analyzed assembly build")
    identity = json.loads((evidence / "streamengine-managed.json").read_text())
    members = json.loads((evidence / "streamengine-managed-members.json").read_text())
    if identity["subject"]["digest"]["sha256"] != digest or members["subject"]["digest"]["sha256"] != digest:
        raise ValueError("REA evidence identity mismatch")
    metadata = data.index(b"BSJB")
    length = struct.unpack_from("<I", data, metadata + 12)[0]
    position = (metadata + 16 + length + 3) & ~3
    _, count = struct.unpack_from("<HH", data, position)
    position += 4
    streams = {}
    for _ in range(count):
        offset, size = struct.unpack_from("<II", data, position)
        position += 8
        end = data.index(b"\0", position)
        name = data[position:end].decode("ascii")
        position = (end + 4) & ~3
        streams[name] = (metadata + offset, size)
    if data[streams["#~"][0] + 6] & 4:
        raise ValueError("Wide Blob indexes unsupported")
    rows = identity["normalized_result"]["metadata"]["table_row_counts"]
    if rows["Field"] >= 16384 or rows["Param"] >= 16384 or rows["Property"] >= 16384:
        raise ValueError("Wide HasConstant indexes unsupported")
    attributes = identity["normalized_result"]["attributes"]
    first_attribute = next(a for a in attributes if a["token"] == "0x0c000001")
    start = first_attribute["row_offset"] - rows["Constant"] * 6
    fields = {int(f["token"], 16) & 0xffffff: f for f in members["normalized_result"]["fields"]}
    selected = {"tobii_error_t", "tobii_validity_t", "tobii_field_of_use_t", "tobii_stream_t"}
    constants = []
    for row in range(rows["Constant"]):
        offset = start + row * 6
        kind, padding, parent, index = struct.unpack_from("<BBHH", data, offset)
        if parent & 3:
            continue  # Only HasConstant Field rows are relevant.
        field = fields[parent >> 2]
        if field["declaring_type"].split(".")[-1] not in selected:
            continue
        blob = streams["#Blob"][0] + index
        if kind != 8 or padding != 0 or data[blob] != 4:
            raise ValueError("Expected a four-byte signed int32 enum constant")
        constants.append({"name": field["name"], "type": field["declaring_type"],
                          "value": struct.unpack_from("<i", data, blob + 1)[0],
                          "field_token": field["token"], "constant_row_offset": offset,
                          "blob_value_offset": blob + 1})
    result = {"source_sha256": digest,
              "method": "Direct ECMA-335 Constant table and Blob heap decoding; table position anchored to REA CustomAttribute first row and Constant row count. Not a REA-produced evidence ID.",
              "constants": constants, "streams": streams, "table_start": start}
    (evidence / "streamengine-constants.json").write_text(json.dumps(result, indent=2) + "\n")
    print(f"Recovered {len(constants)} constants for assembly {digest}")


if __name__ == "__main__":
    main()
