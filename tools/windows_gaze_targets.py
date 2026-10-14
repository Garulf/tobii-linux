#!/usr/bin/env python3
"""Fullscreen gaze targets on the primary monitor, with a JSON-lines timing log.

Shows a countdown, then one dot at a time at normalized display positions,
each for --dwell seconds. Every target is logged with its normalized position
and its start and end times on two clocks: QueryPerformanceCounter in
microseconds (the Stream Engine system clock on Windows) and wall-clock time.
Press Escape to abort. Run it in the desktop session, not over SSH.
"""
import argparse
import json
import time
import tkinter

TARGETS = ((0.5, 0.5), (0.1, 0.1), (0.9, 0.1), (0.9, 0.9), (0.1, 0.9), (0.5, 0.5),
           (0.3, 0.5), (0.7, 0.5))
DOT_RADIUS = 14
COUNTDOWN_S = 5


def clocks():
    return {'qpc_us': time.perf_counter_ns() // 1000, 'wall': time.time()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', required=True)
    parser.add_argument('--dwell', type=float, default=3.0)
    args = parser.parse_args()
    root = tkinter.Tk()
    root.attributes('-fullscreen', True)
    root.attributes('-topmost', True)
    root.configure(background='black', cursor='none')
    width, height = root.winfo_screenwidth(), root.winfo_screenheight()
    canvas = tkinter.Canvas(root, width=width, height=height, background='black', highlightthickness=0)
    canvas.pack()
    log = open(args.output, 'w')

    def write(kind, **values):
        log.write(json.dumps({'kind': kind, **values, **clocks()}) + '\n')
        log.flush()

    def finish(_event=None):
        write('end')
        log.close()
        root.destroy()

    def countdown(remaining):
        canvas.delete('all')
        if remaining == 0:
            show(0)
            return
        canvas.create_text(width / 2, height / 2, fill='white', font=('Segoe UI', 40),
                           text=f'Look at each dot until it moves\n\nstarting in {remaining}',
                           justify='center')
        root.after(1000, countdown, remaining - 1)

    def show(index):
        if index > 0:
            write('target_end', index=index - 1)
        if index == len(TARGETS):
            finish()
            return
        x, y = TARGETS[index]
        canvas.delete('all')
        cx, cy = x * width, y * height
        canvas.create_oval(cx - DOT_RADIUS, cy - DOT_RADIUS, cx + DOT_RADIUS, cy + DOT_RADIUS,
                           fill='white', outline='')
        canvas.create_oval(cx - 3, cy - 3, cx + 3, cy + 3, fill='red', outline='')
        write('target_start', index=index, x=x, y=y)
        root.after(int(args.dwell * 1000), show, index + 1)

    write('screen', width=width, height=height)
    root.bind('<Escape>', finish)
    countdown(COUNTDOWN_S)
    root.mainloop()
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
