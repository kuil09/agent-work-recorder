#!/usr/bin/env python3
"""Synthetic telemetry failures with real CLI/daemon/remux; no live capture claims."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time

from e2e import Harness, probe

SCRIPT = r'''
import json, os, pathlib, subprocess, sys, threading, time
started = time.monotonic()
lock = threading.Lock()
def emit(value):
    with lock: print(json.dumps(value), flush=True)
def telemetry():
    while True:
        mode = pathlib.Path(os.environ['REC_HEALTH_MODE']).read_text()
        elapsed = int((time.monotonic() - started) * 1000)
        health = dict(state='receiving', first_frame='validated', target_available=True,
                      frames_received=1, frames_written=elapsed//33+1,
                      last_frame_ms=0, last_frame_age_ms=elapsed,
                      last_sample_age_ms=0, capture_error=None, visual_warning=None, intervals=[])
        if mode == 'lost': health['target_available'] = False
        if mode == 'missing': health['last_sample_age_ms'] = 4000
        if mode == 'dark': health['visual_warning'] = 'source pixels mostly dark'
        if mode == 'error':
            health['capture_error'] = 'injected capture delivery failure'
            emit(dict(event='error', elapsed_ms=elapsed, message=health['capture_error']))
        if mode == 'pending':
            health.update(first_frame='pending', frames_received=0, frames_written=0, last_frame_ms=None, last_frame_age_ms=None, last_sample_age_ms=None)
        emit(dict(event='health', elapsed_ms=elapsed, health=health))
        time.sleep(.2)
threading.Thread(target=telemetry, daemon=True).start()
emit(dict(event='ready', elapsed_ms=0))
args = sys.argv[1:]
output = args[args.index('--output') + 1]
for line in sys.stdin:
    if json.loads(line)['cmd'] == 'stop':
        subprocess.run(['ffmpeg', '-v', 'error', '-f', 'lavfi', '-i', f'color=size=320x240:duration={max(1,time.monotonic()-started):.3f}', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', output], check=True)
        emit(dict(event='stopped', path=output))
        break
'''

def main():
    repo = Path(__file__).resolve().parents[1]
    h = Harness(repo / 'target/debug/rec')
    try:
        script = h.root / 'health-helper.py'; script.write_text(SCRIPT)
        wrapper = h.root / 'capture-helper'
        wrapper.write_text('#!/bin/sh\nexec ' + json.dumps(sys.executable) + ' ' + json.dumps(str(script)) + ' "$@"\n')
        mode = h.root / 'mode'; mode.write_text('dark')
        h.env['REC_HEALTH_MODE'] = str(mode)
        h.call('start', '--window-id', '123', '--no-git-context', '--output', 'health.mp4')
        session_path = h.home / '.agent-recorder/session'
        saved_session = session_path.read_bytes()
        saved_events = h.events_path().read_bytes() if h.events_path().exists() else b''
        def wait(expected):
            deadline = time.monotonic() + 4
            while time.monotonic() < deadline:
                r = json.loads(h.call('status', '--json').stdout)
                if r['capture_health']['state'] == expected: return r
                time.sleep(.05)
            raise AssertionError((expected, r))
        r = wait('visual_warning'); assert r['capture_health']['target_available'] is True
        assert r['capture_target'] == 'window ID 123'
        for selected, expected in [('pending', 'unverified'), ('missing', 'missing_frames'), ('lost', 'target_lost'), ('normal', 'receiving'), ('error', 'capture_error')]:
            mode.write_text(selected); r = wait(expected)
            if selected == 'pending': assert r['capture_health']['first_frame'] == 'pending'
            if selected == 'error': assert r['capture_health']['target_available'] is True
        assert saved_session == session_path.read_bytes()
        assert saved_events == (h.events_path().read_bytes() if h.events_path().exists() else b'')
        assert 'annotation saved to diagnostics' in h.call('note', 'Capture is impaired; evidence is diagnostic only').stdout
        stopped = h.call('stop')
        assert 'target_lost from' in stopped.stdout and 'missing_frames from' in stopped.stdout and 'capture_error from' in stopped.stdout
        assert any(s['codec_name'] == 'h264' for s in probe(h.root / 'health.mp4')['streams'])
        retained = h.tmp / 'agent-recorder' / r['run_id']
        assert (retained / 'raw.mp4').is_file() and (retained / 'capture-health.json').is_file()
        assert not session_path.exists()
        print('PASS: read-only status, pending/dark/missing/lost/error separation, recovery intervals, MP4 and diagnostics preserved; requested window scope unchanged')
    finally:
        h.close()

if __name__ == '__main__': main()
