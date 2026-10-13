"""Read-only hardware inventory entry point."""
import argparse
import json
import sys
from .usb import USBContext, USBError


def main():
    parser = argparse.ArgumentParser(description='Native Eye Tracker 5 transport research')
    parser.add_subparsers(dest='command', required=True).add_parser(
        'probe', help='Inspect standard USB descriptors without claiming interfaces')
    parser.parse_args()
    try:
        with USBContext() as context:
            devices = context.probe()
        print(json.dumps({'status': 'transport-only; tracking protocol incomplete',
                          'devices': devices}, indent=2))
        return 0
    except (USBError, OSError, RuntimeError) as error:
        print(json.dumps({'error': str(error)}), file=sys.stderr)
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
