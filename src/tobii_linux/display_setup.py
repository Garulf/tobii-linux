"""Display setup of the Eye Tracker 5: where the monitor is in the tracker's coordinate frame.

Tobii's configuration app computes the display area (top-left, top-right and
bottom-left corners in millimetres, tracker frame) from the monitor's physical
size, the tracker's mounting geometry (command 2110) and one user adjustment,
the horizontal position of the tracker under the screen. It writes the result
with tobii_set_display_area, which the runtime turns into command 1440 and
stores in screenplane.setpm. docs/display-setup.md has the decompiled sources
and the check against the Windows reference host.

compose_display_area follows DisplayAreaUtils.ComposeDisplayArea: a flat
rectangle with its bottom edge centred on the origin is shifted by the
mounting's external offset plus the tracker offset, tilted about the x axis by
the mounting angle and shifted by the internal offset.
"""
from dataclasses import dataclass
import math
from pathlib import Path
import struct

from .gaze_device import SETTING_FILE_HEADER, setting_file, setting_file_payload
from .ttp import ProtocolError
from .ttp_fields import StructHeader, float32

Vector = tuple[float, float, float]

EDID_SIZE = 128
EDID_HEADER = bytes.fromhex('00ffffffffffff00')
EDID_SIZE_HINT_OFFSET = 66
EDID_RESOLUTION_OFFSET = 56
SIZE_HINT_ASPECT_TOLERANCE = 0.1
MM_PER_INCH = 25.4
SCREEN_PLANE_FLOATS = 9
DISPLAY_AREA_TRAILER_VALUE = 12345


@dataclass(frozen=True)
class MountingGeometry:
    """tobii_geometry_mounting_t, the 2110 reply in wire order.

The external offset places the display's bottom edge relative to the tracker
before the tilt, the internal offset moves the tilted plane into the tracker
frame. Stream Engine hands the values to the app as float32.
"""
    guides: int
    width_mm: float
    angle_deg: float
    external_offset_mm: Vector
    internal_offset_mm: Vector

    def as_float32(self):
        return MountingGeometry(self.guides, float32(self.width_mm), float32(self.angle_deg),
                                tuple(map(float32, self.external_offset_mm)),
                                tuple(map(float32, self.internal_offset_mm)))


ET5_MOUNTING = MountingGeometry(2, 184.0, 20.0, (0.0, -0.16, 13.85), (0.0, 5.38, 9.86))


@dataclass(frozen=True)
class DisplayArea:
    top_left: Vector
    top_right: Vector
    bottom_left: Vector

    @property
    def width_mm(self):
        return math.dist(self.top_left, self.top_right)

    @property
    def height_mm(self):
        return math.dist(self.top_left, self.bottom_left)

    @property
    def floats(self):
        return (*self.top_left, *self.top_right, *self.bottom_left)

    @classmethod
    def from_floats(cls, values):
        values = [float(v) for v in values]
        if len(values) != SCREEN_PLANE_FLOATS:
            raise ValueError('A display area needs nine numbers: top-left, top-right, bottom-left')
        return cls(tuple(values[0:3]), tuple(values[3:6]), tuple(values[6:9]))

    def as_float32(self):
        return DisplayArea.from_floats(map(float32, self.floats))

    def almost_equals(self, other, squared_mm=0.1):
        """DisplayArea.AlmostEquals: every corner within sqrt(0.1) mm."""
        return all(sum((a - b) ** 2 for a, b in zip(mine, theirs)) < squared_mm
                   for mine, theirs in zip(self.corners, other.corners))

    @property
    def corners(self):
        return self.top_left, self.top_right, self.bottom_left


@dataclass(frozen=True)
class DisplaySetup:
    """What the app recovers from a stored display area (DisplayAreaUtils.DecomposeDisplayArea)."""
    size_mm: tuple[float, float]
    tracker_offset_mm: float
    extra_angle_deg: float


def _add(a, b):
    return tuple(x + y for x, y in zip(a, b))


def _subtract(a, b):
    return tuple(x - y for x, y in zip(a, b))


def _rotate_about_x(point, radians):
    x, y, z = point
    cos, sin = math.cos(radians), math.sin(radians)
    return x, y * cos - z * sin, y * sin + z * cos


