#!/usr/bin/env python3
"""Autonomous Reminder verification on isolated local or macmini GUI carriers.

Build locally; install only a manifest-owned dedicated test carrier through
LaunchServices; run three independent WKWebView trials with real shell exits
and real application restarts; retain evidence; clean exact owned resources.
Local system actions are coordinated with the connected agent's authorized
cua_repl driver; all receipts are checked against actual native callbacks.
OS notification clicks require an already-authorized public UI automation
channel. Missing access is BLOCKED, never replaced with injected callbacks.
Offline evaluation retains original A1–E6 subassertions and separates core,
platform, cleanup and shared-safety conclusions. Historical/duplicate/unrun
records never certify the current candidate. Exit 0 PASS, 1 FAIL, 2 BLOCKED. No user interaction or permission escalation.
"""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shlex
import shutil
import subprocess
import tempfile
import time
import uuid

ROOT = Path(__file__).resolve().parent.parent
TAURI = ROOT / 'app/src-tauri'
IDS = [f'{letter}{n}' for letter, count in [('A', 4), ('B', 5), ('C', 5), ('D', 5), ('E', 6)] for n in range(1, count + 1)]


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def source_digest():
    patch = subprocess.check_output(['git', 'diff', '--binary', 'HEAD'], cwd=ROOT)
    extra = subprocess.check_output(['git', 'ls-files', '--others', '--exclude-standard', '-z'], cwd=ROOT)
    for raw in sorted(extra.split(b'\0')):
        if raw:
            patch += raw + b'\0' + (ROOT / os.fsdecode(raw)).read_bytes()
    return hashlib.sha256(patch).hexdigest()


def run(cmd, timeout=300, log=None, cwd=ROOT):
    result = subprocess.run(cmd, cwd=cwd, capture_output=True, timeout=timeout)
    if log:
        Path(log).write_bytes(result.stdout + result.stderr)
    return result


