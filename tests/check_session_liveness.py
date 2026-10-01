#!/usr/bin/env python3
"""Issue #7: real CLI/daemon, injected kill(positive_pid, 0) errno, synthetic capture.

This is NOT a real sandbox/TCC test. The fault-injection library exists only in the
isolated harness. Production code has no environment-variable liveness override.
Requires cargo build, cc, ffmpeg and ffprobe. Supports Darwin and Linux test hosts.
"""
import errno
import json
import os
import platform
from pathlib import Path
import subprocess
import sys
import tempfile
import time

from e2e import Harness, probe


INJECTOR = r'''
#include <sys/types.h>
#include <signal.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <dlfcn.h>

static int injected_errno(pid_t pid, int sig) {
    const char *path = getenv("REC_LIVENESS_TEST_PID_FILE");
    const char *number = getenv("REC_LIVENESS_TEST_ERRNO");
    if (!path || !number || sig != 0 || pid <= 0) return 0;
    FILE *file = fopen(path, "r");
    if (!file) return 0;
    long target = 0;
    int read = fscanf(file, "%ld", &target);
    fclose(file);
    if (read != 1 || target != (long)pid) return 0;
    int result = atoi(number);
    const char *trace = getenv("REC_LIVENESS_TEST_TRACE");
    if (trace) {
        file = fopen(trace, "a");
        if (file) { fprintf(file, "%ld:%d\n", (long)pid, result); fclose(file); }
    }
    return result;
}

#ifdef __APPLE__
static int test_kill(pid_t pid, int sig) {
    int error = injected_errno(pid, sig);
    if (error) { errno = error; return -1; }
    return kill(pid, sig);
}
__attribute__((used)) static struct {
    const void *replacement;
    const void *replacee;
} interpose __attribute__((section("__DATA,__interpose"))) = {
    (const void *)test_kill, (const void *)kill
};
#else
int kill(pid_t pid, int sig) {
    int error = injected_errno(pid, sig);
    if (error) { errno = error; return -1; }
    int (*original)(pid_t, int) = (int (*)(pid_t, int))dlsym(RTLD_NEXT, "kill");
    if (!original) { errno = EIO; return -1; }
    return original(pid, sig);
}
#endif
'''


def build_injector(root):
    source = root / 'deny-probe.c'
    source.write_text(INJECTOR)
    darwin = sys.platform == 'darwin'
    library = root / ('deny-probe.dylib' if darwin else 'deny-probe.so')
    flags = ['-dynamiclib'] if darwin else ['-shared', '-fPIC']
    if darwin and platform.machine() == 'arm64':
        # The injected environment reaches Apple's arm64e shell/Python children
        # as well as the arm64 Rust CLI and daemon on Apple Silicon runners.
        flags += ['-arch', 'arm64', '-arch', 'arm64e']
    command = ['cc', *flags, '-Wall', '-Wextra', str(source), '-o', str(library)]
    if not darwin:
        command += ['-ldl']
    subprocess.run(command, check=True, timeout=30)
    return library, 'DYLD_INSERT_LIBRARIES' if darwin else 'LD_PRELOAD'


def fault_environment(h, library, loader, target, error):
    return dict(h.env, **{
        loader: str(library),
        'REC_LIVENESS_TEST_PID_FILE': str(target),
        'REC_LIVENESS_TEST_ERRNO': str(error),
        'REC_LIVENESS_TEST_TRACE': str(h.root / 'native-probes.log'),
    })


def snapshot(h):
    session_path = h.home / '.agent-recorder/session'
    session = h.session()
    endpoint = Path(session['socket'])
    return (session_path.read_bytes(), session_path.stat().st_ino,
            endpoint.lstat().st_ino, endpoint.lstat().st_mode)


def invoke(h, env, *args):
    return subprocess.run([h.binary, *args], env=env, cwd=h.root,
                          capture_output=True, text=True, timeout=45)


