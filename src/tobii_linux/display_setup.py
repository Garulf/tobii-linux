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

The model, parsers and device procedures are crates/tobii-gaze/src/display.rs;
the dataclasses here carry their results.
"""
from dataclasses import dataclass
from pathlib import Path

from ._native import display_setup as _ds
from .gaze_device import drive
from .ttp_fields import float32

Vector = tuple[float, float, float]

EDID_SIZE = 128
EDID_HEADER = _ds.EDID_HEADER
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


ET5_MOUNTING = MountingGeometry(*_ds.ET5_MOUNTING)


@dataclass(frozen=True)
class DisplayArea:
    top_left: Vector
    top_right: Vector
    bottom_left: Vector

    @property
    def width_mm(self):
        return _ds.width_mm(self)

    @property
    def height_mm(self):
        return _ds.height_mm(self)

    @property
    def floats(self):
        return (*self.top_left, *self.top_right, *self.bottom_left)

    @classmethod
    def from_floats(cls, values):
        return cls(*_ds.area_from_floats([float(v) for v in values]))

    def as_float32(self):
        return DisplayArea.from_floats(map(float32, self.floats))

    def almost_equals(self, other, squared_mm=0.1):
        """DisplayArea.AlmostEquals: every corner within sqrt(0.1) mm."""
        return _ds.almost_equals(self, other, squared_mm)

    @property
    def corners(self):
        return self.top_left, self.top_right, self.bottom_left


@dataclass(frozen=True)
class DisplaySetup:
    """What the app recovers from a stored display area (DisplayAreaUtils.DecomposeDisplayArea)."""
    size_mm: tuple[float, float]
    tracker_offset_mm: float
    extra_angle_deg: float


def flat_display_area(size_mm):
    """DisplayAreaUtils.FromPhysicalDisplaySize: bottom edge centred on the origin, z = 0."""
    return DisplayArea(*_ds.flat_display_area(size_mm))


def compose_display_area(size_mm, mounting=ET5_MOUNTING, tracker_offset_mm=0.0):
    """The display area Tobii's display setup writes for a monitor of size_mm.

