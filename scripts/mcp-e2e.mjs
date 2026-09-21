#!/usr/bin/env node
// Independent JSON-RPC client for a real, isolated Deck MCP smoke run.
// Deck must already be open with an explicitly authorized disposable project.
import { spawn } from 'node:child_process';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

function options(argv) {
  const values = {};
  for (let index = 0; index < argv.length; index += 2) {
    const key = argv[index];
    const value = argv[index + 1];
    if (!key?.startsWith('--') || value === undefined) throw new Error('arguments must be --name value pairs');
    values[key.slice(2)] = value;
  }
  for (const key of ['adapter', 'socket', 'client-id', 'credential-file', 'project-id', 'cwd']) {
    if (!values[key]) throw new Error(`missing --${key}`);
  }
  return values;
}

const config = options(process.argv.slice(2));
const prefix = `e2e_${Date.now()}_${process.pid}`;
const holderId = `${prefix}_holder`;
const child = spawn(config.adapter, ['--client-id', config['client-id'], '--socket', config.socket, '--credential-fd', '3'], {
  stdio: ['pipe', 'pipe', 'pipe', 'pipe'],
});
child.stdio[3].end(readFileSync(config['credential-file']));
let nextId = 1;
let buffered = '';
let stderr = '';
const pending = new Map();
child.stdout.setEncoding('utf8');
child.stderr.setEncoding('utf8');
child.stderr.on('data', chunk => { stderr += chunk; });
child.stdout.on('data', chunk => {
  buffered += chunk;
  for (;;) {
    const at = buffered.indexOf('\n');
    if (at < 0) break;
    const line = buffered.slice(0, at);
    buffered = buffered.slice(at + 1);
    if (!line) continue;
    const message = JSON.parse(line);
    if (message.id !== undefined && pending.has(message.id)) {
      pending.get(message.id)(message);
      pending.delete(message.id);
    }
  }
});

function request(method, params = {}) {
  const id = nextId++;
  child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', id, method, params })}\n`);
  return new Promise((resolve, reject) => {
    pending.set(id, resolve);
    setTimeout(() => {
      if (pending.delete(id)) reject(new Error(`MCP timeout for ${method}`));
    }, 15_000).unref();
  });
}

async function call(name, args = {}) {
  const response = await request('tools/call', { name, arguments: args });
  assert.equal(response.error, undefined, JSON.stringify(response));
  assert.equal(response.result.isError, false, JSON.stringify(response.result));
  return response.result.structuredContent;
}

async function expectError(name, args, code) {
  const response = await request('tools/call', { name, arguments: args });
  assert.equal(response.result.isError, true, JSON.stringify(response.result));
  assert.equal(response.result.structuredContent.error.code, code);
}

async function waitOperation(operationId) {
  for (let count = 0; count < 80; count += 1) {
    const operation = await call('deck_operation_get', { operation_id: operationId });
    if (['committed', 'rejected', 'ambiguous'].includes(operation.state)) return operation;
    await new Promise(resolve => setTimeout(resolve, 250));
  }
  throw new Error('Deck operation did not finish');
}

async function readToExit(jobId, cursor) {
  let output = '';
  for (let count = 0; count < 100; count += 1) {
    const result = await call('deck_job_read', {
      job_id: jobId, cursor, max_bytes: 16 * 1024, wait_ms: 500,
    });
    output += result.output;
    cursor = result.nextCursor;
    if (['exited', 'lost'].includes(result.state)) return { ...result, output };
  }
  throw new Error('MCP job did not finish');
}

async function execJob(session, requestId, script, waitMs = 100) {
  const args = {
    request_id: requestId,
    session_id: session.sessionId,
    expected_generation: session.sessionGeneration,
    control_epoch: session.controlEpoch,
    holder_id: holderId,
    script,
    cwd: config.cwd,
    wait_ms: waitMs,
  };
  const started = await call('deck_exec', args);
  return { args, started, finished: await readToExit(started.jobId, started.outputCursor) };
}

