#!/usr/bin/env python3
"""Exercise the real CLI/daemon/remux with a synthetic capture helper, not a desktop.

Requires cargo build, Python 3 and ffmpeg/ffprobe. All sessions use isolated HOME/TMPDIR.
The synthetic helper is deliberately confined to this test file.
"""
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time


def helper(args):
    trace = Path(os.environ['REC_E2E_TRACE'])

    def log(value):
        with trace.open('a', encoding='utf-8') as out:
            out.write(json.dumps(value, ensure_ascii=False) + '\n')

    log({'argv': args})
    if os.environ.get('REC_E2E_MODE') == 'fail-start':
        print(json.dumps({'event': 'error', 'message': 'synthetic capture failure'}), flush=True)
        return 2
    assert args[0] == 'start', args
    output = Path(args[args.index('--output') + 1])
    audio = '--system-audio' in args and os.environ.get('REC_E2E_MODE') != 'drop-audio'
    started = time.monotonic()
    print(json.dumps({'event': 'ready', 'elapsed_ms': 0}), flush=True)
    for line in sys.stdin:
        request = json.loads(line)
        log(request)
        if request['cmd'] == 'stop':
            duration = max(.2, time.monotonic() - started)
            command = ['ffmpeg', '-nostdin', '-y', '-v', 'error', '-f', 'lavfi', '-i',
                       f'color=size=320x240:rate=30:duration={duration:.6f}']
            if audio:
                command += ['-f', 'lavfi', '-i', f'sine=frequency=440:duration={duration:.6f}']
            command += ['-c:v', 'libx264', '-pix_fmt', 'yuv420p']
            if audio:
                command += ['-c:a', 'aac', '-shortest']
            subprocess.run(command + [str(output)], check=True, timeout=30)
            print(json.dumps({'event': 'stopped', 'path': str(output)}), flush=True)
            return 0
    return 1


class Harness:
    def __init__(self, binary, mode='normal'):
        self.root = Path(tempfile.mkdtemp(prefix='rec-e2e-', dir='/tmp')).resolve()
        self.home = self.root / 'home'
        self.tmp = self.root / 'tmp'
        self.home.mkdir()
        self.tmp.mkdir()
        self.trace = self.root / 'capture.jsonl'
        wrapper = self.root / 'capture-helper'
        wrapper.write_text('#!/bin/sh\nexec ' + '\"' + sys.executable + '\"' + ' ' + '\"' + str(Path(__file__).resolve()) + '\"' + ' --helper "$@"\n')
        wrapper.chmod(0o700)
        self.binary = str(binary)
        self.env = dict(os.environ, HOME=str(self.home), TMPDIR=str(self.tmp),
                        REC_CAPTURE=str(wrapper), REC_E2E_TRACE=str(self.trace), REC_E2E_MODE=mode,
                        PHASE2_TEST_VALUE='caller environment')

    def call(self, *args, code=0, cwd=None):
        result = subprocess.run([self.binary, *args], env=self.env, cwd=cwd or self.root,
                                capture_output=True, text=True, timeout=45)
        assert result.returncode == code, (args, result.returncode, result.stdout, result.stderr)
        return result

    def session(self):
        return json.loads((self.home / '.agent-recorder/session').read_text())

    def events_path(self):
        return self.tmp / 'agent-recorder' / self.session()['run_id'] / 'events.jsonl'

    def events(self):
        return [json.loads(line) for line in self.events_path().read_text().splitlines()]

    def ipc(self, message):
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
            stream.settimeout(5)
            stream.connect(self.session()['socket'])
            stream.sendall((json.dumps(message) + '\n').encode())
            with stream.makefile() as reader:
                return json.loads(reader.readline())

    def close(self):
        try:
            session = self.session()
            pid = int(session['pid'])
            # Only this harness's detached daemon owns this isolated session directory.
            if os.getpgid(pid) == pid:
                os.killpg(pid, signal.SIGKILL)
        except (FileNotFoundError, ProcessLookupError):
            pass
        shutil.rmtree(self.root, ignore_errors=True)


def probe(path):
    return json.loads(subprocess.check_output(['ffprobe', '-v', 'error', '-show_format',
                                               '-show_streams', '-show_chapters', '-of', 'json', str(path)]))


