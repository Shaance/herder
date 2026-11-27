import fcntl
import os
import pty
import select
import signal
import subprocess
import sys
import time
from pathlib import Path

binary, run, directory, decoy = sys.argv[1:]
alias = Path(os.environ['HERDER_STATE_DIR'])
lock = open(Path(directory) / 'run.lock', 'r+')
fcntl.flock(lock, fcntl.LOCK_EX)
notify_read, notify_write = os.pipe()
session, master = pty.fork()
if session == 0:
    os.close(notify_write)
    lock.close()
    signal.signal(signal.SIGTTOU, signal.SIG_IGN)
    child = subprocess.Popen([binary, 'wait', run], preexec_fn=os.setpgrp)
    code = 1
    try:
        if os.read(notify_read, 1):
            os.kill(child.pid, signal.SIGSTOP)
            os.waitpid(child.pid, os.WUNTRACED)
            os.tcsetpgrp(1, child.pid)
            os.kill(child.pid, signal.SIGCONT)
            code = child.wait(timeout=8)
    finally:
        if child.poll() is None:
            child.kill()
            child.wait()
        os._exit(code)
os.close(notify_read)
output = bytearray()
def read_until(marker):
    deadline = time.monotonic() + 8
    while marker not in output:
        ready, _, _ = select.select([master], [], [], max(0, deadline - time.monotonic()))
        assert ready, (marker, output)
        chunk = os.read(master, 4096)
        assert chunk, (marker, output)
        output.extend(chunk)
status = None
try:
    read_until(b'\trunning')
    assert b'\x1b]7577;' not in output, output
    os.write(notify_write, b'1')
    read_until(b':argv=')
    alias.unlink()
    alias.symlink_to(decoy, target_is_directory=True)
    fcntl.flock(lock, fcntl.LOCK_UN)
    read_until(b'final.md')
    assert (directory + '/final.md').encode() in output, output
    assert b':clear=1' in output, output
    _, status = os.waitpid(session, 0)
finally:
    os.close(notify_write)
    os.close(master)
    lock.close()
    if status is None:
        os.waitpid(session, 0)
assert status == 0, status
