"""The monitors of the desktop the setup page runs on, and Tobii's rules for moving between them.

Tobii's display setup (VirtualScreen, MonitorMappingModel; docs/setup-flow.md)
starts on the only monitor, else on the one the tracker was mapped to, else
on the primary one (the monitor at 0, 0). Its "previous" monitor is the one
whose left edge is the nearest to the left of the current one's, "next" the
nearest to the right; only the left edge counts.

Sources, all read only:

- Linux under X11 (or XWayland): `xrandr --prop`, which gives each connected
  output's geometry, rotation, primary flag and EDID.
- Linux without xrandr: the EDIDs under /sys/class/drm. There is no
  geometry then, so the page cannot be moved between monitors.
- WSL: the Windows desktop through powershell.exe: each screen's bounds in
  physical pixels, the monitor's device instance id (what Tobii stores as the
  monitor id and the game's window tracking matches) and its EDID from the
  registry.

The choice rules and the parsers are crates/tobii-gaze/src/monitors.rs; the
queries themselves (subprocess, PowerShell) stay here.
"""
from dataclasses import dataclass, field
import base64
import gzip
import shutil
import subprocess

from ._native import monitors as _mon

EDID_ID = slice(8, 18)
ROTATIONS = {'normal': 0, 'left': 90, 'inverted': 180, 'right': 270}
WINDOWS_ROTATIONS = {0: 0, 1: 90, 2: 180, 3: 270}
QUERY_TIMEOUT_S = 20.0


@dataclass(frozen=True)
class Monitor:
    """One monitor: `key` names it stably (Windows instance id, else the output name)."""
    key: str
    name: str
    bounds: tuple | None = None
    primary: bool = False
    rotation: int = 0
    edid: bytes | None = field(default=None, repr=False)
    instance_id: str | None = None

    @property
    def left(self):
        return self.bounds[0] if self.bounds else None

    @property
    def resolution_px(self):
        return tuple(self.bounds[2:]) if self.bounds else None

    @property
    def size_mm(self):
        """The EDID size hint in the orientation the monitor is used in, or None."""
        return _mon.size_mm(self)

    @property
    def native_resolution(self):
        return _mon.native_resolution(self)

    @property
    def edid_id(self):
        """EDID bytes 8 to 17 (manufacturer, product, serial, week, year) in hex, as Tobii's display id keeps them."""
        return _mon.edid_id(self)

    @property
    def display_id(self):
        """Tobii's display id string (displayinfo.setpm) when the Windows instance id is known."""
        return _mon.display_id(self)

    def record(self):
        return {'key': self.key, 'name': self.name, 'bounds': list(self.bounds) if self.bounds else None,
                'primary': self.primary, 'rotation': self.rotation, 'instance_id': self.instance_id,
                'edid_id': self.edid_id, 'size_mm': list(self.size_mm) if self.size_mm else None,
                'display_id': self.display_id}


def same_aspect_ignoring_orientation(a, b, tolerance=0.001):
    """AspectRatio.IsSameIgnoreOrientation."""
    return _mon.same_aspect_ignoring_orientation(tuple(a) if a else None, tuple(b) if b else None, tolerance)


def has_recommended_aspect(monitor, resolution_px=None):
    """VirtualScreen.MonitorHasRecommendedAspectRatio: the mode's aspect equals the panel's native one."""
    return _mon.has_recommended_aspect(monitor, tuple(resolution_px) if resolution_px else None)


def _pick(monitors, index):
    return None if index is None else monitors[index]


def primary(monitors):
    return _pick(monitors, _mon.primary(monitors))


def initial_monitor(monitors, persisted_key=None):
    """InitSelectedMonitor: the only monitor, else the persisted one, else the primary one."""
    return _pick(monitors, _mon.initial_monitor(monitors, persisted_key))


def previous_monitor(monitors, current):
    """GetPreviousDisplayDevice: the largest left edge below the current one's."""
    return _pick(monitors, _mon.previous_monitor(monitors, current))


def next_monitor(monitors, current):
    """GetNextDisplayDevice: the smallest left edge above the current one's."""
    return _pick(monitors, _mon.next_monitor(monitors, current))


# xrandr ------------------------------------------------------------------------------------------

def parse_xrandr(text):
    """Connected outputs of `xrandr --prop`, with their EDID when listed."""
    return [Monitor(*monitor) for monitor in _mon.parse_xrandr(text)]


def drm_fallback(root='/sys/class/drm'):
    """Connected DRM connectors with their EDID but no geometry (Wayland without xrandr)."""
    return [Monitor(*monitor) for monitor in _mon.drm_fallback(root)]


def linux_monitors(run=subprocess.run):
    if shutil.which('xrandr'):
        result = run(['xrandr', '--prop'], capture_output=True, text=True, timeout=QUERY_TIMEOUT_S)
        if result.returncode == 0:
            monitors = parse_xrandr(result.stdout)
            if monitors:
                return monitors
    return drm_fallback()