def _transform(area, function):
    return DisplayArea(*(function(corner) for corner in area.corners))


def flat_display_area(size_mm):
    """DisplayAreaUtils.FromPhysicalDisplaySize: bottom edge centred on the origin, z = 0."""
    width, height = size_mm
    half = 0.5 * width
    return DisplayArea((-half, height, 0.0), (half, height, 0.0), (-half, 0.0, 0.0))


def compose_display_area(size_mm, mounting=ET5_MOUNTING, tracker_offset_mm=0.0):
    """The display area Tobii's display setup writes for a monitor of size_mm.

tracker_offset_mm is the horizontal position of the tracker centre relative to
the display centre, positive to the right; the app's moving-lines step sets it
(EyetrackerMonitorMappingDevice.WriteDisplaySetupToTracker adds it to the
mounting's external offset). Corners are float32, as tobii_display_area_t
carries them.
"""
    mounting = mounting.as_float32()
    external = _add(mounting.external_offset_mm, (tracker_offset_mm, 0.0, 0.0))
    radians = mounting.angle_deg * (math.pi / 180.0)
    area = _transform(flat_display_area(size_mm), lambda p: _subtract(p, external))
    area = _transform(area, lambda p: _rotate_about_x(p, radians))
    area = _transform(area, lambda p: _add(p, mounting.internal_offset_mm))
    return area.as_float32()


def _angle_between(a, b):
    """Vector3.AngleBetween: acos of the clamped normalized dot product."""
    length = max(math.hypot(*a) * math.hypot(*b), 1e-6)
    return math.acos(max(-1.0, min(1.0, sum(x * y for x, y in zip(a, b)) / length)))


def decompose_display_area(area: DisplayArea, mounting=ET5_MOUNTING) -> DisplaySetup:
    """Monitor size, tracker offset and extra tilt that reproduce area, as the app reads them back."""
    mounting = mounting.as_float32()
    tilt = _angle_between(_subtract(area.top_left, area.bottom_left), (0.0, 1.0, 0.0))
    flat = _transform(area, lambda p: _subtract(p, mounting.internal_offset_mm))
    flat = _transform(flat, lambda p: _rotate_about_x(p, -tilt))
    flat = _transform(flat, lambda p: _add(p, mounting.external_offset_mm))
    width, height = area.width_mm, area.height_mm
    return DisplaySetup((width, height), -0.5 * width - flat.bottom_left[0],
                        math.degrees(tilt) - mounting.angle_deg)


def size_from_tracker_lines(left, right, tracker_width_mm, aspect_ratio):
    """MovingLinesScreenPlaneSetup: monitor size and tracker offset from the two line positions.

left and right are where the tracker's ends sit on screen, normalized from the
left edge; the app derives the monitor width from the tracker width they span.
"""
    if not 0 <= left < right <= 1:
        raise ValueError('Line positions must satisfy 0 <= left < right <= 1')
    width = tracker_width_mm / (right - left)
    return (width, width / aspect_ratio), ((left + right) * 0.5 - 0.5) * width


def tracker_lines(size_mm, tracker_offset_mm, tracker_width_mm):
    """Line positions the app starts from for a known size and offset (the inverse of the above)."""
    span = tracker_width_mm / size_mm[0]
    centre = tracker_offset_mm / size_mm[0] + 0.5
    return centre - span * 0.5, centre + span * 0.5


ET5_TOTAL_WIDTH_MM = 286.0
DEFAULT_MONITOR_WIDTH_MM = 376.3
DEFAULT_ASPECT = 16 / 9
MIN_LINE = 0.02
MAX_LINE = 0.98
MIN_LINE_DISTANCE = 0.05


def picture_proportion(mounting=ET5_MOUNTING):
    """EyeTrackerToLineWidthProportion: the tracker picture's width over the line distance.

GetTotalWidth gives 286 mm for an Eye Tracker 5 with two guides; with no or
one guide the picture is as wide as the lines.
"""
    return ET5_TOTAL_WIDTH_MM / mounting.width_mm if mounting.guides >= 2 else 1.0