tracker_offset_mm is the horizontal position of the tracker centre relative to
the display centre, positive to the right; the app's moving-lines step sets it
(EyetrackerMonitorMappingDevice.WriteDisplaySetupToTracker adds it to the
mounting's external offset). Corners are float32, as tobii_display_area_t
carries them.
"""
    return DisplayArea(*_ds.compose_display_area(size_mm, mounting, tracker_offset_mm))


def decompose_display_area(area: DisplayArea, mounting=ET5_MOUNTING) -> DisplaySetup:
    """Monitor size, tracker offset and extra tilt that reproduce area, as the app reads them back."""
    return DisplaySetup(*_ds.decompose_display_area(area, mounting))


def size_from_tracker_lines(left, right, tracker_width_mm, aspect_ratio):
    """MovingLinesScreenPlaneSetup: monitor size and tracker offset from the two line positions.

left and right are where the tracker's ends sit on screen, normalized from the
left edge; the app derives the monitor width from the tracker width they span.
"""
    return _ds.size_from_tracker_lines(left, right, tracker_width_mm, aspect_ratio)


def tracker_lines(size_mm, tracker_offset_mm, tracker_width_mm):
    """Line positions the app starts from for a known size and offset (the inverse of the above)."""
    return _ds.tracker_lines(size_mm, tracker_offset_mm, tracker_width_mm)


ET5_TOTAL_WIDTH_MM = 286.0
DEFAULT_MONITOR_WIDTH_MM = 376.3
DEFAULT_ASPECT = _ds.DEFAULT_ASPECT
MIN_LINE = 0.02
MAX_LINE = 0.98
MIN_LINE_DISTANCE = 0.05


def picture_proportion(mounting=ET5_MOUNTING):
    """EyeTrackerToLineWidthProportion: the tracker picture's width over the line distance.

GetTotalWidth gives 286 mm for an Eye Tracker 5 with two guides; with no or
one guide the picture is as wide as the lines.
"""
    return _ds.picture_proportion(mounting)


def clamp_lines(left, right):
    """ScreenPlaneSetupViewModel setters, applied left then right as the app sets them."""
    return _ds.clamp_lines(left, right)


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
    return _ds.is_mapped(device_area)


def line_instructions(mounting=ET5_MOUNTING) -> str:
    """ScreenPlaneSetup_FullScreenMessage_Instructions, or InstructionsG6Small for a one-guide tracker."""
    return _ds.line_instructions(mounting)


def initial_lines(mounting=ET5_MOUNTING, device_area=None, size_hint=None, screen_px=None) -> LineSetup:
    """MovingLinesScreenPlaneSetupFactory.CreateMovingLinesScreenPlaneSetup.

With a stored display area the size is the EDID hint when there is one, else
the decomposed size (DisplaySetupStorage), and the offset comes from the area.
Otherwise the width is the hint, if its aspect matches the screen's pixels, or
376.3 mm, the aspect is the screen's, and the tracker starts centred.
"""
    return LineSetup(*_ds.initial_lines(mounting, device_area, tuple(size_hint) if size_hint else None,
                                        tuple(screen_px) if screen_px else None))


def area_from_lines(left, right, aspect, mounting=ET5_MOUNTING):
    """What the wizard writes when the user confirms lines at left and right: size, offset and area."""
    size, offset, area = _ds.area_from_lines(left, right, aspect, mounting)
    return size, offset, DisplayArea(*area)


def size_from_diagonal(diagonal_inches, aspect=(16, 9)):
    """Panel size in millimetres from its diagonal and aspect ratio."""
    return _ds.size_from_diagonal(diagonal_inches, aspect)


def edid_size_mm(edid: bytes):
    """EdidDataInfoReader.TryGetMonitorSizeHintFromEdidData: the first detailed timing's image size."""
    return _ds.edid_size_mm(edid)


def edid_native_resolution(edid: bytes):
    """EdidDataInfoReader.TryGetNativeResolutionFromEdidData: the first detailed timing's active pixels."""
    return _ds.edid_native_resolution(edid)


def size_hint_is_plausible(size_mm, resolution_px):
    """TryGetScreenSizeHint drops a hint whose aspect ratio differs from the screen's by 0.1 or more."""
    return _ds.size_hint_is_plausible(size_mm, resolution_px)


@dataclass(frozen=True)
class Monitor:
    connector: str
    status: str
    size_mm: tuple[float, float]
    resolution_px: tuple[int, int]


def drm_monitors(root='/sys/class/drm'):
    """Monitors with an EDID under the DRM sysfs tree, connected ones first."""
    return [Monitor(*monitor) for monitor in _ds.drm_monitors(root)]


def display_area_from_reply(values) -> DisplayArea:
    """The 1430 reply: three 8001 triples (top-left, top-right, bottom-left) and a trailer array."""
    return DisplayArea(*_ds.display_area_from_reply(values))


def mounting_from_reply(values) -> MountingGeometry:
    """The 2110 reply: guides, width, angle, then the external and internal offset triples."""
    return MountingGeometry(*_ds.mounting_from_reply(values))


def screen_plane_setting(area: DisplayArea) -> bytes:
    """screenplane.setpm bytes: setting header, then bottom-left, top-left, top-right as float32."""
    return _ds.screen_plane_setting(area)


def parse_screen_plane_setting(data: bytes) -> DisplayArea:
    return DisplayArea(*_ds.parse_screen_plane_setting(data))


def save_display_area(path, area: DisplayArea, overwrite=False):
    return Path(_ds.save_display_area(path, area, overwrite))


def load_display_area(path) -> DisplayArea:
    """A screenplane.setpm file (from this tool or Windows), or text holding the nine numbers."""
    return DisplayArea(*_ds.load_display_area(path))


def read_display_area(session) -> DisplayArea:
    return DisplayArea(*drive(_ds.read_display_area(), session))


def read_mounting(session) -> MountingGeometry:
    return MountingGeometry(*drive(_ds.read_mounting(), session))


def apply_display_area(session, area: DisplayArea):
    """1440 set_display_area with the corners, then 1430 to read back what the device holds."""
    return DisplayArea(*drive(_ds.apply_display_area(area), session))