try {
  const initialized = await request('initialize', {
    protocolVersion: '2025-06-18',
    capabilities: {},
    clientInfo: { name: 'deck-production-e2e', version: '1' },
  });
  assert.equal(initialized.result.serverInfo.name, 'deck-mcp');
  child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' })}\n`);
  const listed = await request('tools/list');
  assert.equal(listed.result.tools.length, 14);
  for (const name of ['deck_project_list', 'deck_project_read', 'deck_project_search']) {
    assert.ok(listed.result.tools.some(tool => tool.name === name), `${name} is registered`);
  }
  const controlTool = listed.result.tools.find(tool => tool.name === 'deck_session_control');
  assert.ok(controlTool.inputSchema.required.includes('holder_id'));

  const capabilities = await call('deck_capabilities');
  assert.equal(capabilities.executionMode, 'trusted-host');
  assert.equal(capabilities.realOsSandbox, false);

  const create = await call('deck_session_create', {
    request_id: `${prefix}_create`,
    project_id: config['project-id'],
    cwd: config.cwd,
    title: 'MCP production E2E',
  });
  const createDone = await waitOperation(create.operationId);
  assert.equal(createDone.state, 'committed', JSON.stringify(createDone));

  const sessions = await call('deck_sessions_list');
  assert.equal(sessions.sessions.length, 1);
  let session = sessions.sessions[0];
  const controlled = await call('deck_session_control', {
    request_id: `${prefix}_control`, session_id: session.sessionId,
    expected_generation: session.sessionGeneration, action: 'request', holder_id: holderId,
  });
  session = { ...session, ...controlled.result };
  assert.equal(session.controlOwner, config['client-id']);
  await expectError('deck_exec', {
    request_id: `${prefix}_before_grant`, session_id: session.sessionId,
    expected_generation: session.sessionGeneration, control_epoch: session.controlEpoch,
    holder_id: holderId, script: 'print MUST_NOT_RUN', cwd: config.cwd,
  }, 'EXECUTION_GRANT_REQUIRED');
  process.stderr.write('Approve the local execution window in the isolated Deck UI.\n');
  let approved = false;
  for (let count = 0; count < 600; count += 1) {
    const inspected = await call('deck_session_inspect', {
      session_id: session.sessionId, holder_id: holderId,
    });
    if (inspected.mayStartNextJob) { approved = true; break; }
    await new Promise(resolve => setTimeout(resolve, 500));
  }
  assert.equal(approved, true, 'local execution approval did not arrive');

  const failing = await execJob(session, `${prefix}_failing`, `git init -q
cat > calc.sh <<'EOF'
#!/bin/zsh
print 3
EOF
chmod +x calc.sh
cat > test.sh <<'EOF'
#!/bin/zsh
actual=$(./calc.sh)
if [[ "$actual" != 4 ]]; then
  print -u2 -- "expected 4, got $actual"
  exit 1
fi
print PASS
EOF
chmod +x test.sh
git add calc.sh test.sh
./test.sh`);
  assert.equal(failing.finished.exitCode, 1);
  assert.match(failing.finished.output, /expected 4, got 3/);

  const retry = await call('deck_exec', failing.args);
  assert.equal(retry.jobId, failing.started.jobId);
  await expectError('deck_exec', { ...failing.args, script: 'print MUST_NOT_RUN' }, 'REQUEST_ID_CONFLICT');

  const fixed = await execJob(session, `${prefix}_fixed`, `sed -i '' 's/print 3/print 4/' calc.sh
./test.sh`);
  assert.equal(fixed.finished.exitCode, 0);
  assert.match(fixed.finished.output, /PASS/);

  const diff = await execJob(session, `${prefix}_diff`, 'git diff -- calc.sh');
  assert.equal(diff.finished.exitCode, 0);
  assert.match(diff.finished.output, /-print 3/);
  assert.match(diff.finished.output, /\+print 4/);

  const longStarted = await call('deck_exec', {
    request_id: `${prefix}_long`,
    session_id: session.sessionId,
    expected_generation: session.sessionGeneration,
    control_epoch: session.controlEpoch,
    holder_id: holderId,
    script: 'for n in 1 2 3; do print -- "chunk-$n"; sleep 0.4; done',
    cwd: config.cwd,
    wait_ms: 50,
  });
  const firstRead = await call('deck_job_read', {
    job_id: longStarted.jobId,
    cursor: longStarted.outputCursor,
    max_bytes: 16 * 1024,
    wait_ms: 100,
  });
  assert.equal(firstRead.state, 'running');
  const longDone = await readToExit(longStarted.jobId, firstRead.nextCursor);
  assert.match(firstRead.output + longDone.output, /chunk-3/);

  const interactive = await call('deck_exec', {
    request_id: `${prefix}_interactive`,
    session_id: session.sessionId,
    expected_generation: session.sessionGeneration,
    control_epoch: session.controlEpoch,
    holder_id: holderId,
    script: 'IFS= read -r answer; print -- "answer=$answer"',
    cwd: config.cwd,
    wait_ms: 50,
  });
  await call('deck_job_input', {
    request_id: `${prefix}_input`,
    job_id: interactive.jobId,
    session_generation: session.sessionGeneration,
    control_epoch: session.controlEpoch,
    holder_id: holderId,
    input: 'hello 世界\n',
  });
  const interactiveDone = await readToExit(interactive.jobId, interactive.outputCursor);
  assert.equal(interactiveDone.exitCode, 0);
  assert.match(interactiveDone.output, /answer=hello 世界/);
  await expectError('deck_job_input', {
    request_id: `${prefix}_late_input`,
    job_id: interactive.jobId,
    session_generation: session.sessionGeneration,
    control_epoch: session.controlEpoch,
    holder_id: holderId,
    input: 'MUST_NOT_RUN\n',
  }, 'JOB_NOT_RUNNING');

  const interrupted = await call('deck_exec', {
    request_id: `${prefix}_interrupt_job`,
    session_id: session.sessionId,
    expected_generation: session.sessionGeneration,
    control_epoch: session.controlEpoch,
    holder_id: holderId,
    script: `trap 'print interrupted; exit 130' INT
while true; do sleep 1; done`,
    cwd: config.cwd,
    wait_ms: 50,
  });
  await call('deck_job_interrupt', {
    request_id: `${prefix}_interrupt`,
    job_id: interrupted.jobId,
    session_generation: session.sessionGeneration,
    control_epoch: session.controlEpoch,
    holder_id: holderId,
  });
  const interruptedDone = await readToExit(interrupted.jobId, interrupted.outputCursor);
  assert.equal(interruptedDone.interruptRequested, true);
  assert.ok(interruptedDone.exitCode === 130 || interruptedDone.terminationSignal === 2);

  const close = await call('deck_session_close', {
    request_id: `${prefix}_close`,
    session_id: session.sessionId,
    expected_generation: session.sessionGeneration,
    control_epoch: session.controlEpoch,
    holder_id: holderId,
    confirm_running: false,
  });
  const closeDone = await waitOperation(close.operationId);
  assert.equal(closeDone.state, 'committed', JSON.stringify(closeDone));
  assert.equal((await call('deck_sessions_list')).sessions.length, 0);

  console.log(JSON.stringify({
    pass: true,
    protocolTools: listed.result.tools.length,
    failingExit: failing.finished.exitCode,
    fixedExit: fixed.finished.exitCode,
    idempotentJobId: retry.jobId,
    incrementalFirstState: firstRead.state,
    interactiveExit: interactiveDone.exitCode,
    interruptExit: interruptedDone.exitCode,
    interruptSignal: interruptedDone.terminationSignal,
    closeState: closeDone.state,
    stderrBytes: Buffer.byteLength(stderr),
  }, null, 2));
} finally {
  child.stdin.end();
  await new Promise(resolve => child.once('exit', resolve));
  assert.equal(buffered, '');
}