def clamp_lines(left, right):
    """ScreenPlaneSetupViewModel setters, applied left then right as the app sets them."""
    left = max(min(left, 1.0 - MIN_LINE_DISTANCE), MIN_LINE)
    right = min(max(right, left + MIN_LINE_DISTANCE), MAX_LINE)
    return left, right


@dataclass(frozen=True)
class LineSetup:
    """Where the wizard puts the lines first, and the monitor aspect it keeps while they move."""
    left: float
    right: float
    size_mm: tuple[float, float]
    tracker_offset_mm: float
    aspect: float
    source: str


def is_mapped(device_area) -> bool:
    """Whether the device holds a display area, the Linux counterpart of Tobii's persisted monitor id."""
    return device_area is not None and device_area.width_mm > 0 and device_area.height_mm > 0


def line_instructions(mounting=ET5_MOUNTING) -> str:
    """ScreenPlaneSetup_FullScreenMessage_Instructions, or InstructionsG6Small for a one-guide tracker."""
    if mounting.guides == 1:
        return 'Move the lines to the sides of your eye tracker.'
    return 'Move the lines to the marks on top of your eye tracker.'


def initial_lines(mounting=ET5_MOUNTING, device_area=None, size_hint=None, screen_px=None) -> LineSetup:
    """MovingLinesScreenPlaneSetupFactory.CreateMovingLinesScreenPlaneSetup.

With a stored display area the size is the EDID hint when there is one, else
the decomposed size (DisplaySetupStorage), and the offset comes from the area.
Otherwise the width is the hint, if its aspect matches the screen's pixels, or
376.3 mm, the aspect is the screen's, and the tracker starts centred.
"""
    if is_mapped(device_area):
        stored = decompose_display_area(device_area, mounting)
        size = tuple(size_hint) if size_hint and all(size_hint) else stored.size_mm
        left, right = clamp_lines(*tracker_lines(size, stored.tracker_offset_mm, mounting.width_mm))
        return LineSetup(left, right, size, stored.tracker_offset_mm, size[0] / size[1],
                         'edid size and stored offset' if size_hint and all(size_hint) else 'stored display area')
    usable_hint = size_hint if size_hint and screen_px and size_hint_is_plausible(size_hint, screen_px) else None
    width = usable_hint[0] if usable_hint else DEFAULT_MONITOR_WIDTH_MM
    aspect = screen_px[0] / screen_px[1] if screen_px and all(screen_px) else DEFAULT_ASPECT
    left, right = clamp_lines(*tracker_lines((width, width / aspect), 0.0, mounting.width_mm))
    return LineSetup(left, right, (width, width / aspect), 0.0, aspect,
                     'edid size estimate' if usable_hint else 'default width estimate')


def area_from_lines(left, right, aspect, mounting=ET5_MOUNTING):
    """What the wizard writes when the user confirms lines at left and right: size, offset and area."""
    size, offset = size_from_tracker_lines(left, right, mounting.width_mm, aspect)
    return size, offset, compose_display_area(size, mounting, offset)


def size_from_diagonal(diagonal_inches, aspect=(16, 9)):
    """Panel size in millimetres from its diagonal and aspect ratio."""
    width, height = aspect
    diagonal_mm = diagonal_inches * MM_PER_INCH
    scale = diagonal_mm / math.hypot(width, height)
    return width * scale, height * scale


def edid_size_mm(edid: bytes):
    """EdidDataInfoReader.TryGetMonitorSizeHintFromEdidData: the first detailed timing's image size."""
    if len(edid) < EDID_SIZE or not edid.startswith(EDID_HEADER):
        raise ValueError('Not an EDID base block')
    low_width, low_height, high = edid[EDID_SIZE_HINT_OFFSET:EDID_SIZE_HINT_OFFSET + 3]
    return float((high & 0xF0) << 4 | low_width), float((high & 0x0F) << 8 | low_height)


def edid_native_resolution(edid: bytes):
    """EdidDataInfoReader.TryGetNativeResolutionFromEdidData: the first detailed timing's active pixels."""
    if len(edid) < EDID_SIZE or not edid.startswith(EDID_HEADER):
        raise ValueError('Not an EDID base block')
    d = edid[EDID_RESOLUTION_OFFSET:EDID_RESOLUTION_OFFSET + 6]
    return (d[2] & 0xF0) << 4 | d[0], (d[5] & 0xF0) << 4 | d[3]


