"""Compact JSON payloads and grayscale PNG images for the demo's browser stream."""
import base64
import json
import math
import struct
import zlib

from .. import gaze

PNG_SIGNATURE = b'\x89PNG\r\n\x1a\n'
PNG_GRAYSCALE_8BIT = (8, 0, 0, 0, 0)
TRACKER_STATES = {0: 'searching', 1: 'recovering', 2: 'tracking'}


def _png_chunk(kind, data):
    return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))


def png_grayscale(width, height, pixels, level=1):
    """An 8-bit grayscale PNG; every row uses filter type 0."""
    if len(pixels) != width * height:
        raise ValueError('Pixel count does not match the image size')
    rows = b''.join(b'\0' + bytes(pixels[y * width:(y + 1) * width]) for y in range(height))
    header = struct.pack('>II5B', width, height, *PNG_GRAYSCALE_8BIT)
    return (PNG_SIGNATURE + _png_chunk(b'IHDR', header)
            + _png_chunk(b'IDAT', zlib.compress(rows, level)) + _png_chunk(b'IEND', b''))


def compact_json(data):
    return json.dumps(data, separators=(',', ':'), allow_nan=False)


def rounded(values, digits):
    if values is None:
        return None
    result = [round(float(v), digits) for v in values]
    return result if all(math.isfinite(v) for v in result) else None


def rounded_scalar(number, digits):
    if number is None or not math.isfinite(number):
        return None
    return round(float(number), digits)


def int16_base64(values, scale):
    """Little-endian int16 values * scale, base64 encoded; None when anything is not finite or out of range."""
    import numpy as np
    if values is None:
        return None
    scaled = np.round(np.asarray(values, dtype=np.float64).reshape(-1) * scale)
    if not np.isfinite(scaled).all() or np.abs(scaled).max(initial=0) > 32767:
        return None
    return base64.b64encode(scaled.astype('<i2').tobytes()).decode('ascii')


MESH_PIXEL_SCALE = 10
MESH_MILLIMETRE_SCALE = 10


def mesh_topology_event(edges, vertex_count, display_corners, tracker_position):
    """Sent once: wireframe edges and the scene the 3D view draws around the head (millimetres)."""
    return {'vertices': vertex_count, 'edges': [index for edge in edges for index in edge],
            'pixel_scale': MESH_PIXEL_SCALE, 'millimetre_scale': MESH_MILLIMETRE_SCALE,
            'display': [rounded(corner, 1) for corner in display_corners],
            'tracker': rounded(tracker_position, 1)}


def frame_event(sequence, frame, tracked, tracker_state, mesh_pixels=None):
    """Camera image with the overlay of the same frame: face box, landmarks, mesh and tracking state."""
    landmarks = None
    if tracked is not None and tracked.frame_points is not None:
        landmarks = rounded((v for point in tracked.frame_points for v in point), 4)
    face = None
    if tracked is not None and tracked.face is not None and tracked.face[2] > 0:
        face = [int(v) for v in tracked.face]
    image = png_grayscale(frame.width, frame.height, frame.pixels)
    return {'seq': sequence, 't': frame.timestamp, 'width': frame.width, 'height': frame.height,
            'png': base64.b64encode(image).decode('ascii'),
            'state': TRACKER_STATES.get(tracker_state) if tracked is not None else None,
            'face': face, 'landmarks': landmarks,
            'quality': rounded_scalar(tracked.quality_score, 3) if tracked is not None else None,
            'accepted': bool(tracked.accepted) if tracked is not None else None,
            'mesh': int16_base64(mesh_pixels, MESH_PIXEL_SCALE)}


def pose_event(sample, tracked, tracker_state, processing_ms, mesh_millimetres=None):
    """Head-pose sample in millimetres and degrees, tracker diagnostics and the posed mesh."""
    valid = bool(sample.valid)
    return {'t': sample.timestamp_us, 'valid': valid,
            'position': rounded(sample.position_mm, 1) if valid else None,
            'rotation': rounded((math.degrees(v) for v in sample.rotation_rad), 2) if valid else None,
            'state': TRACKER_STATES.get(tracker_state),
            'quality': rounded_scalar(tracked.quality_score, 3),
            'accepted': bool(tracked.accepted),
            'ms': round(processing_ms, 1),
            'mesh3d': int16_base64(mesh_millimetres, MESH_MILLIMETRE_SCALE) if valid else None}


def eye_event(eye, openness_valid, openness):
    """One eye of a gaze.GazeDataRecord, with values only where the runtime marks them valid."""
    valid = bool(eye.eye_valid)
    pupil = eye.pupil_diameter if eye.pupil_valid and eye.pupil_diameter > 0 else None
    return {'valid': valid, 'point': rounded(eye.gaze_point_2d, 4) if valid else None,
            'pupil': rounded_scalar(pupil, 2),
            'openness': rounded_scalar(openness, 3) if openness_valid else None,
            'origin': rounded(eye.origin, 1) if eye.origin_valid else None}


def gaze_event(frame, point, source_name):
    """One gaze.GazeFrame and the point of the selected gaze source, normalised to the display."""
    combined, openness = frame.gaze_point, frame.eye_openness
    return {'t': frame.timestamp_us, 'source': source_name,
            'point': rounded((point.x, point.y), 4) if point is not None else None,
            'combined_valid': bool(combined.valid),
            'combined': rounded(combined.point, 4) if combined.valid else None,
            'left': eye_event(frame.gaze_data.left, openness.left_valid, openness.left),
            'right': eye_event(frame.gaze_data.right, openness.right_valid, openness.right)}


def _column_display(value):
    if isinstance(value, tuple):
        return rounded(value, 4)
    if isinstance(value, float):
        return rounded_scalar(value, 4)
    return value


def gaze_columns_event(frame):
    """Every named column of the device record (gaze.COLUMNS), by column id."""
    device = frame.device
    return {'t': frame.timestamp_us,
            'columns': [[column_id, name, _column_display(getattr(device, name))]
                        for column_id, (name, _, _) in sorted(gaze.COLUMNS.items()) if name is not None]}


def sse_block(topic, payload):
    """One Server-Sent Events message; `payload` must not contain newlines."""
    return f'event: {topic}\ndata: {payload}\n\n'.encode()
