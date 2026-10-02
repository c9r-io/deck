#!/usr/bin/env python3
"""Focused Translation Lens setup acceptance in a real isolated WKWebView.

Build once, run controlled download races, then real HTTPS installation and
Bergamot translation, restart enabled, and restart disabled. No clipboard
access. Uses only synthetic shell output and native input in the test window.
Evidence and private test data stay under a fresh /tmp directory; no release,
version changes, production settings, or production sessions are touched.
"""
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent
BUNDLE = ROOT / 'app/src-tauri/target/debug/deck-smoke.app'
RUN = Path(tempfile.mkdtemp(prefix='deck-translation-setup-', dir='/tmp')).resolve()
REPORT = {'verdict': 'FAIL', 'runRoot': str(RUN), 'runs': [], 'cleanup': []}
SOCKETS = []


def command(args, log, **kwargs):
    with open(RUN / log, 'w') as out:
        result = subprocess.run(args, cwd=ROOT, stdout=out, stderr=subprocess.STDOUT, **kwargs)
    return result.returncode


def pids(data):
    text = subprocess.check_output(['ps', '-axo', 'pid=,command='], text=True)
    executable = str(BUNDLE / 'Contents/MacOS/deck')
    return [int(line.strip().split(None, 1)[0]) for line in text.splitlines()
            if len(line.strip().split(None, 1)) == 2
            and line.strip().split(None, 1)[1].startswith(executable + ' ')
            and f'--smoke-data-dir {data}' in line]


def stop(data):
    for pid in pids(data):
        os.kill(pid, signal.SIGTERM)
    deadline = time.monotonic() + 15
    while pids(data) and time.monotonic() < deadline:
        time.sleep(.2)
    if pids(data):
        raise RuntimeError('owned app did not stop')


def run(scenario, data, socket, build=False):
    data.mkdir(mode=0o700, exist_ok=True)
    fixture = data / 'translation-fixture'
    fixture.mkdir(mode=0o700, exist_ok=True)
    (fixture / 'scenario').write_text(scenario)
    mode = 'translation' if scenario == 'setup-controlled' else 'translation-native'
    entry = {'scenario': scenario, 'data': str(data), 'socket': socket}
    REPORT['runs'].append(entry)
    if socket not in SOCKETS:
        SOCKETS.append(socket)
    before = (data / 'app.log').stat().st_size if (data / 'app.log').exists() else 0
    print(f'{scenario}: launching', flush=True)
    if build:
        env = dict(os.environ, DECK_SMOKE_DATA_DIR=str(data), DECK_SMOKE_TMUX_SOCKET=socket,
                   DECK_SMOKE_WKWEBVIEW=mode)
        code = command(['app/run.sh'], 'build.log', env=env, timeout=3600)
        if code:
            raise RuntimeError(f'build/launch failed: {code}')
        mismatch = [str(p) for p in (ROOT / 'app/ui').rglob('*') if p.is_file()
                    and (not (ROOT / 'app/src-tauri/ui-dist' / p.relative_to(ROOT / 'app/ui')).exists()
                         or p.read_bytes() != (ROOT / 'app/src-tauri/ui-dist' / p.relative_to(ROOT / 'app/ui')).read_bytes())]
        REPORT['embeddedFrontendMatches'] = not mismatch
        if mismatch:
            raise RuntimeError('embedded frontend mismatch')
        REPORT['binarySha256'] = hashlib.sha256((BUNDLE / 'Contents/MacOS/deck').read_bytes()).hexdigest()
        REPORT['sourceDiffSha256'] = hashlib.sha256(subprocess.check_output(['git', 'diff', 'HEAD'], cwd=ROOT)).hexdigest()
    else:
        subprocess.run(['open', '-n', str(BUNDLE), '--args', '--smoke-data-dir', str(data),
                        '--smoke-tmux-socket', socket, '--smoke-wkwebview', mode], check=True)
    deadline = time.monotonic() + 360
    while time.monotonic() < deadline:
        log = (data / 'app.log').read_bytes()[before:].decode(errors='replace') if (data / 'app.log').exists() else ''
        if '[ui] smoke-check done ' in log:
            break
        time.sleep(1)
    else:
        raise RuntimeError(f'{scenario} timed out')
    entry['pids'] = pids(data)
    entry['verdictExit'] = command(['node', '--input-type=module', '-e',
        "import {readFileSync} from 'node:fs'; import {judge} from './scripts/smoke-verdict.mjs'; "
        "const [file,mode]=process.argv.slice(1); "
        "const verdict=judge(readFileSync(file,'utf8'),JSON.parse(readFileSync('app/ui/test/fixtures/translation-setup-manifest.json','utf8')),mode); "
        "console.log(JSON.stringify(verdict,null,2)); process.exit(verdict.ok?0:1);",
        str(data / 'app.log'), scenario], scenario + '-verdict.log')
    (RUN / (scenario + '.log')).write_text(log)
    captures = RUN / (scenario + '-screenshots')
    captures.mkdir()
    for path in data.rglob('*.png'):
        shutil.copy2(path, captures / path.name)
    stop(data)
    print(f'{scenario}: verdict {entry["verdictExit"]}', flush=True)
    if entry['verdictExit']:
        raise RuntimeError(f'{scenario} failed; see evidence')


def main():
    print(RUN, flush=True)
    if command(['scripts/edr_runtime.py'], 'preflight.log'):
        raise RuntimeError('existing development/smoke resources; inspect before using shared test bundle')
    controlled, native = RUN / 'controlled', RUN / 'native'
    suffix = RUN.name.rsplit('-', 1)[-1]
    try:
        run('setup-controlled', controlled, 'deck-smoke-setup-' + suffix + '-c', build=True)
        run('setup-native-install', native, 'deck-smoke-setup-' + suffix + '-n')
        run('setup-restart-enabled', native, 'deck-smoke-setup-' + suffix + '-n')
        run('setup-restart-disabled', native, 'deck-smoke-setup-' + suffix + '-n')
        REPORT['verdict'] = 'PASS'
    except Exception as error:
        REPORT['error'] = str(error)
    finally:
        for data in (controlled, native):
            try:
                stop(data)
            except Exception as error:
                REPORT['cleanup'].append(str(error))
        for socket in SOCKETS:
            code = command(['scripts/edr_runtime.py', '--cleanup', '--socket', socket], socket + '-cleanup.log')
            REPORT['cleanup'].append({'socket': socket, 'exit': code})
            if code:
                REPORT['verdict'] = 'FAIL'
        REPORT['finalInventoryExit'] = command(['scripts/edr_runtime.py'], 'final-inventory.log')
        if REPORT['finalInventoryExit'] or any(isinstance(x, str) for x in REPORT['cleanup']):
            REPORT['verdict'] = 'FAIL'
        (RUN / 'report.json').write_text(json.dumps(REPORT, indent=2) + '\n')
    print(json.dumps(REPORT, indent=2), flush=True)
    return 0 if REPORT['verdict'] == 'PASS' else 1


if __name__ == '__main__':
    raise SystemExit(main())
