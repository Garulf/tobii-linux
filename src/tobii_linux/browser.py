"""Open a local page in a browser on the desktop the user sits at, optionally on a given monitor.

On WSL the desktop is Windows: Edge opens in kiosk mode, so the page fills
the screen without a click, with its own profile so a running Edge does not
swallow the kiosk switches. On Linux xdg-open picks the user's browser and
the page asks for fullscreen itself.

To put the page on a particular monitor the browser is started at that
monitor's position (`--window-position`) in kiosk mode, which needs Edge on
WSL or a Chromium-family browser on Linux under X11 or XWayland; each
monitor gets its own browser profile so a new kiosk window opens there even
while another one is still up.
"""
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

EDGE_PROFILE = r'%LOCALAPPDATA%\tobii-linux\edge-setup'
WINDOWS_DRIVE = Path('/mnt/c')
CHROMIUM_BROWSERS = ('chromium', 'chromium-browser', 'google-chrome', 'google-chrome-stable', 'microsoft-edge',
                     'microsoft-edge-stable', 'brave-browser')


def running_under_wsl(environ=os.environ, proc_version=Path('/proc/version')):
    if environ.get('WSL_DISTRO_NAME'):
        return True
    try:
        return 'microsoft' in proc_version.read_text().lower()
    except OSError:
        return False


def profile_suffix(monitor):
    return '' if monitor is None else '-' + re.sub(r'[^A-Za-z0-9]+', '-', monitor.key).strip('-')[-40:]


def placement(monitor):
    if monitor is None or not monitor.bounds:
        return []
    x, y = monitor.bounds[:2]
    return [f'--window-position={x},{y}']


def edge_kiosk_command(url, monitor=None):
    return ['cmd.exe', '/c', 'start', 'msedge', '--kiosk', url, '--edge-kiosk-type=fullscreen', '--no-first-run',
            *placement(monitor), f'--user-data-dir={EDGE_PROFILE}{profile_suffix(monitor)}']


def chromium_browser(environ=os.environ):
    return next((path for path in map(shutil.which, CHROMIUM_BROWSERS) if path), None) if environ.get('DISPLAY') else None


def chromium_kiosk_command(browser, url, monitor, environ=os.environ):
    profile = Path(tempfile.gettempdir())/f'tobii-linux-setup{profile_suffix(monitor)}'
    x11 = ['--ozone-platform=x11'] if environ.get('WAYLAND_DISPLAY') else []
    return [browser, *x11, '--kiosk', '--no-first-run', *placement(monitor), f'--user-data-dir={profile}', url]


def can_place_on_monitor(wsl=None, environ=os.environ):
    """Whether open_page can put the page on a chosen monitor."""
    if running_under_wsl(environ) if wsl is None else wsl:
        return shutil.which('cmd.exe') is not None
    return chromium_browser(environ) is not None


def open_command(url, wsl=None, monitor=None, environ=os.environ):
    """The command that shows url to the user (on `monitor` when given), or None when no launcher is available."""
    if running_under_wsl(environ) if wsl is None else wsl:
        return edge_kiosk_command(url, monitor) if shutil.which('cmd.exe') else None
    browser = chromium_browser(environ) if monitor is not None else None
    if browser:
        return chromium_kiosk_command(browser, url, monitor, environ)
    return ['xdg-open', url] if shutil.which('xdg-open') else None


def open_page(url, wsl=None, monitor=None):
    """Start the browser without waiting for it; returns the command used, or None."""
    command = open_command(url, wsl, monitor)
    if command is None:
        return None
    working_directory = WINDOWS_DRIVE if command[0] == 'cmd.exe' and WINDOWS_DRIVE.is_dir() else None
    subprocess.Popen(command, cwd=working_directory, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                     stderr=subprocess.DEVNULL, start_new_session=True)
    return command