# Windows (from WSL) ------------------------------------------------------------------------------

WINDOWS_QUERY = r'''
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class TobiiLinuxDisplays {
  [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
  public struct DisplayDevice {
    public int cb;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string DeviceName;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)] public string DeviceString;
    public int StateFlags;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)] public string DeviceID;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)] public string DeviceKey;
  }
  [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
  public struct DevMode {
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmDeviceName;
    public short dmSpecVersion, dmDriverVersion, dmSize, dmDriverExtra;
    public int dmFields, dmPositionX, dmPositionY, dmDisplayOrientation, dmDisplayFixedOutput;
    public short dmColor, dmDuplex, dmYResolution, dmTTOption, dmCollate;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmFormName;
    public short dmLogPixels;
    public int dmBitsPerPel, dmPelsWidth, dmPelsHeight, dmDisplayFlags, dmDisplayFrequency;
    public int dmICMMethod, dmICMIntent, dmMediaType, dmDitherType, dmReserved1, dmReserved2;
    public int dmPanningWidth, dmPanningHeight;
  }
  [DllImport("user32.dll", CharSet = CharSet.Unicode)]
  public static extern bool EnumDisplayDevices(string device, int index, ref DisplayDevice info, int flags);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)]
  public static extern bool EnumDisplaySettings(string device, int mode, ref DevMode info);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  public static string MonitorInterface(string adapter) {
    var info = new DisplayDevice(); info.cb = Marshal.SizeOf(info);
    return EnumDisplayDevices(adapter, 0, ref info, 1) ? info.DeviceID : null;
  }
  public static int Orientation(string adapter) {
    var mode = new DevMode(); mode.dmSize = (short)Marshal.SizeOf(mode);
    return EnumDisplaySettings(adapter, -1, ref mode) ? mode.dmDisplayOrientation : 0;
  }
}
"@
[TobiiLinuxDisplays]::SetProcessDPIAware() | Out-Null
$screens = foreach ($screen in [System.Windows.Forms.Screen]::AllScreens) {
  $interface = [TobiiLinuxDisplays]::MonitorInterface($screen.DeviceName)
  $edid = $null
  if ($interface) {
    $instance = ($interface -replace '^\\\\\?\\', '' -replace '#\{[^}]*\}$', '') -replace '#', '\'
    $key = "HKLM:\SYSTEM\CurrentControlSet\Enum\$instance\Device Parameters"
    $value = Get-ItemProperty -Path $key -Name EDID -ErrorAction SilentlyContinue
    if ($value) { $edid = [Convert]::ToBase64String($value.EDID) }
  }
  [pscustomobject]@{ name = $screen.DeviceName; primary = $screen.Primary;
    bounds = @($screen.Bounds.X, $screen.Bounds.Y, $screen.Bounds.Width, $screen.Bounds.Height);
    orientation = [TobiiLinuxDisplays]::Orientation($screen.DeviceName); interface = $interface; edid = $edid }
}
ConvertTo-Json -Compress -Depth 3 @($screens)
'''


def instance_id_from_interface(interface):
    """\\\\?\\DISPLAY#GSM5B7F#5&5028a6b&0&UID41221#{guid} -> DISPLAY\\GSM5B7F\\5&5028A6B&0&UID41221."""
    return _mon.instance_id_from_interface(interface)


def parse_windows_monitors(text):
    """Monitors from the JSON WINDOWS_QUERY prints."""
    return [Monitor(*monitor) for monitor in _mon.parse_windows_monitors(text)]


def encoded_command(script):
    """-EncodedCommand text running `script` gzip-compressed, short enough for an 8191-character command line."""
    packed = base64.b64encode(gzip.compress(script.encode())).decode()
    bootstrap = ("$s = New-Object IO.MemoryStream(,[Convert]::FromBase64String('" + packed + "')); "
                 "$z = New-Object IO.Compression.GzipStream($s, [IO.Compression.CompressionMode]::Decompress); "
                 "Invoke-Expression (New-Object IO.StreamReader($z)).ReadToEnd()")
    return base64.b64encode(bootstrap.encode('utf-16-le')).decode()


def windows_monitors(run=subprocess.run):
    result = run(['powershell.exe', '-NoProfile', '-NonInteractive', '-EncodedCommand', encoded_command(WINDOWS_QUERY)],
                 capture_output=True, text=True, timeout=QUERY_TIMEOUT_S)
    if result.returncode != 0:
        raise OSError(f'Windows monitor query failed: {result.stderr.strip()[:300]}')
    return parse_windows_monitors(result.stdout.strip())


def desktop_monitors(wsl=None):
    """The monitors of the desktop the browser opens on; an empty list when they cannot be read."""
    from .browser import running_under_wsl
    try:
        if running_under_wsl() if wsl is None else wsl:
            return windows_monitors() if shutil.which('powershell.exe') else []
        return linux_monitors()
    except (OSError, ValueError, subprocess.SubprocessError):
        return []
