import base64
from contextlib import contextmanager
import errno
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import signal
import sys
import time

binary, run, name, directory = sys.argv[1:]
run_dir = Path(directory)
record_path = run_dir / 'run.json'
record = json.loads(record_path.read_text())
launch = record['launch']
native = launch['native']
repo = record['repo']
fixture = Path(os.environ['CLAUDE_FIXTURE'])
calls_path = fixture / 'agents-calls'
osc = b'\x1b]7577;'


def calls():
    return calls_path.read_text().splitlines() if calls_path.exists() else []


def listing(**changes):
    worker = dict(id=native['id'], kind='background', sessionId=native['session_id'],
                  cwd=repo, status='busy', state='working')
    worker.update(changes)
    (fixture / 'agents.json').write_text(json.dumps([worker]))


def save_launch(value):
    record_path.write_text(json.dumps(dict(record, launch=value)))


events = []


def note(event):
    events.append((time.monotonic(), event))


def send(pid, sig):
    note(f'send {signal.Signals(sig).name}')
    os.kill(pid, sig)


def describe(status):
    if os.WIFSIGNALED(status):
        return f'killed by {signal.Signals(os.WTERMSIG(status)).name}'
    return f'exit {os.WEXITSTATUS(status)}'


def timeline():
    start = events[0][0] if events else 0
    return '; '.join(f'+{(at - start) * 1000:.0f}ms {event}' for at, event in events)


@contextmanager
def monitor(*args, exit_code=None):
    events.clear()
    pid, master = pty.fork()
    if pid == 0:
        # A launcher can start the fixture with SIGINT ignored or blocked, and herder
        # deliberately keeps an inherited ignore; give the monitor a terminal's default.
        signal.signal(signal.SIGINT, signal.SIG_DFL)
        signal.pthread_sigmask(signal.SIG_UNBLOCK, {signal.SIGINT})
        os.execv(binary, [binary, 'wait', *args])
    note(f'spawn wait {" ".join(args)}')
    output = bytearray()

    def read_until(marker, after=0, timeout=8):
        deadline = time.monotonic() + timeout
        while output.find(marker, after) == -1:
            ready, _, _ = select.select([master], [], [], max(0, deadline - time.monotonic()))
            if not ready:
                raise TimeoutError(args, marker, output)
            chunk = os.read(master, 4096)
            assert chunk, (args, marker, output)
            output.extend(chunk)
        note(f'saw {marker!r}')
        return output

    try:
        yield pid, read_until
    finally:
        if exit_code is None:
            send(pid, signal.SIGINT)
        deadline = time.monotonic() + 3
        while True:
            exited, status = os.waitpid(pid, os.WNOHANG)
            if exited:
                note(f'reaped: {describe(status)}')
                os.close(master)
                note('closed master')
                break
            if time.monotonic() >= deadline:
                send(pid, signal.SIGKILL)
                _, status = os.waitpid(pid, 0)
                note(f'reaped: {describe(status)}')
                # Close the terminal only after reaping so the monitor cannot see a hangup.
                os.close(master)
                note('closed master')
                break
            ready, _, _ = select.select([master], [], [], 0.05)
            if ready:
                try:
                    output.extend(os.read(master, 4096))
                except OSError as error:
                    if error.errno != errno.EIO:
                        raise
        expected = -signal.SIGINT if exit_code is None else exit_code
        assert os.waitstatus_to_exitcode(status) == expected, (
            f'monitor {args} expected exit code {expected}, got {describe(status)} (raw {status}); '
            f'events: {timeline()}; output tail: {bytes(output[-600:])!r}'
        )


def assert_argv(output):
    packet = re.search(rb'\x1b\]7577;[^\x1b]*:argv=([^:]+):mac=', output)
    assert packet, output
    argv = [base64.b64decode(arg).decode() for arg in packet[1].split(b',')]
    assert argv == [binary, 'wait', '--run-dir', directory], argv


listing()
for args in ([run], [name], ['--run-dir', directory]):
    before = len(calls())
    with monitor(*args) as (_, until):
        assert_argv(until(b'\trunning'))
        assert calls()[before:] == [f"{native['profile']}\t{repo}\tagents --json --all"]

for changes in (dict(sessionId='different-session'), dict(cwd=directory), dict(id='other')):
    listing(**changes)
    with monitor(run) as (_, until):
        assert osc not in until(b'\tunknown')

listing(status=None, state='stopped')
with monitor(run, exit_code=1) as (_, until):
    assert osc not in until(b'\tstopped')

listing()
save_launch(dict(state='pending_resume', previous=native, attempt=2, error=None))
with monitor(run) as (_, until):
    assert osc not in until(b'\trunning')

save_launch(dict(state='accepted', id=native['id'], profile=None, error=None))
listing(id='other')
with monitor(run) as (_, until):
    assert osc not in until(b'\tunknown')
    assert json.loads(record_path.read_text())['launch']['state'] == 'accepted'
    listing()
    assert_argv(until(b'\trunning'))
    assert json.loads(record_path.read_text())['launch']['native'] == dict(native, profile=None)
    assert calls()[-1] == f'unset\t{repo}\tagents --json --all'

save_launch(launch)
before = len(calls())
with (run_dir / 'run.lock').open('r+') as lock:
    fcntl.flock(lock, fcntl.LOCK_EX)
    with monitor(run, exit_code=0) as (pid, until):
        assert osc not in until(b'\trunning')
        assert len(calls()) == before
        fcntl.flock(lock, fcntl.LOCK_UN)
        assert_argv(until(b':mac='))
        (fixture / 'agents.json').write_text('[]')
        until(b'\tunknown')
        withdrawn = len(until(b':clear=1'))
        send(pid, signal.SIGCONT)
        try:
            until(b':argv=', withdrawn, timeout=0.5)
        except TimeoutError:
            pass
        else:
            raise AssertionError('SIGCONT published a withdrawn record')

        listing()
        recovered = until(b'\trunning', withdrawn)
        assert_argv(recovered[withdrawn:])
        assert json.loads(record_path.read_text())['launch'] == launch

        resumed = len(recovered)
        send(pid, signal.SIGCONT)
        assert_argv(until(b':mac=', resumed)[resumed:])
        completion = len(until(b'\x1b\\', resumed))
        listing(status=None, state='done')
        until(b'\tfinished', completion)
        until(b':clear=1', completion)

listing()
with monitor(run, exit_code=-signal.SIGINT) as (pid, until):
    assert_argv(until(b'\trunning'))
    (fixture / 'agents.json').write_text('[]')
    withdrawn = len(until(b'\tunknown'))
    listing()
    recovered = until(b'\trunning', withdrawn)
    assert_argv(recovered[withdrawn:])
    interruption = len(recovered)
    send(pid, signal.SIGINT)
    until(b':clear=1', interruption)