def main():
    repo = Path(__file__).resolve().parents[1]
    binary = repo / 'target/debug/rec'
    assert binary.is_file(), 'run cargo build first'
    checks = []
    with tempfile.TemporaryDirectory(prefix='rec-s7-inject-') as temporary:
        library, loader = build_injector(Path(temporary))
        h = Harness(binary)
        try:
            h.call('start', '--window-id', '123', '--output', 'review.mp4')
            original = snapshot(h)
            pid = h.session()['pid']
            target = h.root / 'target-pid'
            target.write_text(str(pid))

            # A PATH-injected kill command would reproduce the old implementation's
            # destructive branch. The fixed CLI must never invoke this executable.
            trap = h.root / 'trap-bin'
            trap.mkdir()
            kill = trap / 'kill'
            kill.write_text('#!/bin/sh\necho external-kill-invoked >&2\nexit 1\n')
            kill.chmod(0o700)
            path = str(trap) + os.pathsep + h.env['PATH']
            env = dict(h.env, PATH=path)
            normal = invoke(h, env, 'note', 'native check ignores an external kill executable')
            assert normal.returncode == 0, (normal.stdout, normal.stderr)
            assert 'external-kill-invoked' not in normal.stderr
            assert snapshot(h) == original
            checks.append('PID inspection does not invoke an executable from PATH')

            for error, args in [
                (errno.EPERM, ('note', 'denied note')),
                (errno.EPERM, ('stop',)),
                (errno.EPERM, ('start', '--window-id', '123')),
                (errno.EACCES, ('note', 'inaccessible note')),
                (errno.EIO, ('note', 'uncertain I/O result')),
                (errno.ESRCH, ('note', 'even absent probes must not unlink')),
            ]:
                env = fault_environment(h, library, loader, target, error)
                env['PATH'] = path
                denied = invoke(h, env, *args)
                assert denied.returncode == 1, (args, denied.stdout, denied.stderr)
                assert 'external-kill-invoked' not in denied.stderr
                if error != errno.ESRCH:
                    assert 'left unchanged' in denied.stderr, denied.stderr
                    assert 'authorized host context' in denied.stderr, denied.stderr
                    assert 'No active recording' not in denied.stderr
                assert f'{pid}:{error}' in (h.root / 'native-probes.log').read_text()
                assert snapshot(h) == original
                # Same original daemon/listener can still receive host messages.
                assert h.ipc({'op': 'status'})['ok']
                h.call('note', f'host remains connected after injected errno {error}')
            checks.append('native EPERM/EACCES/EIO/ESRCH cannot unlink the original session or socket')
            checks.append('denied note/stop/start preserve bytes and socket inode; host IPC remains reachable')
            h.call('stop')
            assert any(s['codec_name'] == 'h264' for s in probe(h.root / 'review.mp4')['streams'])
            assert not (h.home / '.agent-recorder/session').exists()
            checks.append('the same host Run successfully stops and publishes a validated synthetic MP4')
        finally:
            h.close()

        # pid_alive also protects the daemon's active-test owner. Inject EPERM
        # only for the test CLI PID after TestBegin, not for the daemon itself.
        h = Harness(binary)
        child = None
        try:
            target = h.root / 'target-pid'
            target.write_text('0')
            h.env = fault_environment(h, library, loader, target, errno.EPERM)
            h.call('start', '--window-id', '123', '--output', 'test-owner.mp4')
            marker = h.root / 'child-started'
            program = f"import pathlib,time; pathlib.Path({str(marker)!r}).touch(); time.sleep(3)"
            child = subprocess.Popen([h.binary, 'test', sys.executable, '-c', program],
                                     env=h.env, cwd=h.root, stdout=subprocess.PIPE,
                                     stderr=subprocess.PIPE, text=True)
            deadline = time.monotonic() + 5
            while not marker.exists() and time.monotonic() < deadline:
                time.sleep(.02)
            assert marker.exists()
            target.write_text(str(child.pid))
            h.call('note', 'active test is inaccessible, not dead')
            stopped = h.call('stop', code=1)
            assert 'test is still running' in stopped.stderr, stopped.stderr
            assert not any(e['kind'] == 'test_result' for e in h.events())
            assert f'{child.pid}:{errno.EPERM}' in (h.root / 'native-probes.log').read_text()
            stdout, stderr = child.communicate(timeout=10)
            assert child.returncode == 0, (stdout, stderr)
            results = [e for e in h.events() if e['kind'] == 'test_result']
            assert len(results) == 1 and results[0]['test_result']['exit_code'] == 0
            assert not results[0]['test_result']['error']
            h.call('stop')
            checks.append('EPERM cannot reap a live test owner or permit premature stop')
        finally:
            if child is not None and child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
            h.close()

    evidence = repo / 'artifacts'
    evidence.mkdir(exist_ok=True)
    (evidence / 'session-liveness-report.json').write_text(json.dumps({
        'passed': checks,
        'faults': 'native kill(pid, 0) errno injection in isolated test processes',
        'capture': 'synthetic helper; real CLI, daemon and FFmpeg finalization',
        'live_sandbox_tested': False,
        'existing_damaged_recording_recovered': False,
    }, indent=2))
    print(f'{len(checks)} session-liveness scenario groups passed.')


if __name__ == '__main__':
    main()
