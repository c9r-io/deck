import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const carrier = readFileSync(new URL('./channel-first-send-smoke.mjs', import.meta.url), 'utf8');
const launcher = readFileSync(new URL('../../run.sh', import.meta.url), 'utf8');
const fixture = readFileSync(new URL('../../src-tauri/examples/channel_fixture.rs', import.meta.url), 'utf8');
const launchArgs = readFileSync(new URL('../../src-tauri/src/launch_args.rs', import.meta.url), 'utf8');

test('the hidden interval is one native delayed-envelope await', () => {
  const hidden = carrier.indexOf("kind: 'hide'");
  const wait = carrier.indexOf("const staged = await inv('channel_smoke_envelope'");
  const after = carrier.indexOf("await report('channel-bg-envelope'", wait);
  assert.ok(hidden >= 0 && wait > hidden && after > wait);
  assert.match(carrier.slice(wait, after), /delaySecs: 245/);
  assert.doesNotMatch(carrier.slice(wait, after), /pause\(|setTimeout|snapshot|activate|queue_list|smoke_channel_fixture/);
  const native = readFileSync(new URL('../../src-tauri/src/inbound_channel.rs', import.meta.url), 'utf8');
  const oracle = native.slice(native.indexOf('pub(crate) async fn channel_smoke_envelope'), native.indexOf('pub(crate) fn channel_status'));
  const verdict = oracle.indexOf('let result = SmokeEnvelopeResult');
  const saved = oracle.indexOf('join("channel-smoke-result.json")');
  const notification = oracle.indexOf('app.emit("channel-changed", ())');
  assert.ok(oracle.indexOf('Duration::from_secs(75)') < verdict && verdict < saved && saved < notification);
  assert.equal(oracle.match(/app\.emit\(/g)?.length, 1, 'only one post-oracle notification');
});

test('the channel carrier launches behind other apps with a closed fixture PATH', () => {
  const branch = launcher.slice(launcher.indexOf('if [ "$SMOKE_MODE" = channel-first-send ]'));
  assert.match(branch, /OPEN_BACKGROUND=-g/);
  assert.match(branch, /Contents\/MacOS\/channel-fixture-bin/);
  assert.match(branch, /home\/\.zprofile/);
  assert.doesNotMatch(branch, /DECK_SMOKE_DATA_DIR\/fixture-bin/);
  assert.match(launcher, /codesign --force --sign - "\$APP\/Contents\/MacOS\/channel-fixture-bin\/claude"/);
  assert.ok(
    launcher.indexOf('codesign --force --sign - "$APP/Contents/MacOS/channel-fixture-bin/claude"')
      < launcher.indexOf('codesign --force --sign - --entitlements Entitlements.plist "$APP"'),
    'the nested fixture is signed before the app bundle is sealed',
  );
  assert.match(launcher, /deck-channel-smoke-\$CHANNEL_SMOKE_SUFFIX\.app/);
  assert.match(launcher, /io\.c9r\.deck\.smoke\.channel\.x\$CHANNEL_SMOKE_SUFFIX/);
  assert.match(fixture, /not named claude/);
  assert.match(fixture, /unsafe PATH/);
  assert.match(fixture, /canonicalize/);
  assert.match(fixture, /deck-channel-smoke-/);
  assert.match(launchArgs, /pub\(crate\) fn channel_fixture_bin/);
  assert.match(launchArgs, /std::fs::canonicalize\(std::env::current_exe/);
  assert.match(launchArgs, /metadata\.permissions\(\)\.mode\(\) & 0o111/);
  assert.match(launchArgs, /std::env::remove_var\(name\)/);
  assert.match(launchArgs, /format!\("\{\}:\/usr\/bin:\/bin", bin\.display\(\)\)/);
  assert.doesNotMatch(fixture, /std::process::Command|TcpStream|UdpSocket|NSPasteboard|pbcopy|pbpaste/);
});

// Accepted startup risk is not evidence of Agent readiness. The fixture has
// no hook, so a successful automatic head must audit its actual dependency.
test('the no-Signal carrier audits one actual readiness override', () => {
  assert.match(carrier, /readiness_overridden === true/);
  assert.match(carrier, /staged.persistedOverrides === 1/);
  assert.match(carrier, /fixture.persistedOverrides === 1/);
  assert.match(carrier, /ownIds.has\(value.item\)/);
  assert.match(carrier, /includes\(ownPlans\[0\].stage\)/);
});