def ssh(code, timeout=30):
    return run(['ssh', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=10', 'macmini', code], timeout=timeout)


def remote_python(code, timeout=30):
    # The alias owns host/auth details; never print them or shell-expand code.
    return ssh('python3 -c ' + shlex.quote(code), timeout)


def wait_until(check, seconds):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = check()
        if value:
            return value
        time.sleep(.5)
    raise TimeoutError('declared observation deadline expired')


def assert_delivered_after_exit(inventory, identifier, exited_at, deadline):
    matching = [row for row in inventory.get('delivered', []) if row.get('identifier') == identifier]
    if len(matching) != 1 or not exited_at <= matching[0].get('deliveredAt', 0) <= deadline:
        raise AssertionError('No unique real system delivery after verified process exit')
    return matching[0]



def console_state():
    """Read only public console lock facts, never account names or credentials."""
    observed = int(time.time() * 1000)
    try:
        value = plistlib.loads(subprocess.check_output(['ioreg','-a','-n','Root','-d','1']))
        facts = []
        def visit(item):
            if isinstance(item, dict):
                for key, child in item.items():
                    if key == 'IOConsoleUsers':
                        facts.extend({k:v for k,v in row.items() if k in ['CGSSessionScreenIsLocked','CGSSessionScreenLockedTime','kCGSSessionOnConsoleKey']} for row in child)
                    else: visit(child)
            elif isinstance(item, list):
                for child in item: visit(child)
        visit(value)
        active = [row for row in facts if row.get('kCGSSessionOnConsoleKey') is True]
        locked = active[0].get('CGSSessionScreenIsLocked') if len(active) == 1 else None
        return {'observedAt':observed,'locked':locked,'facts':facts}
    except (OSError, subprocess.SubprocessError, plistlib.InvalidFileException):
        return {'observedAt':observed,'locked':None,'facts':[]}


def macmini_main(args):
    evidence = args.evidence.resolve()
    evidence.mkdir(parents=True, exist_ok=True)
    os.chmod(evidence, 0o700)
    report = {'startingHEAD': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
              'branch': subprocess.check_output(['git', 'branch', '--show-current'], cwd=ROOT, text=True).strip(),
              'candidateDiffDigest': source_digest(), 'userInterventionCount': 0,
              'environmentReady': False, 'isolationVerified': False, 'productAssertionsPassed': False,
              'requiredTestsExecuted': False, 'cleanupCompleted': False, 'sharedResourcesSafe': True,
              'matrix': [{'id': key, 'status': 'NOT RUN', 'layers': [], 'evidence': []} for key in IDS],
              'trials': [], 'commands': [], 'blockers': [], 'verdict': 'BLOCKED'}
    work = Path(tempfile.mkdtemp(prefix='deck-reminder-build-'))
    owned = []
    bundle = work / 'Deck Reminder Test.app'
    failure = None
    def command(cmd, name, timeout=300):
        result = run(cmd, timeout=timeout, log=evidence / f'{name}.log')
        report['commands'].append({'command': cmd, 'exitCode': result.returncode, 'log': f'{name}.log'})
        if result.returncode:
            raise RuntimeError(f'{name} exited {result.returncode}')
        return result
    try:
        preflight = ssh("sw_vers; uname -m; stat -f %Su /dev/console; test -e /Applications/deck.app && echo INSTALL_PRESENT; test -e ~/.deck && echo DATA_PRESENT; true")
        platform_lines = preflight.stdout.decode(errors='replace').splitlines()
        platform_lines = [line if n != 4 else 'Active GUI login verified' for n, line in enumerate(platform_lines)]
        (evidence / 'remote-platform.txt').write_text('\n'.join(platform_lines))
        if preflight.returncode or b'arm64' not in preflight.stdout:
            raise RuntimeError('isolated Mac mini unavailable')
        probe = ssh("swift -e 'import AppKit; import ApplicationServices; import CoreGraphics; print(AXIsProcessTrusted()); print(CGEventSource.secondsSinceLastEventType(.combinedSessionState,eventType:.null)); print(NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? \"none\")'", 60)
        (evidence / 'remote-gui.txt').write_bytes(probe.stdout + probe.stderr)
        lines = probe.stdout.decode().splitlines()
        if probe.returncode or len(lines) < 2 or float(lines[1]) < 300:
            report['blockers'].append('Dedicated GUI unavailable or recently active; no focus takeover attempted')
            return 2
        report['wkEnvironmentReady'] = True
        report['driverProbes'] = [{'driver':'own-window AppKit','scope':'test application only','externalSystemActions':False}]
        report['notificationUIReady'] = any(probe.get('externalSystemActions') is True for probe in report['driverProbes'])
        report['accessibilityAuthorized'] = lines[0] == 'true'
        # SSH Swift is not an authorized system UI driver on this resource.
        # Own-window AppKit input does not grant Notification Center access.
        report['blockers'].append('No available already-authorized remote system UI automation channel; real notification clicks, cold-start actions and permission dialog cannot be safely operated')
        report['blockers'].append('Real sleep/wake not attempted without an exclusive recovery channel')
        command(['cargo', 'build', '--manifest-path', str(TAURI / 'Cargo.toml')], 'candidate-build', 600)
        contents = bundle / 'Contents'
        macos = contents / 'MacOS'; resources = contents / 'Resources'
        macos.mkdir(parents=True); resources.mkdir()
        shutil.copy2(TAURI / 'target/debug/deck-app', macos / 'deck')
        for name in ['tmux', 'deck-status-helper', 'deck-mcp', 'deck-mcp-runner']:
            source = TAURI / 'binaries' / f'{name}-aarch64-apple-darwin'
            if source.exists(): shutil.copy2(source, macos / name)
        version = json.loads((TAURI / 'tauri.conf.json').read_text())['version']
        # Find an existing authorized signing identity without recording it.
        identity = run(['security', 'find-identity', '-v', '-p', 'codesigning'])
        match = re.search(rb'\b([A-F0-9]{40})\b[^\n]*Developer ID Application', identity.stdout)
        if not match:
            report['blockers'].append('Existing Developer ID signing identity unavailable')
            return 2
        signing = match.group(1).decode()
        report['binaryIdentity'] = {'sha256': digest(macos / 'deck'), 'version': version}
        for index in range(3):
            run_id = uuid.uuid4().hex[:12]
            remote_root = f'/tmp/deck-reminder-{run_id}'
            app = f'{remote_root}/Deck Reminder Test.app'
            data = f'{remote_root}/data'
            socket = f'deck-smoke-reminder-{run_id}'
            bundle_id = f'io.c9r.deck.reminder.smoke.{run_id}'
            trial = {'runId': run_id, 'root': remote_root, 'data': data, 'bundle': app, 'bundleId': bundle_id, 'socket': socket,
                     'candidateDiffDigest': report['candidateDiffDigest'], 'binaryIdentity': report['binaryIdentity'],
                     'userInterventions': 0, 'testerRescueActions': 0, 'phases': [], 'verdict': 'NOT RUN', 'cleanup': {}}
            report['trials'].append(trial); owned.append(trial)
            (evidence / 'resource-manifest.json').write_text(json.dumps(owned, indent=2))
            with (contents / 'Info.plist').open('wb') as stream:
                plistlib.dump({'CFBundleIdentifier': bundle_id, 'CFBundleExecutable': 'deck', 'CFBundleName': 'Deck Reminder Test', 'CFBundlePackageType': 'APPL', 'CFBundleVersion': version, 'CFBundleShortVersionString': version, 'NSHighResolutionCapable': True, 'LSMinimumSystemVersion': '11.0'}, stream)
            (resources / 'deck-smoke-launch.json').write_text(json.dumps({'--smoke-data-dir': data, '--smoke-tmux-socket': socket, '--smoke-wkwebview': 'reminder'}))
            # Sign only test-owned copied artifacts. Identity text is not logged.
            for executable in macos.iterdir():
                signed = run(['codesign', '--force', '--sign', signing, str(executable)], timeout=45)
                if signed.returncode: raise RuntimeError('candidate sidecar signing failed')
            signed = run(['codesign', '--force', '--sign', signing, '--entitlements', str(TAURI / 'Entitlements.plist'), str(bundle)], timeout=45)
            if signed.returncode: raise RuntimeError('candidate bundle signing failed')
            trial['signedBinarySha256'] = digest(macos / 'deck')
            command(['ditto', '-c', '-k', '--keepParent', str(bundle), str(work / 'carrier.zip')], f'archive-{run_id}')
            if remote_python(f"import os; os.mkdir({remote_root!r},0o700); open({(remote_root+'/owner')!r},'w').write({run_id!r}); os.mkdir({data!r},0o700); os.mkdir({(data+'/home')!r},0o700); os.mkdir({(data+'/translation-fixture')!r},0o700)").returncode:
                raise RuntimeError('unique remote root could not be created')
            command(['scp', '-q', str(work / 'carrier.zip'), f'macmini:{remote_root}/carrier.zip'], f'transfer-{run_id}')
            if ssh(f'ditto -x -k {shlex.quote(remote_root+"/carrier.zip")} {shlex.quote(remote_root)}').returncode:
                raise RuntimeError('carrier extraction failed')
            verified_binary = remote_python(f"import hashlib; from pathlib import Path; print(hashlib.sha256(Path({(app+'/Contents/MacOS/deck')!r}).read_bytes()).hexdigest())")
            if verified_binary.stdout.decode().strip() != trial['signedBinarySha256']: raise RuntimeError('remote binary identity mismatch')
            trial['remoteBinaryIdentityVerified'] = True
            report['isolationVerified'] = None
            def read_log():
                result = remote_python(f"from pathlib import Path; p=Path({(data+'/app.log')!r}); print(p.read_text() if p.exists() else '')")
                return result.stdout.decode(errors='replace')
            def app_pids():
                # Read-only identity match, never process-name cleanup.
                result = remote_python(f"import subprocess,json,os; rows=subprocess.check_output(['ps','-axo','pid=,command='],text=True).splitlines(); print(json.dumps([int(r.strip().split(None,1)[0]) for r in rows if len(r.strip().split(None,1))==2 and os.path.realpath(r.strip().split(None,1)[1].split(' --',1)[0])==os.path.realpath({(app+'/Contents/MacOS/deck')!r})]))")
                return json.loads(result.stdout)
            for phase in ['reminder-setup', 'reminder-resume', 'reminder-retired']:
                remote_python(f"from pathlib import Path; Path({(data+'/translation-fixture/scenario')!r}).write_text({phase!r})")
                before = read_log().count('smoke-check reminder-phase-ready a=')
                started = datetime.datetime.now(datetime.timezone.utc).isoformat()
                result = ssh('open -n ' + shlex.quote(app))
                if result.returncode: raise RuntimeError('LaunchServices launch failed')
                expected = 'smoke-check done a=' if phase == 'reminder-retired' else 'smoke-check reminder-phase-ready a='
                def observed():
                    log = read_log()
                    (evidence / f'{run_id}-app.log').write_text(log)
                    if 'smoke-check reminder-exception a=' in log: raise RuntimeError('WKWebView product assertion failed')
                    return log if (expected in log and (phase == 'reminder-retired' or log.count(expected) > before)) else None
                log = wait_until(observed, 105)
                current_board = remote_python(f"import json; from pathlib import Path; value=json.loads(Path({(data+'/deck.json')!r}).read_text()); board=value.get('data',value); print(json.dumps([{{'cardId':c['id'],'session':c['session'],'reminder':{{k:v for k,v in c.get('reminder',{{}}).items() if k!='note'}},'retirements':c.get('reminderRetirements',[])}} for c in board['cards']]))")
                try: trial.setdefault('boardObservations', []).append({'phase':phase,'cards':json.loads(current_board.stdout),'observedAt':datetime.datetime.now(datetime.timezone.utc).isoformat()})
                except ValueError: raise RuntimeError('committed Board observation failed')
                pids = app_pids()
                trial['phases'].append({'phase': phase, 'startedAt': started, 'observedAt': datetime.datetime.now(datetime.timezone.utc).isoformat(), 'deadlineSeconds':105, 'pids':pids})
                (evidence / f'{run_id}-app.log').write_text(log)
                if phase != 'reminder-retired': wait_until(lambda: not app_pids(), 15)
            copy = run(['scp', '-q', '-r', f'macmini:{data}/evidence', str(evidence / run_id)], timeout=60)
            trial['snapshotCopyExit'] = copy.returncode
            local_data = evidence / run_id; local_data.mkdir(exist_ok=True)
            (local_data / 'app.log').write_text(read_log())
            verdict = command([str(ROOT / 'scripts/smoke-verdict'), str(local_data), 'reminder'], f'verdict-{run_id}')
            trial['verdict'] = 'WK PASS / NATIVE BLOCKED' if verdict.returncode == 0 else 'FAIL'
        report['verdict'] = 'BLOCKED'
    except (RuntimeError, subprocess.TimeoutExpired, TimeoutError, ValueError) as error:
        failure = str(error)
        report['failure'] = failure; report['verdict'] = 'FAIL'
    finally:
        # All notification content in these trials is explicitly in-app-only.
        # A successful carrier withdraws its entire dedicated UN namespace.
        for trial in owned:
            root = trial['root']; app = trial['bundle']; socket = trial['socket']
            evidence_dir = evidence / trial['runId']; evidence_dir.mkdir(exist_ok=True)
            run(['scp', '-q', '-r', f"macmini:{trial['data']}/evidence", str(evidence_dir)], timeout=60)
            saved_log = remote_python(f"from pathlib import Path; p=Path({(trial['data']+'/app.log')!r}); print(p.read_text() if p.exists() else '')")
            (evidence / f"{trial['runId']}-final-app.log").write_bytes(saved_log.stdout)
            # Before removing the root, stop only processes whose actual executable
            # equals this manifest's carrier or this manifest's bundled tmux.
            cleanup = remote_python(f"""import subprocess,os,json,signal,time,shutil
app={app!r}; root={root!r}; socket={socket!r}
from pathlib import Path
if not Path(root+'/owner').is_file() or Path(root+'/owner').read_text()!={trial["runId"]!r}:
 print(json.dumps({{'rootRemoved':not os.path.exists(root),'remainingOwnedProcesses':[],'unownedRootUntouched':True}})); raise SystemExit(0)
rows=subprocess.check_output(['ps','-axo','pid=,command='],text=True).splitlines()
pids=[]
for row in rows:
 parts=row.strip().split(None,1)
 if len(parts)==2 and os.path.realpath(parts[1].split(' --',1)[0])==os.path.realpath(app+'/Contents/MacOS/deck'): pids.append(int(parts[0]))
binary=app+'/Contents/MacOS/tmux'
server=subprocess.run([binary,'-L',socket,'display-message','-p','#{{pid}}'],capture_output=True,text=True) if os.path.exists(binary) else None
server_pid=int(server.stdout.strip()) if server and server.returncode==0 else None
parents=set(pids+([server_pid] if server_pid else [])); children=set()
processes=subprocess.check_output(['ps','-axo','pid=,ppid='],text=True).splitlines()
for _ in range(20):
 added=set()
 for row in processes:
  parts=row.split()
  if len(parts)==2 and int(parts[1]) in parents: added.add(int(parts[0]))
 added-=parents
 if not added: break
 children|=added; parents|=added
fingerprints={{pid:subprocess.run(['ps','-p',str(pid),'-o','lstart=','-o','comm='],capture_output=True,text=True).stdout.strip() for pid in children}}
for pid in pids:
 try: os.kill(pid,signal.SIGTERM)
 except ProcessLookupError: pass
# Use the exact unique socket and bundled binary; never the default server.
if server and server.returncode==0:
 subprocess.run([binary,'-L',socket,'kill-server'],capture_output=True)
time.sleep(.5)
remaining_children=[]
for pid,expected in fingerprints.items():
 current=subprocess.run(['ps','-p',str(pid),'-o','lstart=','-o','comm='],capture_output=True,text=True).stdout.strip()
 if current and current==expected:
  try: os.kill(pid,signal.SIGTERM)
  except ProcessLookupError: pass
time.sleep(.5)
for pid,expected in fingerprints.items():
 current=subprocess.run(['ps','-p',str(pid),'-o','lstart=','-o','comm='],capture_output=True,text=True).stdout.strip()
 if current and current==expected: remaining_children.append(pid)
active=[]
for row in subprocess.check_output(['ps','-axo','pid=,command='],text=True).splitlines():
 parts=row.strip().split(None,1)
 if len(parts)==2 and os.path.realpath(parts[1].split(' --',1)[0]).startswith(os.path.realpath(app+'/Contents/MacOS')+'/'): active.append(int(parts[0]))
reachable=subprocess.run([binary,'-L',socket,'display-message','-p','#{{pid}}'],capture_output=True).returncode==0 if os.path.exists(binary) else False
socket_path=Path('/tmp')/('tmux-'+str(os.getuid()))/socket
# A dead unique test server may leave its socket inode; ownership was verified
# by the root marker and its server PID before stopping it.
if not reachable and socket_path.exists(): socket_path.unlink()
socket_absent=not socket_path.exists()
if not active and not remaining_children and not reachable and socket_absent and os.path.isdir(root): shutil.rmtree(root)
print(json.dumps({{'appPidsStopped':pids,'remainingOwnedProcesses':active+remaining_children,'ownedChildPids':sorted(children),'serverPid':server_pid,'serverUnreachable':not reachable,'socketAbsent':socket_absent,'rootRemoved':not os.path.exists(root),'notifications':'explicit in-app-only trials; carrier inventory and withdrawal checked'}}))
""", 45)
            try: trial['cleanup'] = json.loads(cleanup.stdout)
            except (ValueError, UnicodeDecodeError): trial['cleanup'] = {'error':'cleanup response unavailable','exitCode':cleanup.returncode}
        report['cleanupCompleted'] = all(t['cleanup'].get('rootRemoved') and t['cleanup'].get('socketAbsent') and not t['cleanup'].get('remainingOwnedProcesses') for t in owned)
        shutil.rmtree(work)
        wk_passed = len(owned) == 3 and all(t['verdict'] == 'WK PASS / NATIVE BLOCKED' for t in owned)
        # WK logs remain evidence, but parent totals cannot certify permutations.
        report['readiness'] = {'ownWindow': wk_passed, 'systemUI': report.get('notificationUIReady')}
        if failure:
            report['matrix'].append({'id':'execution','status':'FAIL','executed':True,'evidence':[failure]})
        report['finalHEAD'] = subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip()
        report['candidateDiffDigestAfter'] = source_digest()
        if report['candidateDiffDigestAfter'] != report['candidateDiffDigest']:
            report['verdict']='FAIL'; report['failure']='Source changed during certification'
        report['runtimeCleanupObserved'] = report['cleanupCompleted']
        (evidence/'raw-runtime-report.json').write_text(json.dumps(report, indent=2))
        receipts = wk_evidence(report, evidence)
        report['assertionReceipts'] = receipts
        report.update(aggregate(receipts, report['candidateDiffDigest'], wk_resources(report, evidence), report['readiness']))
        if failure or report['candidateDiffDigestAfter'] != report['candidateDiffDigest']:
            report['verdict']='FAIL'; report['exitCode']=1
        (evidence / 'unattended-acceptance.json').write_text(json.dumps(report,indent=2))
        print(json.dumps({'verdict':report['verdict'],'report':str(evidence/'unattended-acceptance.json'),'cleanupCompleted':report['cleanupCompleted'],'userInterventionCount':0}))
    return report['exitCode']


def same_executable(command, executable):
    """Match a complete launch path, including spaces and /tmp normalization."""
    return os.path.realpath(command.split(' --', 1)[0]) == os.path.realpath(executable)


# Each original matrix row retains its required permutations. A named receipt
# covers only its own assertion and evidence layer, never the entire parent.
ASSERTIONS = {
    'A1': [('legacy', 'Legacy Board read/write', 'logic'), ('schema', 'Sticky/new/unknown schema and old-reader refusal', 'logic'), ('failures', 'Corrupt data and write refusal', 'logic')],
    'A2': [('entries', 'Both editor entries', 'wk'), ('shortcuts', 'Every shortcut choice', 'wk'), ('preview', 'Explicit date/weekday/timezone preview', 'wk'), ('validation', 'Invalid/past time and note limits', 'wk'), ('save', 'Editor saves committed Reminder', 'wk'), ('edit', 'Editor changes committed Reminder', 'wk'), ('cancel', 'Editor cancel leaves committed Reminder intact', 'wk')],
    'A3': [('single', 'One active reminder per card', 'logic'), ('move', 'Cross-project move preserves binding', 'wk'), ('rename', 'Rename preserves binding', 'wk'), ('identity', 'Same-name replacement session/card does not inherit', 'wk')],
    'A4': [('saved', 'Saved Reminder survives actual app restart', 'wk'), ('edited', 'Edited Reminder survives actual app restart', 'wk'), ('snoozed', 'System Snooze survives actual app restart', 'system'), ('time', 'Zone/DST/forward/backward time models', 'logic')],
    'B1': [('registration', 'Actual UN registration matches committed identity/time', 'native'), ('delivery', 'Actual UN due delivery', 'native'), ('visible', 'Actual system notification visible', 'system'), ('attention', 'Due card/Attention/Dock counting', 'wk')],
    'B2': [('deck', 'Other card foreground: visible arrival, no focus/card move', 'system'), ('sentinel', 'Owned other app foreground: visible arrival, no focus/card move', 'system')],
    'B3': [('view', 'Viewing and system dismissal do not end reminder', 'system'), ('repeat', 'Repeated poll/restart do not bombard', 'wk'), ('summary', 'Multiple missed reminders use in-app summary', 'wk')],
    'B4': [('reasons', 'Agent reason coexistence/transitions and episode independence', 'wk'), ('count', 'Independent reasons deduplicate card count', 'logic')],
    'B5': [('settings', 'notifyAway/hooks/sound combinations and wording', 'wk'), ('inapp', 'In-app-only withdraws actual system requests', 'native')],
    'C1': [('quit', 'Normal quit and verified absent process before due', 'native'), ('delivery', 'Unique actual due delivery with non-rearming observer', 'native')],
    'C2': [('click', 'Actual OS default click cold-starts through native callback', 'system'), ('board', 'Authoritative Board initialization focuses correct due card', 'system'), ('stopped', 'Reminder still pending and target still stopped', 'system'), ('noextra', 'No extra shell/Agent generation started by cold Open', 'system')],
    'C3': [('running', 'Actual running-app OS Snooze saves exactly action time + one hour', 'system'), ('cold', 'Actual stopped-app OS Snooze saves exactly action time + one hour', 'system'), ('restart', 'New time survives actual restart and old request withdrawn', 'system')],
    'C4': [('stopped', 'Stopped/renamed target system click cannot resume work', 'system'), ('stale', 'Deleted target and stale notification cannot open wrong session', 'system')],
    'C5': [('edit', 'Actual edit replaces old request identity/time', 'native'), ('snooze', 'Actual Snooze replaces old request identity/time', 'system'), ('end', 'Cancel/End withdraws actual pending/delivered requests', 'native'), ('agent', 'Actual Agent notification namespace unaffected', 'native')],
    'D1': [('shell', 'Real protected/unprotected ordinary shell exits', 'wk')],
    'D2': [('automation', 'Real Automation finish=close protected/unprotected controls', 'wk')],
    'D3': [('shell', 'Blocked shell exit remains blocked after End/poll/restart; new generation retires', 'wk'), ('automation', 'Blocked real Automation run remains blocked after End/poll/restart', 'wk')],
    'D4': [('doors', 'Manual/batch/project/remote close doors enforce protection', 'wk'), ('race', 'Changed Reminder during confirmation rejects stale claim', 'wk')],
    'D5': [('save', 'Actual failed End save preserves protection and requests', 'native'), ('native', 'Native close/cancel partial failures preserve recovery and report results', 'native')],
    'E1': [('action', 'Old/duplicate Snooze cannot overwrite or resurrect', 'logic'), ('race', 'Actual out-of-order add and concurrent edit/delete fences', 'native')],
    'E2': [('save', 'Actual failed save retains committed protection', 'native'), ('windows', 'Pre/post persist/register/withdraw/response crash windows converge without storm', 'native')],
    'E3': [('authorized', 'Actual authorized notification chain', 'native'), ('faults', 'Undecided/denied/read/add/unavailable branches preserve wording/protection', 'logic')],
    'E4': [('restart', 'Actual startup reconcile', 'wk'), ('rebuild', 'Real WebView rebuild and multiple overdue reconciliation', 'wk'), ('time', 'Forward/backward time reconciliation models', 'logic')],
    'E5': [('sleep', 'Real physical sleep/wake with recorded conditions', 'platform')],
    'E6': [('normal', 'Normal own UN/process/tmux/socket/root cleanup', 'cleanup'), ('assert', 'Actual assertion failure cleanup', 'cleanup'), ('timeout', 'Actual timeout cleanup', 'cleanup'), ('cancel', 'Actual cancellation cleanup', 'cleanup'), ('sigint', 'Actual external SIGINT cleanup', 'cleanup'), ('shared', 'Ownership/action audit and shared-resource safety', 'safety'), ('background', 'Final observation of OS-generated background notice', 'safety')],
}
# These native views are aliases, not extra receipts or extra executed tests.
NATIVE_ASSERTIONS = {
    'N1': ['B1.registration', 'B1.delivery', 'B1.visible', 'B1.attention', 'B2.deck', 'B2.sentinel'],
    'N2': ['C1.quit', 'C1.delivery'],
    'N3': ['C2.click', 'C2.board', 'C2.stopped', 'C2.noextra'],
    'N4': ['C3.running', 'C3.cold', 'C3.restart'],
    'N5': ['C5.edit', 'C5.snooze', 'C5.end', 'C5.agent'],
    'N6': ['D5.save', 'D5.native', 'E1.race', 'E2.save', 'E2.windows', 'E6.assert', 'E6.timeout', 'E6.cancel', 'E6.sigint'],
}
CRITICAL = {'B1.registration', 'B1.delivery', 'B1.visible', 'C1.quit', 'C1.delivery',
            'C2.click', 'C2.board', 'C2.stopped', 'C2.noextra', 'C3.running', 'C3.cold',
            'C3.restart', 'C5.edit', 'C5.end', 'D1.shell', 'D3.shell'}


def evidence_refs(paths):
    return [{'path': str(Path(path).resolve()), 'sha256': digest(path) if Path(path).is_file() else None} for path in paths]


def references_valid(refs):
    return bool(refs) and all(Path(ref.get('path', '')).is_file()
                              and digest(ref['path']) == ref.get('sha256') for ref in refs)


def receipt(assertion, trial, layer, paths, status='PASS'):
    return {'assertionId': assertion, 'candidateDiffDigest': trial['candidateDiffDigest'],
            'runId': trial['runId'], 'environmentId': trial.get('environmentId', 'local'),
            'binaryIdentityVerified': bool(trial.get('compiledBinarySha256')) and bool(trial.get('testedBinarySha256'))
            and trial.get('testedBinarySha256') == trial.get('signedBinarySha256'),
            'compiledBinarySha256': trial.get('compiledBinarySha256'),
            'signedBinarySha256': trial.get('signedBinarySha256'), 'testedBinarySha256': trial.get('testedBinarySha256'),
            'layer': layer, 'status': status, 'executed': True, 'evidence': evidence_refs(paths)}


def row_result(items):
    statuses = [item['status'] for item in items]
    status = ('FAIL' if 'FAIL' in statuses else 'PASS' if statuses and all(s == 'PASS' for s in statuses)
              else 'PARTIAL PASS' if any(s in ['PASS', 'PARTIAL PASS'] for s in statuses)
              else 'BLOCKED' if 'BLOCKED' in statuses else 'NOT RUN')
    return {'status': status, 'executed': all(item.get('executed') is True for item in items),
            'partiallyExecuted': any(item.get('executed') is True for item in items),
            'remainingAssertions': [item['id'] for item in items if item['status'] != 'PASS']}


def aggregate(receipts, candidate, resources, readiness=None):
    """Evaluate Reminder evidence only; logical fixtures never certify OS runs.

    Resource checks are independently evidenced tri-state observations. Cleanup
    cannot supply isolation or shared safety. Critical repetitions must belong
    to the same three independent run IDs in one environment, not a stitched run.
    """
    specs = {f'{parent}.{suffix}': (label, layer) for parent, assertions in ASSERTIONS.items()
             for suffix, label, layer in assertions}
    accepted = []; excluded = []
    for item in receipts:
        reason = None
        if item.get('assertionId') not in specs: reason = 'unknown assertion'
        elif item.get('candidateDiffDigest') != candidate: reason = 'historical/different candidate'
        elif item.get('executed') is not True: reason = 'not executed'
        elif item.get('layer') != 'logic' and (item.get('binaryIdentityVerified') is not True
                or not item.get('compiledBinarySha256') or not item.get('testedBinarySha256')
                or item.get('testedBinarySha256') != item.get('signedBinarySha256')): reason = 'unknown/mismatched binary identity'
        elif item.get('layer') != specs[item['assertionId']][1]: reason = 'wrong evidence layer'
        elif not item.get('runId') or not item.get('environmentId'): reason = 'missing run/environment identity'
        elif not references_valid(item.get('evidence')): reason = 'missing/changed evidence file'
        elif item.get('status') not in ['PASS', 'FAIL', 'BLOCKED', 'PARTIAL PASS']: reason = 'unsupported receipt status'
        if reason: excluded.append({'receipt': item, 'reason': reason})
        else: accepted.append(item)
    critical_builds = {}
    for item in accepted:
        if item['assertionId'] in CRITICAL:
            critical_builds.setdefault(item['environmentId'], set()).add(item['compiledBinarySha256'])
    identity_failure = any(len(builds) > 1 for builds in critical_builds.values())
    results = []
    for key, (label, layer) in specs.items():
        found = [item for item in accepted if item['assertionId'] == key]
        unique = {(item['environmentId'], item['runId']) for item in found if item['status'] == 'PASS'}
        required = 3 if key in CRITICAL else 1
        enough = any(len({run for env, run in unique if env == environment}) >= required for environment, _ in unique)
        status = ('FAIL' if any(item['status'] == 'FAIL' for item in found) else 'PASS' if enough
                  else 'PARTIAL PASS' if unique or any(item['status'] == 'PARTIAL PASS' for item in found) else 'BLOCKED' if any(item['status'] == 'BLOCKED' for item in found) else 'NOT RUN')
        results.append({'id': key, 'description': label, 'layer': layer, 'status': status,
                        'executed': bool(found), 'requiredIndependentRuns': required,
                        'observedDistinctRuns': len(unique), 'evidence': found})
    by_id = {item['id']: item for item in results}
    # Intersect complete chains; separate WK/native runs are never combined.
    chains = None
    for key in CRITICAL:
        identities = {(item['environmentId'], item['runId']) for item in accepted
                      if item['assertionId'] == key and item['status'] == 'PASS'}
        chains = identities if chains is None else chains & identities
    complete_chains = sorted(chains or set())
    three_chains = any(sum(env == environment for env, _ in complete_chains) >= 3 for environment, _ in complete_chains)
    resource_results = {}
    for name in ['isolation', 'cleanup', 'sharedSafety']:
        proof = resources.get(name, {})
        verified = proof.get('candidateDiffDigest') == candidate and references_valid(proof.get('evidence'))
        value = proof.get('value') if verified else None
        resource_results[name] = {'status': 'PASS' if value is True else 'FAIL' if value is False else 'UNKNOWN',
                                  'value': value, 'evidence': proof.get('evidence', []), 'scope': proof.get('scope', 'unspecified')}
    scopes = {}
    for name, layers in [('core', {'logic', 'wk', 'native', 'system'}), ('platform', {'platform'}),
                         ('cleanup', {'cleanup'}), ('sharedSafety', {'safety'})]:
        selected = [item for item in results if item['layer'] in layers]
        state = row_result(selected)
        failed = state['status'] == 'FAIL'
        passed = state['status'] == 'PASS'
        if name == 'core':
            failed |= resource_results['isolation']['value'] is False or identity_failure
            passed &= three_chains and resource_results['isolation']['value'] is True
        if name in ['cleanup', 'sharedSafety']:
            failed |= resource_results[name]['value'] is False
            passed &= resource_results[name]['value'] is True
        state['verdict'] = 'FAIL' if failed else 'PASS' if passed else 'BLOCKED'
        scopes[name] = state
    failed = any(scope['verdict'] == 'FAIL' for scope in scopes.values())
    verdict = 'FAIL' if failed else 'PASS' if all(scope['verdict'] == 'PASS' for scope in scopes.values()) else 'BLOCKED'
    matrix = [dict(id=parent, **row_result([by_id[f'{parent}.{suffix}'] for suffix, _, _ in assertions]),
                   assertions=[by_id[f'{parent}.{suffix}'] for suffix, _, _ in assertions]) for parent, assertions in ASSERTIONS.items()]
    native = [dict(id=key, **row_result([by_id[i] for i in ids]), assertions=ids) for key, ids in NATIVE_ASSERTIONS.items()]
    core_ready = {k: v for k, v in (readiness or {}).items() if k != 'safeSleep'}
    return {'verdict': verdict, 'exitCode': {'PASS': 0, 'FAIL': 1, 'BLOCKED': 2}[verdict],
            'scopes': scopes, 'matrix': matrix, 'nativeMatrix': native, 'resources': resource_results,
            'readiness': {'core': core_ready, 'platform': {'safeSleep': (readiness or {}).get('safeSleep')}},
            'environmentReady': bool(core_ready) and all(value is True for value in core_ready.values()),
            'isolationVerified': resource_results['isolation']['value'],
            'cleanupCompleted': resource_results['cleanup']['value'], 'sharedResourcesSafe': resource_results['sharedSafety']['value'],
            'productAssertionsPassed': scopes['core']['verdict'] == 'PASS',
            'requiredTestsExecuted': all(item['executed'] for item in results),
            'completeCriticalChains': complete_chains, 'criticalBuildIdentityFailure': identity_failure, 'excludedReceipts': excluded}


def local_evidence(report, evidence):
    """Import only explicit final-candidate facts backed by saved observations.

    This conservative adapter does not promote aggregate gate totals, old parent
    verdicts or a carrier's existence. Unrecorded permutations stay NOT RUN.
    """
    receipts = []; isolation_checks = []; cleanup_checks = []
    for trial in report.get('trials', []):
        folder = evidence / trial['runId']; facts = trial.get('facts', {})
        snapshots = list(folder.glob('*.json'))
        def add(key, paths, status='PASS'):
            layer = next(layer for suffix, _, layer in ASSERTIONS[key.split('.')[0]] if suffix == key.split('.')[1])
            receipts.append(receipt(key, trial, layer, paths, status))
        def snapshot(name):
            path = folder / (name + '.json')
            return path, json.loads(path.read_text()) if path.is_file() else {}
        initial_path, initial = snapshot('initial-registration')
        if initial.get('inventory', {}).get('pending'):
            add('B1.registration', [initial_path, folder/'resource-manifest.json'])
            if initial['inventory'].get('authorization') in [3, 4]: add('E3.authorized', [initial_path])
        after = facts.get('afterQuitDelivery')
        if after and after.get('observerDidNotLoadBoard') is True:
            assert_delivered_after_exit({'delivered': [after['delivered']]}, after['delivered']['identifier'], after['exitedAt'], after['dueAt'] + 45000)
            phases = trial.get('phases', [])
            if any(p.get('normalExitConfirmedAt') == after['exitedAt'] and p.get('remainingOwnedPids') == [] for p in phases):
                paths = [folder/'resource-manifest.json', folder/'quit-delivery-inventory.json']
                add('C1.quit', paths); add('C1.delivery', paths); add('B1.delivery', paths)
        for key, flag, names in [
            ('C5.edit', 'editedNativeRequestReplaced', ['initial-registration', 'edited-system-registration']),
            ('A2.edit', 'editedNativeRequestReplaced', ['initial-registration', 'edited-system-registration']),
            ('D5.save', 'saveFailurePreservesProtection', ['failed-end-still-protected']),
            ('E2.save', 'saveFailurePreservesProtection', ['failed-end-still-protected', 'native-observe']),
            ('B5.inapp', 'inAppOnlyWithdrawsSystemRequests', ['native-in-app-inventory']),
            ('C5.end', 'endedRestartRetained', ['native-end-inventory', 'native-observe'])]:
            if facts.get(flag) is True:
                paths = [snapshot(name)[0] for name in names]
                if all(path.is_file() for path in paths): add(key, paths)
        # A manifest flag alone cannot substitute for the actual system receipt.
        system = [('B1.visible', 'foregroundSystemVisibility'), ('C2.click', 'defaultClickColdStart'),
                  ('C2.board', 'defaultClickColdStart'), ('C2.stopped', 'defaultClickColdStart'),
                  ('C3.running', 'runningSnooze'), ('C3.cold', 'coldSnooze')]
        audit_path = folder/'native-evidence/reminder-native-actions.json'
        audit = json.loads(audit_path.read_text()) if audit_path.is_file() else []
        for key, flag in system:
            fact = facts.get(flag)
            if not fact: continue
            if fact == 'BLOCKED':
                add(key, [folder/'resource-manifest.json'], 'BLOCKED'); continue
            kind = 'observe-visible' if key == 'B1.visible' else 'default-click' if key.startswith('C2.') else 'snooze'
            matching_ui = []
            for path in folder.glob('ui-*.json'):
                ui = json.loads(path.read_text()); request = ui.get('request', {}); response = ui.get('response', {})
                if (request.get('runId') == trial['runId'] and request.get('kind') == kind
                        and response.get('id') == request.get('id') and response.get('status') == 'PASS'
                        and response.get('driver') == 'cua_repl' and response.get('actor') == 'codex' and response.get('observation')
                        and (not isinstance(fact, dict) or request.get('identifier') == fact.get('request'))): matching_ui.append(path)
            if not matching_ui: continue
            paths = matching_ui + [folder/'resource-manifest.json']
            if isinstance(fact, dict):
                if sum(item == fact for item in audit) != 1: continue
                paths.append(audit_path)
                if key.startswith('C3.'):
                    registration = 'running-snooze-registration' if key == 'C3.running' else 'cold-snooze-persisted'
                    path, value = snapshot(registration)
                    cards = [c for c in value.get('cards', []) if c.get('id') == fact.get('cardId')]
                    if not cards or cards[0].get('reminder', {}).get('dueAt') != fact.get('actedAt', 0) + 3600000: continue
                    paths.append(path)
                if key in ['C2.board', 'C2.stopped']:
                    log_path = folder/'app.log'
                    if not log_path.is_file() or 'reminder-native-observation a=15' not in log_path.read_text(): continue
                    paths.append(log_path)
            add(key, paths)
        observations = [json.loads(path.read_text()).get('inventory', {}) for path in snapshots]
        private = [item['privateEnvironment'] for item in observations if 'privateEnvironment' in item]
        root = trial.get('root', ''); data = trial.get('data', '')
        manifest_ok = root.startswith('/tmp/deck-reminder-local-') and data == root + '/data' and trial.get('socket') == 'deck-smoke-reminder-' + trial['runId']
        checks = bool(private) and all(all(item.get(k) is True for k in ['home', 'claude', 'codex']) for item in private)
        known_identity = bool(trial.get('testedBinarySha256')) and bool(trial.get('signedBinarySha256')) and bool(trial.get('compiledBinarySha256'))
        isolation_value = (manifest_ok and checks and receipt('C1.quit', trial, 'native', [])['binaryIdentityVerified']) if private and known_identity else None
        isolation_checks.append((isolation_value, snapshots))
        cleanup = trial.get('cleanup') or {}
        clean_ok = (cleanup.get('notificationsVerified') is True and cleanup.get('inventory', {}).get('pending') == []
                    and cleanup.get('inventory', {}).get('delivered') == [] and cleanup.get('remainingOwnedPids') == []
                    and all(cleanup.get(k) is True for k in ['serverUnreachable', 'socketAbsent', 'installationRemoved', 'rootRemoved']))
        cleanup_checks.append((clean_ok if cleanup else None, [folder/'resource-manifest.json', folder/'cleanup-inventory.json']))
        if clean_ok: add('E6.normal', cleanup_checks[-1][1])
    def proof(checks, scope):
        return {'value': (False if any(value is False for value, _ in checks) else True if checks and all(value is True for value, _ in checks) else None),
                'candidateDiffDigest': report['candidateDiffDigest'], 'scope': scope,
                'evidence': evidence_refs([path for _, paths in checks for path in paths])}
    resources = {'isolation': proof(isolation_checks, 'Recorded local carrier manifest and actual private HOME/Agent runtime observations'),
                 'cleanup': proof(cleanup_checks, 'Recorded test-owned runtime/UN cleanup; excludes OS background notice'),
                 'sharedSafety': {'value': None, 'scope': 'Shared resources and unobserved OS-generated background notice; separate action audit required'}}
    return receipts, resources



def wk_evidence(report, evidence):
    """Named real WK checks prove their stated subset, not all permutations."""
    receipts = []
    checks = {
        'A2.entries': ['reminder-ui-save', 'reminder-header'],
        'A2.preview': ['reminder-preview'],
        'A2.validation': ['reminder-date-reject', 'reminder-note-reject'],
        'A2.save': ['reminder-ui-save', 'reminder-durable'],
        'A2.cancel': ['reminder-header', 'reminder-restart-saved'],
        'A4.saved': ['reminder-restart-saved'],
        'A3.rename': ['reminder-header', 'reminder-restart-saved'],
        'B1.attention': ['reminder-real-due'],
        'D1.shell': ['reminder-real-exit', 'reminder-new-lifecycle'],
        'D3.shell': ['reminder-no-backlog', 'reminder-restart-held', 'reminder-new-lifecycle'],
        'E4.restart': ['reminder-restart-saved', 'reminder-restart-held'],
    }
    for original in report.get('trials', []):
        trial = dict(original, environmentId='macmini',
                     compiledBinarySha256=original.get('binaryIdentity', {}).get('sha256'),
                     testedBinarySha256=original.get('signedBinarySha256') if original.get('remoteBinaryIdentityVerified') is True else None)
        log_path = evidence / original['runId'] / 'app.log'
        if not log_path.is_file(): continue
        log = log_path.read_text()
        for assertion, names in checks.items():
            matches = {name: re.findall(r'smoke-check '+name+r' a=(-?\d+)', log) for name in names}
            if all(matches.values()):
                status = 'FAIL' if any('-1' in values for values in matches.values()) else 'PASS'
                item = receipt(assertion, trial, 'wk', [log_path, evidence/('unattended-acceptance.json' if (evidence/'unattended-acceptance.json').is_file() else 'raw-runtime-report.json')], status)
                if status == 'PASS' and assertion in ['A2.preview', 'A2.validation']:
                    item['status'] = 'PARTIAL PASS'
                    item['coveredSubset'] = ('Timezone string in preview; full date/weekday preview still unasserted' if assertion == 'A2.preview' else 'Past date and 281-character note rejection; other invalid/boundary permutations unasserted')
                receipts.append(item)
    return receipts



def logic_evidence(report, evidence):
    """Use exact named historical gate results; total test counts prove no item."""
    candidate = report.get('candidate', {}).get('candidateDiffDigest')
    if not candidate: return []
    checks = {
        'A1.schema': ('ac00-workspace-gate.log', [
            'storage::tests::reminder_board_requires_sticky_v5_and_old_readers_refuse_protection ... ok',
            'storage::tests::newer_schema_is_refused_untouched_and_save_wont_overwrite ... ok']),
        'A4.time': ('ac00-ui-gate.log', [
            '✔ time round trips reject gaps, invalid dates and ambiguous implicit choices',
            '✔ due is independent of viewing, agent episodes and host clock rollback']),
        'B4.count': ('ac00-ui-gate.log', ['✔ due is independent of viewing, agent episodes and host clock rollback']),
        'E1.action': ('ac00-ui-gate.log', ['✔ old and duplicate system actions cannot overwrite new reminders or resurrect cancelled cards']),
        'E4.time': ('ac00-ui-gate.log', ['✔ due is independent of viewing, agent episodes and host clock rollback']),
    }
    receipts = []
    for assertion, (name, matches) in checks.items():
        path = evidence/name
        gate = next((g for g in report.get('gates', []) if g.get('evidence') == name), {})
        if path.is_file() and gate.get('exitCode') == 0 and all(match in path.read_text() for match in matches):
            trial = {'candidateDiffDigest': candidate, 'runId': name, 'environmentId': 'logic'}
            receipts.append(receipt(assertion, trial, 'logic', [path, evidence/'unattended-acceptance.json']))
    return receipts



def wk_resources(report, evidence):
    """Remote isolation needs a manifest, verified carrier and runtime checks."""
    checks = []; paths = []
    for trial in report.get('trials', []):
        path = evidence/trial['runId']/'reminder-system-inventory.json'
        if not path.is_file(): checks.append(None); continue
        inventory = json.loads(path.read_text())
        private = inventory.get('privateEnvironment')
        paths.append(path)
        root = trial.get('root', '')
        known = private is not None and trial.get('remoteBinaryIdentityVerified') is not None
        checks.append((root == '/tmp/deck-reminder-'+trial['runId'] and trial.get('data') == root+'/data'
                       and trial.get('socket') == 'deck-smoke-reminder-'+trial['runId']
                       and trial.get('remoteBinaryIdentityVerified') is True
                       and all(private.get(key) is True for key in ['home', 'claude', 'codex'])) if known else None)
    source = evidence/('unattended-acceptance.json' if (evidence/'unattended-acceptance.json').is_file() else 'raw-runtime-report.json')
    paths.append(source)
    def value(items):
        return False if any(item is False for item in items) else True if items and all(item is True for item in items) else None
    clean = [t.get('cleanup', {}) for t in report.get('trials', [])]
    clean_value = value([all(c.get(k) is True for k in ['rootRemoved', 'socketAbsent', 'serverUnreachable'])
                         and c.get('remainingOwnedProcesses') == [] if c else None for c in clean])
    return {'isolation': {'value': value(checks), 'candidateDiffDigest': report['candidateDiffDigest'],
                          'scope': 'Remote per-run manifest, actual transferred signed identity and private runtime environment', 'evidence': evidence_refs(paths)},
            'cleanup': {'value': clean_value, 'candidateDiffDigest': report['candidateDiffDigest'],
                        'scope': 'Recorded remote owned runtime cleanup; UN emptiness separately checked by named WK events', 'evidence': evidence_refs(paths)},
            'sharedSafety': {'value': None, 'scope': 'Shared-resource safety requires separate action/observation audit'}}


def cleanup_control_evidence(report, evidence):
    """Intentional CLI failures retain raw FAIL and only certify cleanup scope."""
    receipts = []
    for control in report.get('negativeCleanupControls', []):
        path = Path(control['evidence'])
        if not path.is_file(): continue
        raw = json.loads(path.read_text())
        candidate = report['candidate']['candidateDiffDigest']
        if raw.get('candidateDiffDigest') != candidate or control.get('candidateDiffDigest') != candidate: continue
        if control.get('exitCode') != 1 or control.get('realPendingArmed') is not True: continue
        if control.get('negativeControlVerdict') != 'PASS' or raw.get('frozenBuild', {}).get('removed') is not True: continue
        trials = raw.get('trials', [])
        if len(trials) != 1: continue
        trial = trials[0]; cleanup = trial.get('cleanup') or {}
        initial = path.parent/trial['runId']/'initial-registration.json'
        actual_cleanup = path.parent/trial['runId']/'cleanup-inventory.json'
        if not initial.is_file() or not actual_cleanup.is_file(): continue
        pending = json.loads(initial.read_text()).get('inventory', {}).get('pending')
        inventory = json.loads(actual_cleanup.read_text())
        if not pending or inventory.get('pending') != [] or inventory.get('delivered') != []: continue
        if not (cleanup.get('notificationsVerified') is True and cleanup.get('remainingOwnedPids') == []
                and all(cleanup.get(k) is True for k in ['rootRemoved', 'socketAbsent', 'serverUnreachable', 'installationRemoved'])): continue
        mode = control['mode']
        expected = {'assert': 'deliberate assertion cleanup control', 'timeout': 'declared observation deadline expired',
                    'cancel': 'deliberate cancellation cleanup control', 'sigint': ''}
        if mode not in expected or raw.get('failure') != expected[mode]: continue
        if mode == 'sigint' and not (control.get('verifierPID') and control.get('signalAt')): continue
        receipts.append(receipt('E6.'+mode, trial, 'cleanup', [path, initial, actual_cleanup, evidence/'unattended-acceptance.json']))
    return receipts


def saved_evidence(report, evidence):
    """Read the existing closure layout without stitching runtime generations."""
    if report.get('target') == 'local': return local_evidence(report, evidence)
    if 'candidate' not in report: return wk_evidence(report, evidence), wk_resources(report, evidence)
    receipts = logic_evidence(report, evidence) + cleanup_control_evidence(report, evidence)
    resources = {}; observations = {}
    for name in ['locked-native-final', 'locked-wk-final']:
        path = evidence/name/'unattended-acceptance.json'
        if not path.is_file(): continue
        raw = json.loads(path.read_text())
        if raw.get('candidateDiffDigest') != report['candidate']['candidateDiffDigest']: continue
        imported, proof = saved_evidence(raw, path.parent)
        receipts.extend(imported); observations[name] = proof
    for key in ['isolation', 'cleanup']:
        proofs = [observation[key] for observation in observations.values()]
        values = [proof.get('value') for proof in proofs]
        value = (False if any(v is False for v in values) else True if len(values) == 2 and all(v is True for v in values) else None)
        resources[key] = {'value': value, 'candidateDiffDigest': report['candidate']['candidateDiffDigest'],
                          'scope': 'Recorded native and WK resources, individually scoped; not a joined E2E',
                          'evidence': [ref for proof in proofs for ref in proof.get('evidence', [])]}
    resources['sharedSafety'] = {'value': None, 'scope': 'OS background notice not finally observed; no complete shared-resource conclusion',
                                 'candidateDiffDigest': report['candidate']['candidateDiffDigest'],
                                 'evidence': evidence_refs([evidence/'independent-local-resources.json', evidence/'independent-remote-resources.json', evidence/'ui-preflight-actions.json'])}
    return receipts, resources


def evaluate_saved(args):
    """Read existing evidence without launching apps, modifying it or using UI."""
    if (args.evidence/'unattended-acceptance.json').exists():
        raise ValueError('Refusing to overwrite an existing offline report')
    source = args.evaluate_saved.resolve()
    report = json.loads(source.read_text())
    receipts, resources = saved_evidence(report, source.parent)
    candidate = report.get('candidateDiffDigest') or report['candidate']['candidateDiffDigest']
    result = aggregate(receipts, candidate, resources, report.get('readiness'))
    result.update({'evaluatedReport': str(source), 'evaluatedReportSha256': digest(source),
                   'testedCandidateDiffDigest': candidate, 'evaluatorDiffDigest': source_digest(),
                   'noNewProductExecution': True, 'originalVerdict': report['verdict'],
                   'originalTrials': report.get('trials', []), 'originalFailure': report.get('failure'),
                   'blockers': report.get('blockers', []), 'originalMatrix': report.get('matrix'),
                   'originalNativeMatrix': report.get('nativeMatrix'),
                   'historicalTrials': report.get('historicalTrials', []),
                   'cleanupUnknowns': report.get('cleanupUnknowns', []),
                   'assertionReceipts': receipts})
    # Preserve a valid execution failure even when no assertion was completed.
    if report.get('verdict') == 'FAIL':
        result['verdict'] = 'FAIL'; result['exitCode'] = 1
    args.evidence.mkdir(parents=True, exist_ok=True)
    (args.evidence/'unattended-acceptance.json').write_text(json.dumps(result, indent=2))
    print(json.dumps({'verdict': result['verdict'], 'report': str(args.evidence/'unattended-acceptance.json')}))
    return result['exitCode']


def binary_identity_matches(trials, compiled_sha):
    """Every trial must use the frozen build and its actually launched signature."""
    return bool(trials) and bool(compiled_sha) and all(
        trial.get('compiledBinarySha256') == compiled_sha
        and bool(trial.get('testedBinarySha256'))
        and trial['testedBinarySha256'] == trial.get('signedBinarySha256')
        for trial in trials)


class LocalCarrier:
    """Own one signed local test identity and its public-system-UI requests."""
    def __init__(self, evidence, signing, binary, index, actions, ui_timeout=90, inventory_only=False):
        self.evidence = evidence
        self.actions = actions
        self.ui_timeout = ui_timeout
        self.inventory_only = inventory_only
        self.signing = signing
        self.sentinel = None
        self.app = Path('/Applications/Deck Reminder Smoke Acceptance.app')
        if self.app.exists():
            raise RuntimeError('Dedicated test installation occupied; untouched')
        self.run_id = uuid.uuid4().hex[:12]
        self.root = Path('/tmp') / ('deck-reminder-local-' + self.run_id)
        self.root.mkdir(mode=0o700)
        (self.root / 'owner').write_text(self.run_id)
        self.data = self.root / 'data'
        for child in ['home', 'translation-fixture', 'evidence']:
            (self.data / child).mkdir(parents=True, mode=0o700)
        self.bundle_id = 'io.c9r.deck.reminder.smoke.local'
        self.socket = 'deck-smoke-reminder-' + self.run_id
        mac = self.app / 'Contents/MacOS'; resources = self.app / 'Contents/Resources'
        try:
            mac.mkdir(parents=True); resources.mkdir()
            (resources / 'deck-smoke-launch.json').write_text(json.dumps({'--smoke-data-dir':str(self.data),'--smoke-tmux-socket':self.socket,'--smoke-wkwebview':'reminder-native'}))
            (resources / 'reminder-smoke-owner').write_text(self.run_id)
            shutil.copy2(binary, mac / 'deck')
            for name in ['tmux','deck-status-helper','deck-mcp','deck-mcp-runner']:
                shutil.copy2(TAURI / 'binaries' / (name+'-aarch64-apple-darwin'), mac / name)
            version = json.loads((TAURI / 'tauri.conf.json').read_text())['version']
            with (self.app / 'Contents/Info.plist').open('wb') as stream:
                plistlib.dump({'CFBundleIdentifier':self.bundle_id,'CFBundleExecutable':'deck','CFBundleName':'Deck Reminder Smoke','CFBundleDisplayName':'Deck Reminder Smoke','CFBundlePackageType':'APPL','CFBundleVersion':version,'CFBundleShortVersionString':version,'NSHighResolutionCapable':True,'LSUIElement':False,'LSBackgroundOnly':False,'NSPrincipalClass':'NSApplication','LSMinimumSystemVersion':'11.0'},stream)
            for executable in mac.iterdir():
                if run(['codesign','--force','--sign',signing,str(executable)],timeout=45).returncode:
                    raise RuntimeError('test executable signing failed')
            if run(['codesign','--force','--sign',signing,'--entitlements',str(TAURI/'Entitlements.plist'),str(self.app)],timeout=45).returncode:
                raise RuntimeError('test bundle signing failed')
            registration=run(['/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister','-f',str(self.app)])
            if registration.returncode: raise RuntimeError('Dedicated app registration failed')
            self.manifest = {'runId':self.run_id,'index':index,'root':str(self.root),'data':str(self.data),'bundle':str(self.app),'bundleId':self.bundle_id,'socket':self.socket,'signedBinarySha256':digest(mac/'deck'),'compiledBinarySha256':digest(binary),'candidateDiffDigest':source_digest(),'phases':[],'facts':{},'cleanup':None}
            self.out = evidence / self.run_id; self.out.mkdir()
            self.save()
        except (OSError, RuntimeError, subprocess.SubprocessError):
            # No process or UN request has been created at construction time.
            marker = self.app / 'Contents/Resources/reminder-smoke-owner'
            if marker.exists() and marker.read_text() == self.run_id:
                shutil.rmtree(self.app)
            if (self.root / 'owner').read_text() == self.run_id:
                shutil.rmtree(self.root)
            raise

    def start_sentinel(self):
        self.sentinel = self.root / 'Reminder Sentinel.app'
        mac = self.sentinel / 'Contents/MacOS'; mac.mkdir(parents=True)
        with (self.sentinel / 'Contents/Info.plist').open('wb') as stream:
            plistlib.dump({'CFBundleIdentifier':'io.c9r.deck.reminder.smoke.sentinel','CFBundleExecutable':'sentinel','CFBundleName':'Deck Reminder Test Sentinel','CFBundlePackageType':'APPL'},stream)
        result = run(['swiftc',str(ROOT/'scripts/reminder-sentinel.swift'),'-o',str(mac/'sentinel')],timeout=60,log=self.out/'sentinel-build.log')
        if result.returncode or run(['codesign','--force','--sign',self.signing,str(self.sentinel)]).returncode: raise RuntimeError('Owned sentinel build failed')
        run(['open','-n',str(self.sentinel),'--args',str(self.data/'evidence/sentinel.json')])
        samples = wait_until(lambda:self.sentinel_samples(),15)
        self.manifest['sentinel']={'bundle':str(self.sentinel),'binarySha256':digest(mac/'sentinel'),'pid':samples[-1]['pid']}
        self.save()

    def sentinel_samples(self):
        path=self.data/'evidence/sentinel.json'
        return json.loads(path.read_text()) if path.exists() else []

    def save(self):
        (self.out / 'resource-manifest.json').write_text(json.dumps(self.manifest,indent=2))

    def pids(self):
        result=[]
        for row in subprocess.check_output(['ps','-axo','pid=,command='],text=True).splitlines():
            parts=row.strip().split(None,1)
            if len(parts)==2 and same_executable(parts[1],str(self.app/'Contents/MacOS/deck')):
                result.append(int(parts[0]))
        return result

    def scenario(self, value):
        (self.data/'translation-fixture/scenario').write_text(value)

    def launch(self, mode='native-idle'):
        if self.pids(): raise RuntimeError('Launch must begin with no owned app process')
        self.scenario(mode)
        actual_digest = digest(self.app/'Contents/MacOS/deck')
        if actual_digest != self.manifest['signedBinarySha256']: raise RuntimeError('Installed test binary changed')
        self.manifest['testedBinarySha256'] = actual_digest
        start=int(time.time()*1000)
        if run(['open','-n',str(self.app)]).returncode: raise RuntimeError('LaunchServices failed')
        self.actions.append({'actor':'codex','action':'LaunchServices '+mode,'runId':self.run_id,'at':start})
        if mode not in ['native-cleanup','native-inventory']:
            wait_until(lambda:self.pids(),15)
        self.manifest['phases'].append({'mode':mode,'launchedAt':start,'pids':self.pids()});self.save()

    def log(self):
        path=self.data/'app.log'
        value=path.read_text() if path.exists() else ''
        (self.out/'app.log').write_text(value)
        return value

    def step(self, command, stage, timeout=30):
        # Real editor clicks need the owned AppKit window active. This is setup,
        # never a notification response or a rescue during arrival observation.
        if not self.inventory_only and command in ['native-edit','native-in-app','native-end','native-end-fault','native-quit']:
            if console_state()['locked'] is True:
                raise PermissionError('Console locked during next-track native input setup; external GUI prerequisite unavailable, no unlock attempted')
            # AX Raise is not evidence that AppKit accepts own-window input.
            # This is next-track setup after the preceding assertions, not
            # assistance inside a notification arrival/response window.
            self.step('native-activate',10)
        count=self.log().count('smoke-check reminder-native-stage a=1 b='+str(stage))
        self.scenario(command+':'+uuid.uuid4().hex[:8])
        def done():
            value=self.log()
            if 'smoke-check reminder-native-stage a=-1' in value: raise RuntimeError('Real WK assertion failed')
            return value.count('smoke-check reminder-native-stage a=1 b='+str(stage))>count
        wait_until(done,timeout)
        self.actions.append({'actor':'codex','action':'own-window '+command,'runId':self.run_id,'at':int(time.time()*1000)})
        self.snapshot(command)

    def board(self):
        value=json.loads((self.data/'deck.json').read_text()); return value.get('data',value)

    def card(self):
        return next(c for c in self.board()['cards'] if c['title'].startswith('Reminder native '))

    def inventory(self, fresh_after=0):
        value=json.loads((self.data/'evidence/reminder-system-inventory.json').read_text())
        if value['observedAt']<fresh_after: return None
        if not all(value.get('privateEnvironment', {}).get(k) is True for k in ['home','claude','codex']): raise RuntimeError('Runtime private environment boundary failed')
        return value

    def snapshot(self, label):
        value={'observedAt':int(time.time()*1000),'pids':self.pids()}
        if (self.data/'deck.json').exists():
            value['cards']=[{key:c[key] for key in ['id','session','title','status','reminderRetirements'] if key in c} | ({'reminder':{k:v for k,v in c['reminder'].items() if k!='note'}} if c.get('reminder') else {}) for c in self.board()['cards']]
        try: value['inventory']=self.inventory()
        except (ValueError,FileNotFoundError): pass
        (self.out/(label+'.json')).write_text(json.dumps(value,indent=2))
        return value

    def registered(self, label):
        card=self.card();r=card['reminder'];identifier='deck.reminder.'+card['id'].encode().hex()+'.'+r['id']+'.'+str(r['revision'])
        def check():
            value=self.inventory()
            rows=[row for row in value['pending'] if row['identifier']==identifier]
            own_prefix='deck.reminder.'+card['id'].encode().hex()+'.'
            obsolete=[row for row in value['pending']+value['delivered'] if row['identifier'].startswith(own_prefix) and row['identifier']!=identifier]
            return value if value['authorization'] in [3,4] and len(rows)==1 and not obsolete and abs(rows[0]['dueAt']-r['dueAt'])<1000 else None
        value=wait_until(check,15)
        self.snapshot(label);return card,identifier,value

    def quit(self):
        self.step('native-quit',5)
        wait_until(lambda:not self.pids(),15)
        at=int(time.time()*1000);self.manifest['phases'].append({'normalExitConfirmedAt':at,'remainingOwnedPids':[]});self.save();self.scenario('native-idle');return at

    def maintenance(self, cleanup=False, name='quit-delivery-inventory.json'):
        if self.pids(): raise RuntimeError('Inventory observer must not conceal a running Deck')
        path=self.data/'evidence/reminder-maintenance.json';path.unlink(missing_ok=True)
        self.launch('native-cleanup' if cleanup else 'native-inventory')
        wait_until(lambda:path.exists() and not self.pids(),15)
        value=json.loads(path.read_text())
        if value.get('boardLoaded') is not False: raise RuntimeError('Observer loaded Board')
        (self.out/('cleanup-inventory.json' if cleanup else name)).write_text(json.dumps(value,indent=2))
        self.scenario('native-idle');return value

    def registered_after_exit(self, label):
        """A response-only launch has ended. The same identity reads the
        notification center without loading a Board: the committed reminder's
        request must be the one pending."""
        card=self.card();r=card['reminder'];identifier='deck.reminder.'+card['id'].encode().hex()+'.'+r['id']+'.'+str(r['revision'])
        value=self.maintenance(name=label+'-inventory.json')
        rows=[row for row in value['pending'] if row['identifier']==identifier]
        own_prefix='deck.reminder.'+card['id'].encode().hex()+'.'
        obsolete=[row for row in value['pending']+value['delivered'] if row['identifier'].startswith(own_prefix) and row['identifier']!=identifier]
        if value['authorization'] not in [3,4] or len(rows)!=1 or obsolete or abs(rows[0]['dueAt']-r['dueAt'])>=1000:
            raise RuntimeError('Cold Snooze ended without its registered request')
        snapshot={'observedAt':int(time.time()*1000),'pids':self.pids(),'inventory':value,
                  'cards':[{key:c[key] for key in ['id','session','title','status','reminderRetirements'] if key in c} | ({'reminder':{k:v for k,v in c['reminder'].items() if k!='note'}} if c.get('reminder') else {}) for c in self.board()['cards']]}
        (self.out/(label+'.json')).write_text(json.dumps(snapshot,indent=2))
        return card,identifier,value

    def ui(self, kind, title=None, identifier=None):
        """Ask only the connected authorized agent driver, never a human."""
        request={'id':uuid.uuid4().hex,'kind':kind,'driver':'cua_repl','runId':self.run_id,'bundleId':self.bundle_id,'bundle':str(self.app),'title':title,'identifier':identifier,'issuedAt':int(time.time()*1000),'deadlineSeconds':self.ui_timeout,'responsePath':str(self.out/'ui-response.json')}
        response=self.out/'ui-response.json';response.unlink(missing_ok=True)
        (self.evidence/'ui-request.json').write_text(json.dumps(request,indent=2))
        print(json.dumps({'systemUIRequest':request}),flush=True)
        wait_until(lambda:response.exists(),self.ui_timeout)
        value=json.loads(response.read_text())
        if value.get('id')!=request['id'] or value.get('driver')!='cua_repl': raise RuntimeError('UI receipt identity mismatch')
        if value.get('actor')!='codex': raise RuntimeError('UI action has no actor audit')
        self.actions.append(dict(value,runId=self.run_id,action='system UI '+kind))
        (self.out/('ui-'+request['id']+'.json')).write_text(json.dumps({'request':request,'response':value},indent=2))
        if value.get('status')!='PASS': raise PermissionError(value.get('reason','Authorized system UI action unavailable'))
        if not value.get('observation'): raise RuntimeError('UI action has no observation audit')
        return value

    def callback(self, kind, identifier, previous_revision, ends=False):
        def match():
            p=self.data/'evidence/reminder-native-actions.json'
            if not p.exists(): return None
            rows=[a for a in json.loads(p.read_text()) if a['request']==identifier and a['kind']==kind]
            if len(rows)>1: raise RuntimeError('Duplicate system callback in exclusive-input trial')
            return rows[0] if rows else None
        action=wait_until(match,20)
        if action['cardId']!=self.card()['id'] or action['revision']!=previous_revision: raise RuntimeError('Wrong native response identity')
        # A Snooze answered while Deck was not running is a response-only
        # launch: Deck transacts it and ends by itself. Every other response
        # leaves a running Deck.
        wait_until((lambda:not self.pids()) if ends else (lambda:self.pids()),15)
        self.snapshot('callback-'+kind);return action

    def clean(self):
        """Stop exact owned generations before non-rearming UN withdrawal."""
        if (self.root/'owner').read_text()!=self.run_id or (self.app/'Contents/Resources/reminder-smoke-owner').read_text()!=self.run_id: raise RuntimeError('Cleanup ownership changed')
        binary=self.app/'Contents/MacOS/tmux';server=run([str(binary),'-L',self.socket,'display-message','-p','#{pid}'])
        server_pid=int(server.stdout.strip()) if server.returncode==0 else None
        sentinel_pids=[row['pid'] for row in self.sentinel_samples()[-1:]] if self.sentinel else []
        parents=set(self.pids()+([server_pid] if server_pid else [])+sentinel_pids);children=set()
        rows=[line.split() for line in subprocess.check_output(['ps','-axo','pid=,ppid='],text=True).splitlines()]
        for _ in range(20):
            found={int(row[0]) for row in rows if len(row)==2 and int(row[1]) in parents}-parents
            if not found: break
            children|=found;parents|=found
        fingerprints={pid:subprocess.run(['ps','-p',str(pid),'-o','lstart=','-o','comm='],capture_output=True,text=True).stdout.strip() for pid in parents}
        for pid in self.pids():
            if subprocess.run(['ps','-p',str(pid),'-o','lstart=','-o','comm='],capture_output=True,text=True).stdout.strip()==fingerprints[pid]:
                try:os.kill(pid,15)
                except ProcessLookupError:pass
        wait_until(lambda:not self.pids(),8)
        notification=self.maintenance(True)
        notifications_empty=notification.get('pending')==[] and notification.get('delivered')==[]
        self.log();shutil.copytree(self.data/'evidence',self.out/'native-evidence',dirs_exist_ok=True)
        if server.returncode==0:run([str(binary),'-L',self.socket,'kill-server'])
        for pid in children | set(sentinel_pids):
            current=subprocess.run(['ps','-p',str(pid),'-o','lstart=','-o','comm='],capture_output=True,text=True).stdout.strip()
            if current and current==fingerprints[pid]:
                try:os.kill(pid,15)
                except ProcessLookupError:pass
        time.sleep(.3)
        remaining=[pid for pid in parents if subprocess.run(['ps','-p',str(pid),'-o','lstart=','-o','comm='],capture_output=True,text=True).stdout.strip()==fingerprints[pid] and fingerprints[pid]]
        reachable=run([str(binary),'-L',self.socket,'display-message','-p','#{pid}']).returncode==0
        socket=Path('/tmp')/('tmux-'+str(os.getuid()))/self.socket
        if not reachable and socket.exists():socket.unlink()
        okay=notifications_empty and not remaining and not self.pids() and not reachable and not socket.exists()
        result={'notificationsVerified':notifications_empty,'inventory':notification,'remainingOwnedPids':remaining,'serverUnreachable':not reachable,'socketAbsent':not socket.exists(),'installationRemoved':False,'rootRemoved':False}
        if okay:
            shutil.rmtree(self.app);shutil.rmtree(self.root)
            result.update(installationRemoved=not self.app.exists(),rootRemoved=not self.root.exists())
        self.manifest['cleanup']=result;self.save();return okay


def local_main(args):
    evidence=args.evidence.resolve();evidence.mkdir(parents=True,exist_ok=True);os.chmod(evidence,0o700)
    start_digest=source_digest();actions=[];owned=[];rows=[{'id':key,'status':'NOT RUN','executed':False,'evidence':[]} for key in IDS]
    native=[{'id':'N'+str(n),'status':'NOT RUN','executed':False,'evidence':[]} for n in range(1,7)]
    report={'startingHEAD':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),'branch':subprocess.check_output(['git','branch','--show-current'],cwd=ROOT,text=True).strip(),'initialStatus':subprocess.check_output(['git','status','--short'],cwd=ROOT,text=True),'candidateDiffDigest':start_digest,'target':'local','historicalEvidence':str(Path.home()/'Downloads/Deck-reminder-verification/20260930/unattended-acceptance.json'),'trials':[],'matrix':rows,'nativeMatrix':native,'actions':actions,'commands':[],'blockers':[],'readiness':{'ownWindow':None,'notificationAuthorization':None,'systemNotificationActions':None,'coldStart':None,'safeSleep':False},'consoleState':console_state(),'executionScope':'inventory' if args.inventory_only else 'complete'}
    cleanup=True;identity=True;failure=None;frozen_root=None;frozen_owner=uuid.uuid4().hex
    try:
        if not args.inventory_only and report['consoleState']['locked'] is True:
            raise PermissionError('Public console state is explicitly locked; GUI authentication is outside authorized test scope. Inventory-only checks remain independent.')
        if not os.access('/Applications',os.W_OK): raise PermissionError('Dedicated test installation requires unavailable privilege')
        if any('io.c9r.deck.reminder.smoke.local' in l for l in subprocess.check_output(['ps','-axo','command='],text=True).splitlines()):raise RuntimeError('Dedicated bundle identity occupied')
        result=run(['cargo','build','--manifest-path',str(TAURI/'Cargo.toml')],timeout=600,log=evidence/'candidate-build.log');report['commands'].append({'command':'cargo build','exitCode':result.returncode})
        if result.returncode:raise RuntimeError('Candidate build failed')
        frozen_root=Path(tempfile.mkdtemp(prefix='deck-reminder-candidate-',dir='/tmp'))
        (frozen_root/'owner').write_text(frozen_owner)
        binary=frozen_root/'deck-app';shutil.copy2(TAURI/'target/debug/deck-app',binary)
        report['frozenBuild']={'root':str(frozen_root),'removed':False}
        report['binaryIdentity']={'sha256':digest(binary),'version':json.loads((TAURI/'tauri.conf.json').read_text())['version']}
        result=run(['security','find-identity','-v','-p','codesigning']);match=re.search(rb'\b([A-F0-9]{40})\b[^\n]*Developer ID Application',result.stdout)
        if not match:raise PermissionError('Existing signing identity unavailable')
        signing=match.group(1).decode()
        for index in range(1 if args.fault_mode else 3):
            carrier=LocalCarrier(evidence,signing,binary,index,actions,args.ui_timeout,args.inventory_only);owned.append(carrier);report['trials'].append(carrier.manifest)
            carrier.launch()
            if not args.inventory_only:
                carrier.step('native-activate',10)
                if not args.fault_mode: carrier.ui('setup-foreground')
            carrier.step('native-create-inventory' if args.inventory_only else 'native-create',1)
            report['readiness']['ownWindow']=True
            observed=wait_until(lambda:carrier.inventory(),15)
            if observed['authorization'] not in [3,4]:
                carrier.ui('authorize-notifications')
                observed=wait_until(lambda:carrier.inventory() if carrier.inventory()['authorization'] in [3,4] else None,30)
            report['readiness']['notificationAuthorization']=observed['authorization'] in [3,4]
            card,identifier,inventory=carrier.registered('initial-registration')
            if args.fault_mode:
                if args.fault_mode=='timeout':wait_until(lambda:False,1)
                if args.fault_mode=='cancel':raise KeyboardInterrupt('deliberate cancellation cleanup control')
                raise AssertionError('deliberate assertion cleanup control')
            facts=carrier.manifest['facts']
            if args.inventory_only:
                exited=carrier.quit();due=card['reminder']['dueAt']
                wait_until(lambda:int(time.time()*1000)>due+1000,max(1,(due+45000-int(time.time()*1000))/1000))
                value=carrier.maintenance();delivery=assert_delivered_after_exit(value,identifier,exited,due+45000)
                facts['afterQuitDelivery']={'exitedAt':exited,'dueAt':due,'delivered':delivery,'observerDidNotLoadBoard':True}
                carrier.launch()
                carrier.step('native-edit-inventory',2);edited,edited_id,_=carrier.registered('edited-system-registration')
                if edited['reminder']['revision']<=card['reminder']['revision'] or edited_id==identifier: raise RuntimeError('Editor failed to replace old native request identity')
                carrier.step('native-end-fault-inventory',9);carrier.registered('failed-end-still-protected')
                carrier.step('native-in-app-inventory',7)
                wait_until(lambda:carrier.inventory()['pending']==[] and carrier.inventory()['delivered']==[],10)
                carrier.step('native-end-inventory',3)
                carrier.quit();carrier.launch();carrier.step('native-observe',4)
                if carrier.card().get('reminder') or not carrier.card().get('reminderRetirements'):raise RuntimeError('Ending lost durable retirement guard')
                facts.update(editedNativeRequestReplaced=True,saveFailurePreservesProtection=True,inAppOnlyWithdrawsSystemRequests=True,endedRestartRetained=True,configurationDriver='own-DOM in actual WKWebView')
                carrier.manifest['inventoryClosure']='PASS';carrier.save()
                if not carrier.clean():cleanup=False;raise RuntimeError('Real cleanup failed')
                continue
            # Foreground Deck is viewing a different test card. No arrival input.
            carrier.step('native-other',8)
            baseline=wait_until(lambda:carrier.inventory(),10)
            facts['foregroundBaseline']=baseline
            due=card['reminder']['dueAt'];deadline=due+45000
            wait_until(lambda:int(time.time()*1000)>=due+1000,max(1,(deadline-int(time.time()*1000))/1000))
            delivered=wait_until(lambda:carrier.inventory() if any(row['identifier']==identifier for row in carrier.inventory()['delivered']) else None,30)
            try:
                carrier.ui('observe-visible',card['title'],identifier)
                facts['foregroundSystemVisibility']='PASS'
            except (PermissionError, TimeoutError) as error:
                facts['foregroundSystemVisibility']='BLOCKED'
                report['blockers'].append('N1 foreground system visibility: '+str(error))
            carrier.step('native-observe',4)
            facts['foregroundDelivery']={'dueAt':due,'inventory':delivered,'evidence':'native-observe.json'}
            facts['foregroundControl']='PASS' if baseline.get('appFrontmost') is True and delivered.get('appFrontmost') is True else 'BLOCKED'
            if facts['foregroundControl']=='BLOCKED': report['blockers'].append('Foreground control not established or externally changed; remaining native tracks continue')
            # Running-system Snooze is a system action, followed by exact Board assertions.
            carrier.ui('snooze',card['title'],identifier)
            action=carrier.callback('snooze',identifier,card['reminder']['revision'])
            wait_until(lambda:carrier.card()['reminder']['dueAt']==action['actedAt']+3600000,15)
            carrier.registered('running-snooze-registration');facts['runningSnooze']=action
            # A separate owned foreground application; arrival must not activate Deck.
            carrier.step('native-edit',2);background_card,background_id,_=carrier.registered('sentinel-registration')
            carrier.start_sentinel()
            background_due=background_card['reminder']['dueAt']
            samples=wait_until(lambda:carrier.sentinel_samples() if carrier.sentinel_samples()[-1].get('sentinelFrontmost') else None,10)
            armed=int(time.time()*1000)
            wait_until(lambda:int(time.time()*1000)>background_due+1000,max(1,(background_due+45000-int(time.time()*1000))/1000))
            background=wait_until(lambda:carrier.inventory() if any(r['identifier']==background_id for r in carrier.inventory()['delivered']) else None,30)
            samples=[s for s in carrier.sentinel_samples() if armed<=s['at']<=background['observedAt']]
            if background.get('appFrontmost') is True: raise RuntimeError('Reminder arrival activated Deck from the owned sentinel')
            facts['sentinelForegroundDelivery']={'armedAt':armed,'dueAt':background_due,'inventory':background,'samples':samples,'status':'PASS' if samples and all(s['sentinelFrontmost'] for s in samples) else 'BLOCKED'}
            try:
                carrier.ui('observe-visible',background_card['title'],background_id)
                facts['sentinelSystemVisibility']='PASS'
            except (PermissionError, TimeoutError) as error:
                facts['sentinelSystemVisibility']='BLOCKED'
                report['blockers'].append('N1 sentinel system visibility: '+str(error))
            # End the passive foreground track before the next owned setup.
            carrier.step('native-activate',10)
            # A real quit, then a non-rearming observer under the same identity.
            carrier.step('native-edit',2);card,identifier,_=carrier.registered('before-quit-registration')
            exited=carrier.quit();due=card['reminder']['dueAt'];wait_until(lambda:int(time.time()*1000)>due+1000,max(1,(due+45000-int(time.time()*1000))/1000))
            value=carrier.maintenance();delivery=assert_delivered_after_exit(value,identifier,exited,due+45000)
            facts['afterQuitDelivery']={'exitedAt':exited,'dueAt':due,'delivered':delivery,'observerDidNotLoadBoard':True}
            if carrier.pids():raise RuntimeError('Notification click must cold-start, observer still running')
            carrier.ui('default-click',card['title'],identifier)
            action=carrier.callback('open',identifier,card['reminder']['revision']);carrier.step('native-observe',4)
            observations=re.findall(r'smoke-check reminder-native-observation a=(\d+)',carrier.log())
            if not observations or int(observations[-1])!=15: raise RuntimeError('Cold-start Open did not retain focus on the stopped due card')
            if carrier.card()['reminder']['revision']!=card['reminder']['revision'] or not carrier.card()['reminder']['due']:raise RuntimeError('Viewing ended or changed reminder')
            facts['defaultClickColdStart']=action;report['readiness']['coldStart']=True
            # New legal UI time; actual OS Snooze from a stopped application.
            carrier.step('native-edit',2);card,identifier,_=carrier.registered('cold-snooze-registration');exited=carrier.quit();due=card['reminder']['dueAt']
            wait_until(lambda:int(time.time()*1000)>due+1000,max(1,(due+45000-int(time.time()*1000))/1000))
            value=carrier.maintenance();assert_delivered_after_exit(value,identifier,exited,due+45000)
            carrier.ui('snooze',card['title'],identifier);action=carrier.callback('snooze',identifier,card['reminder']['revision'],ends=True)
            wait_until(lambda:carrier.card()['reminder']['dueAt']==action['actedAt']+3600000,15);carrier.registered_after_exit('cold-snooze-persisted');facts['coldSnooze']=action
            report['readiness']['systemNotificationActions']=True
            # The cold Snooze was a response-only launch: Deck committed the
            # answer, saw its request registered and ended. The next setup
            # starts it again as an ordinary launch.
            carrier.launch();carrier.registered('snooze-restart')
            carrier.step('native-end-fault',9);facts['saveFailurePreservesProtection']=True
            carrier.step('native-in-app',7)
            wait_until(lambda:carrier.inventory()['pending']==[] and carrier.inventory()['delivered']==[],10)
            facts['inAppOnlyWithdrawsSystemRequests']=True
            carrier.step('native-end',3)
            wait_until(lambda:carrier.inventory()['pending']==[] and carrier.inventory()['delivered']==[],10)
            carrier.quit();carrier.launch();carrier.step('native-observe',4)
            if carrier.card().get('reminder') or not carrier.card().get('reminderRetirements'):raise RuntimeError('Ending lost durable retirement guard')
            facts['endedRestartRetained']=True
            carrier.manifest['criticalClosure']='PASS';carrier.save()
            if not carrier.clean():cleanup=False;raise RuntimeError('Real cleanup failed')
        # Original and native rows are derived below from individual receipts.
        report['blockers'].append('Physical sleep/wake not authorized on shared local desktop; full platform certification remains blocked')
    except PermissionError as error:
        report['blockers'].append(str(error))
    except (RuntimeError,TimeoutError,subprocess.TimeoutExpired,AssertionError,KeyboardInterrupt) as error:
        failure=str(error);report['failure']=failure
        native.append({'id':'execution','status':'FAIL','executed':True,'evidence':[failure]})
    finally:
        for carrier in owned:
            if carrier.manifest.get('cleanup') is None:
                try:cleanup=carrier.clean() and cleanup
                except (RuntimeError,TimeoutError,OSError,subprocess.TimeoutExpired) as error:
                    cleanup=False;carrier.manifest['cleanup']={'error':str(error)};carrier.save()
        identity=source_digest()==start_digest and (not owned or binary_identity_matches([c.manifest for c in owned],report.get('binaryIdentity',{}).get('sha256')))
        if frozen_root is not None:
            if (frozen_root/'owner').is_file() and (frozen_root/'owner').read_text()==frozen_owner:
                shutil.rmtree(frozen_root)
            report['frozenBuild']['removed']=not frozen_root.exists()
            cleanup=cleanup and report['frozenBuild']['removed']
        report['candidateDiffDigestAfter']=source_digest();report['finalHEAD']=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip()
        report['trials']=[c.manifest for c in owned]
        receipts, resources = local_evidence(report, evidence)
        report['assertionReceipts'] = receipts
        report.update(aggregate(receipts, start_digest, resources, report['readiness']))
        report['userInterventionCount'] = sum(action.get('actor') == 'user' for action in actions)
        if failure or not cleanup or not identity:
            report['verdict']='FAIL'; report['exitCode']=1
        report['executionFailure'] = failure
        report['cleanupExecutionSucceeded'] = cleanup
        report['identityUnchanged'] = identity
        report['trials']=[c.manifest for c in owned]
        (evidence/'unattended-acceptance.json').write_text(json.dumps(report,indent=2))
        print(json.dumps({'verdict':report['verdict'],'exitCode':report['exitCode'],'report':str(evidence/'unattended-acceptance.json'),'cleanupCompleted':report['cleanupCompleted']}),flush=True)
    return report['exitCode']


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--evaluate-saved', type=Path, help='Read-only offline evaluation of a saved local report; never runs UI')
    parser.add_argument('--target',choices=['auto','macmini','local'],default='auto')
    parser.add_argument('--evidence',type=Path,default=Path.home()/'Downloads/Deck-reminder-verification'/datetime.datetime.now().strftime('%Y%m%d-%H%M%S'))
    parser.add_argument('--ui-timeout',type=int,default=90)
    parser.add_argument('--fault-mode',choices=['assert','timeout','cancel'])
    parser.add_argument('--inventory-only',action='store_true',help='Execute actual registration/quit/delivery/withdrawal only; required UI scopes remain unverified')
    args=parser.parse_args()
    if args.evaluate_saved:
        if args.evidence.resolve() == args.evaluate_saved.resolve().parent:
            parser.error('Offline output must not overwrite the original evidence directory')
        return evaluate_saved(args)
    # A logged-in local GUI plus a connected agent CUA coordinator is the
    # supported local capability path. Actual authorization/actions are tested
    # later; this selection never declares notification UI ready by itself.
    if args.target=='auto':
        local_gui=run(['stat','-f','%Su','/dev/console']).stdout.strip() not in [b'root',b'loginwindow',b'']
        args.target='local' if local_gui and os.access('/Applications',os.W_OK) else 'macmini'
    return local_main(args) if args.target=='local' else macmini_main(args)


if __name__=='__main__':
    raise SystemExit(main())