def size_hint_is_plausible(size_mm, resolution_px):
    """TryGetScreenSizeHint drops a hint whose aspect ratio differs from the screen's by 0.1 or more."""
    if not all(size_mm) or not all(resolution_px):
        return False
    return abs(size_mm[0] / size_mm[1] - resolution_px[0] / resolution_px[1]) < SIZE_HINT_ASPECT_TOLERANCE


@dataclass(frozen=True)
class Monitor:
    connector: str
    status: str
    size_mm: tuple[float, float]
    resolution_px: tuple[int, int]


def drm_monitors(root='/sys/class/drm'):
    """Monitors with an EDID under the DRM sysfs tree, connected ones first."""
    monitors = []
    for edid_path in sorted(Path(root).glob('*/edid')):
        edid = edid_path.read_bytes()
        try:
            size, resolution = edid_size_mm(edid), edid_native_resolution(edid)
        except ValueError:
            continue
        status_path = edid_path.parent/'status'
        status = status_path.read_text().strip() if status_path.exists() else 'unknown'
        monitors.append(Monitor(edid_path.parent.name, status, size, resolution))
    return sorted(monitors, key=lambda m: m.status != 'connected')


def display_area_from_reply(values) -> DisplayArea:
    """The 1430 reply: three 8001 triples (top-left, top-right, bottom-left) and a trailer array."""
    numbers = [v for v in values if isinstance(v, float)]
    if len(numbers) != SCREEN_PLANE_FLOATS:
        raise ProtocolError('Display area reply must hold three corners')
    return DisplayArea.from_floats(numbers)


def mounting_from_reply(values) -> MountingGeometry:
    """The 2110 reply: guides, width, angle, then the external and internal offset triples."""
    numbers = [v for v in values if not isinstance(v, StructHeader)]
    if len(numbers) != 9 or not isinstance(numbers[0], int):
        raise ProtocolError('Unexpected geometry mounting reply')
    return MountingGeometry(numbers[0], *numbers[1:3], tuple(numbers[3:6]), tuple(numbers[6:9]))


def screen_plane_setting(area: DisplayArea) -> bytes:
    """screenplane.setpm bytes: setting header, then bottom-left, top-left, top-right as float32."""
    corners = (area.bottom_left, area.top_left, area.top_right)
    return setting_file(struct.pack('<9f', *(c for corner in corners for c in corner)))


def parse_screen_plane_setting(data: bytes) -> DisplayArea:
    payload = setting_file_payload(data)
    if len(payload) != 4 * SCREEN_PLANE_FLOATS:
        raise ProtocolError('Screen plane setting must hold three points')
    values = struct.unpack('<9f', payload)
    bottom_left, top_left, top_right = values[0:3], values[3:6], values[6:9]
    return DisplayArea(top_left, top_right, bottom_left)


def save_display_area(path, area: DisplayArea, overwrite=False):
    path = Path(path)
    if path.exists() and not overwrite:
        raise FileExistsError(f'{path} exists')
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(screen_plane_setting(area))
    return path


def load_display_area(path) -> DisplayArea:
    """A screenplane.setpm file (from this tool or Windows), or text holding the nine numbers."""
    data = Path(path).read_bytes()
    if len(data) >= SETTING_FILE_HEADER.size and data[:4] == struct.pack('<I', 4):
        return parse_screen_plane_setting(data)
    return DisplayArea.from_floats(data.decode().replace(',', ' ').split())


def read_display_area(session) -> DisplayArea:
    from .ttp_commands import Command, header_only_request
    return display_area_from_reply(session.reply_values(
        session.call(lambda rid: header_only_request(rid, Command.GET_DISPLAY_AREA))))


def read_mounting(session) -> MountingGeometry:
    from .ttp_commands import Command, header_only_request
    return mounting_from_reply(session.reply_values(
        session.call(lambda rid: header_only_request(rid, Command.GET_GEOMETRY_MOUNTING))))


def apply_display_area(session, area: DisplayArea):
    """1440 set_display_area with the corners, then 1430 to read back what the device holds."""
    from .gaze_device import set_display_area
    session.call(lambda rid: set_display_area(rid, *area.corners))
    return read_display_area(session)
