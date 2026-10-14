"""Open a local page in a browser on the desktop the user sits at.

On WSL the desktop is Windows: Edge opens in kiosk mode, so the page fills
the screen without a click, with its own profile so a running Edge does not
swallow the kiosk switches. On Linux xdg-open picks the user's browser and
the page asks for fullscreen itself.
"""
import os
from pathlib import Path
import shutil
import subprocess

EDGE_PROFILE = r'%LOCALAPPDATA%\tobii-linux\edge-setup'
WINDOWS_DRIVE = Path('/mnt/c')


def running_under_wsl(environ=os.environ, proc_version=Path('/proc/version')):
    if environ.get('WSL_DISTRO_NAME'):
        return True
    try:
        return 'microsoft' in proc_version.read_text().lower()
    except OSError:
        return False


def edge_kiosk_command(url):
    return ['cmd.exe', '/c', 'start', 'msedge', '--kiosk', url, '--edge-kiosk-type=fullscreen',
            '--no-first-run', f'--user-data-dir={EDGE_PROFILE}']


def open_command(url, wsl=None):
    """The command that shows url to the user, or None when no launcher is available."""
    if running_under_wsl() if wsl is None else wsl:
        return edge_kiosk_command(url) if shutil.which('cmd.exe') else None
    return ['xdg-open', url] if shutil.which('xdg-open') else None


def open_page(url, wsl=None):
    """Start the browser without waiting for it; returns the command used, or None."""
    command = open_command(url, wsl)
    if command is None:
        return None
    working_directory = WINDOWS_DRIVE if command[0] == 'cmd.exe' and WINDOWS_DRIVE.is_dir() else None
    subprocess.Popen(command, cwd=working_directory, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                     stderr=subprocess.DEVNULL, start_new_session=True)
    return command