def main():
    repo = Path(__file__).resolve().parents[1]
    binary = (repo / 'target/debug/rec').resolve()
    assert binary.is_file(), 'run cargo build first'
    evidence = repo / 'artifacts'
    evidence.mkdir(exist_ok=True)
    checks = []
    h = Harness(binary)
    try:
        h.call('note', 'no session', code=1)
        h.call('start', '--app', 'com.example.Synthetic', '--system-audio', '--title', '한글 # =', '--output', 'review.mp4')
        h.call('start', code=1)
        h.call('observe', '--status', 'pass', 'an agent claim before the test')
        nested = h.root / 'nested'
        nested.mkdir()
        program = "import os,sys; print(os.getcwd()); print(os.environ['PHASE2_TEST_VALUE']); print(sys.argv[1]); print('stderr-tail',file=sys.stderr); sys.exit(7)"
        result = h.call('test', '--', sys.executable, '-c', program, 'literal $(exit 9); 한글', code=7, cwd=nested)
        assert str(nested) in result.stdout and 'caller environment' in result.stdout
        assert 'literal $(exit 9); 한글' in result.stdout
        last = h.events()[-1]
        assert last['kind'] == 'test_result' and last['test_result']['exit_code'] == 7
        assert last['test_result']['stderr_summary'] == 'stderr-tail\n'
        assert 'PASS' not in last['text']
        checks.append('caller cwd/environment, literal argv, both output streams, exit 7, no automatic PASS')

        h.call('test', '--timeout-secs', '1', sys.executable, '-c', 'import time; time.sleep(30)', code=124)
        assert h.events()[-1]['test_result']['timed_out']
        h.call('test', '/nonexistent/rec-e2e-executable', code=127)
        assert h.events()[-1]['test_result']['error']
        checks.append('timeout 124 and spawn failure 127 are recorded')

        marker = h.root / 'test-started'
        program = f"import pathlib,time; pathlib.Path({str(marker)!r}).write_text('started'); time.sleep(2); print('x'*20000)"
        child = subprocess.Popen([h.binary, 'test', '--', sys.executable, '-c', program],
                                 env=h.env, cwd=h.root, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            deadline = time.monotonic() + 5
            while not marker.exists() and time.monotonic() < deadline:
                time.sleep(.02)
            assert marker.exists()
            h.call('stop', code=1)
            rejected = h.root / 'must-not-exist'
            h.call('test', sys.executable, '-c', f"open({str(rejected)!r},'w').close()", code=1)
            assert not rejected.exists()
            assert not h.ipc({'op': 'test_end', 'run_id': 'WRONG', 'test_step': 1, 'result': {
                'exit_code': 0, 'signal': None, 'duration_ms': 0, 'timed_out': False,
                'stdout_summary': '', 'stderr_summary': '', 'error': None}})['ok']
            h.call('note', 'daemon remains responsive during a test')
            stdout, stderr = child.communicate(timeout=8)
            assert child.returncode == 0, (stdout, stderr)
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()
        assert len(h.events()[-1]['test_result']['stdout_summary'].encode()) <= 4099
        checks.append('daemon responsiveness, active-test stop/reentry guard, wrong-run rejection, bounded output')

        checkpoint = '최종 확인 # = ; \\'
        h.call('checkpoint', checkpoint)
        events = h.events()
        results = [e for e in events if e['kind'] == 'test_result']
        assert len(results) == 4
        assert [e['step'] for e in events] == list(range(1, len(events) + 1))
        assert [e['media_ms'] for e in events] == sorted(e['media_ms'] for e in events)
        tmp = h.events_path().parent
        stop = h.call('stop')
        assert 'Tests      4' in stop.stdout
        output = h.root / 'review.mp4'
        data = probe(output)
        assert any(s['codec_name'] == 'aac' for s in data['streams'])
        assert data['format']['tags']['title'] == '한글 # ='
        assert len(data['chapters']) == 6
        assert data['chapters'][-1]['tags']['title'].endswith(checkpoint)
        for a, b in zip(data['chapters'], data['chapters'][1:]):
            assert float(a['end_time']) == float(b['start_time'])
        assert not tmp.exists() and not (h.home / '.agent-recorder/session').exists()
        trace = [json.loads(line) for line in h.trace.read_text().splitlines()]
        assert '--app' in trace[0]['argv'] and '--system-audio' in trace[0]['argv']
        assert any(e.get('cmd') == 'hud' and e.get('verdict', 'absent') is None for e in trace)
        checks.append('audio/chapter mux, exact Unicode/special-character titles, sequential steps, cleanup and counts')
        shutil.copy2(output, evidence / 'e2e-synthetic-with-audio.mp4')
        (evidence / 'e2e-capture-protocol.json').write_text(json.dumps(trace, ensure_ascii=False, indent=2))
        (evidence / 'e2e-media.json').write_text(json.dumps(data, ensure_ascii=False, indent=2))
        h.call('start', '--output', 'review.mp4', code=1)
        checks.append('existing output is never overwritten')
    finally:
        h.close()

    h = Harness(binary)
    try:
        h.call('start', '--screen', 'full', '--output', 'silent.mp4')
        h.call('checkpoint', 'no system audio requested')
        h.call('stop')
        assert not any(s['codec_type'] == 'audio' for s in probe(h.root / 'silent.mp4')['streams'])
        checks.append('audio is absent unless explicitly requested')
    finally:
        h.close()

    h = Harness(binary, 'drop-audio')
    try:
        h.call('start', '--system-audio', '--output', 'must-not-publish.mp4')
        failure = h.call('stop', code=1)
        assert 'no AAC audio' in failure.stderr
        assert not (h.root / 'must-not-publish.mp4').exists()
        assert (h.tmp / 'agent-recorder' / h.session()['run_id'] / 'raw.mp4').exists()
        checks.append('missing requested audio fails finalization and retains raw capture')
    finally:
        h.close()

    for options in [('--app', 'com.example.Synthetic'), ('--system-audio',), ('--window-id', '123'),
                    ('--screen', 'full'), ()]:
        h = Harness(binary, 'fail-start')
        try:
            result = h.call('start', *options, code=1)
            assert 'refusing to expand scope or drop requested audio' in result.stderr
            assert not (h.home / '.agent-recorder/session').exists()
            # A Run that never started must not leave args/temp directories behind.
            leftovers = list((h.tmp / 'agent-recorder').iterdir())
            assert not leftovers, leftovers
        finally:
            h.close()
    checks.append('app/window/audio capture never silently degrades to full-screen silent fallback')

    h = Harness(binary)
    try:
        started = h.call('start', '--output', 'out.mp4', '--no-git-context')
        assert 'not collected' in started.stdout
        session = h.session()
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stalled:
            stalled.connect(session['socket'])  # connects but never sends a request
            began = time.monotonic()
            h.call('note', 'a stalled client must not block the recorder')
            assert time.monotonic() - began < 3
        status = h.call('status')
        assert session['run_id'] in status.stdout and 'Steps      1' in status.stdout
        h.call('test', '--', 'sh', '-c', 'echo DB_PASSWORD=hunter2')
        masked = json.dumps(h.events()[-2:])
        assert 'hunter2' not in masked and 'DB_PASSWORD=***' in masked
        h.call('test', '--no-output-summary', '--', 'sh', '-c', 'echo plain-output')
        last = h.events()[-1]['test_result']
        assert last['stdout_summary'] == '' and last['stderr_summary'] == ''
        checks.append('stalled clients, read-only status, secret masking and --no-output-summary')

        # A test whose CLI is gone (or cannot be probed) must not block stop forever.
        assert h.ipc({'op': 'test_begin', 'command': ['lost'], 'cwd': '/', 'owner_pid': os.getpid(),
                      'timeout_secs': 300})['ok']
        assert 'Test running' in h.call('status').stdout
        h.call('stop', code=1)
        (h.root / 'out.mp4').write_text('someone else created this during the run')
        stop = h.call('stop', '--abandon-test')
        assert 'Warning' in stop.stdout and 'already exists' in stop.stdout
        assert (h.root / 'out.mp4').read_text().startswith('someone else')
        assert (h.root / f"out-{session['run_id']}.mp4").is_file()
        assert not (h.home / '.agent-recorder/session').exists()
        checks.append('stop --abandon-test and output collision fallback without losing the recording')
    finally:
        h.close()
    (evidence / 'e2e-report.json').write_text(json.dumps({'passed': checks, 'capture': 'synthetic helper, not ScreenCaptureKit'}, indent=2))
    print(f'{len(checks)} end-to-end scenario groups passed (synthetic capture; real CLI/daemon/ffmpeg).')


if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == '--helper':
        sys.exit(helper(sys.argv[2:]))
    main()
