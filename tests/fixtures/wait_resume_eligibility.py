import atexit
import errno
import json
import os
import pty
import subprocess
import sys
from pathlib import Path

binary, run, name, directory, skills, legacy = sys.argv[1:]
legacy_dir = Path(directory).with_name(legacy)
unrelated = subprocess.Popen(['/bin/sleep', '30'], start_new_session=True)
def reap_unrelated():
    unrelated.terminate()
    unrelated.wait()
atexit.register(reap_unrelated)
record_file = legacy_dir / 'run.json'
record = json.loads(record_file.read_text())
record['attempt'] = 2
record['launch']['native']['pid'] = unrelated.pid
record_file.write_text(json.dumps(record))
(legacy_dir / 'run.pid').write_text(str(unrelated.pid))
piped_env = os.environ.copy()
piped_env.pop('OSC7577_TOKEN')
piped = subprocess.run([binary, 'wait', legacy], timeout=8, env=piped_env, capture_output=True)
assert piped.returncode == 0 and b'finished' in piped.stdout, piped
cases = [
    ([run], {}, True),
    ([name], {}, True),
    (['--run-dir', directory], {}, True),
    ([name], {'background': True}, False),
    ([name], {'token': False}, False),
    ([name], {'custom': True}, False),
    ([run, '--repo', '.'], {}, False),
    ([run, '--timeout', '1'], {}, False),
    ([run, '--all'], {}, False),
    ([run, run], {}, False),
    ([run, '--json'], {}, False),
    ([legacy], {}, False),
]
for args, options, expected in cases:
    pid, master = pty.fork()
    if pid == 0:
        env = os.environ.copy()
        if not options.get('token', True):
            env.pop('OSC7577_TOKEN')
        if options.get('custom'):
            env['HERDER_SKILLS_DIR'] = skills
        group = os.setpgrp if options.get('background') else None
        child = subprocess.run([binary, 'wait', *args], timeout=8, env=env, preexec_fn=group)
        os._exit(child.returncode)
    result = bytearray()
    while True:
        try:
            chunk = os.read(master, 4096)
        except OSError as error:
            if error.errno != errno.EIO:
                raise
            break
        if not chunk:
            break
        result.extend(chunk)
    os.close(master)
    assert os.waitpid(pid, 0)[1] == 0, (args, options, result)
    assert b'finished' in result, (args, options, result)
    assert (b'\x1b]7577;' in result) == expected, (args, options, result)
