# Deck agent instructions

Read `CLAUDE.md` before changing this repository. Its hard rules, module
ownership map, release restrictions, EDR constraints, and test gates are
binding. Read the owning module header before editing that subsystem.

## Mac mini test resource

An isolated Apple Silicon Mac mini is available through the existing SSH alias
`macmini`. The alias owns its host, user, host-key, and authentication details;
do not copy those details, private keys, Apple credentials, kube endpoints, or
internal addresses into this repository or command output.

Always inventory live state before using it; do not treat this snapshot as a
permanent machine configuration:

```sh
ssh -o BatchMode=yes -o ConnectTimeout=10 macmini \
  'whoami; hostname; sw_vers; uname -m; stat -f %Su /dev/console'
```

As last verified on 2026-09-20, it was an arm64 Mac mini with an active GUI
login, macOS 27.0 (build 26A428), network access, Command Line Tools, Rust,
Python and Swift. It had no full Xcode, no local code-signing identity, no Deck
installation, and no `kubectl` binary after cleanup. A kubeconfig existed but
its credentials were not valid; never assume Kubernetes readiness from the
file's presence. Recheck all of these facts for each task.

Use this host for isolated signed-app, updater, LaunchServices, Local Network
Privacy, responsible-code, and clean-install experiments when the task calls
for them. Build and Developer-ID-sign on an authorized machine that actually
has the signing identity, then transfer only the finished test artifacts with
`scp`. Prefer a one-time updater key and a loopback-only test feed. Production
updater keys, feeds, tags, releases, notarization credentials, or Apple
Developer changes require explicit authorization and are not prerequisites for
ordinary isolated updater tests.

Install a release carrier only after confirming `/Applications/deck.app` and
`~/.deck` do not contain user state. Launch GUI apps through LaunchServices,
not by executing the binary from SSH:

```sh
ssh macmini 'open -n /Applications/deck.app --args --debug-logging'
```

Follow Deck's EDR and privacy rules during experiments: do not add launchd
jobs, run AppleScript, reset TCC, edit privacy databases, add privileged routes,
or persist executables under the home directory. Create sessions through the
Deck UI/backend when testing process attribution. A successful generic LAN TCP
probe does not prove that Kubernetes or VPN/local routes work.

The 2026-09-20 signed A-to-B updater experiment showed that keeping the old
tmux server across an app replacement left both old and newly created panes
with `kubectl` reporting `no route to host`, even though generic LAN TCP still
worked. Restarting tmux from the installed build immediately changed the same
request to the expected authentication response. Therefore do not relax the
production build-change restart boundary without a new signed updater test
covering the actual Kubernetes route.

Before finishing, stop only processes created by the experiment, restore the
pre-test installation/data state, and inventory again. Prefer moving exact test
artifacts to a uniquely named Trash directory over irreversible deletion. Never
interrupt unrelated sessions or services.

## Experiment resources: registered when started, stopped only through the registry

An experiment outside this checkout's debug bundles (a release carrier, a
relocated home, anything on the Mac mini) keeps everything it starts under ONE
root directory of its own under a temporary directory, its tmux socket
directory included: the carrier is launched with `TMUX_TMPDIR` inside the root,
so its server listens on `<root>/…/tmux-<uid>/<name>`. Keep the root short: a
socket path has about 100 bytes. A release carrier that would use the default
tmux directory is not launched on a machine that has an installed Deck.

Each server and process is registered when it is started, and the registry is
the only way the experiment stops one:

```sh
python3 scripts/edr_runtime.py experiment init --root <root>
python3 scripts/edr_runtime.py experiment add-server --root <root> --socket <full socket path> --tmux <tmux program>
python3 scripts/edr_runtime.py experiment add-process --root <root> --pid <pid>
python3 scripts/edr_runtime.py experiment show --root <root>
python3 scripts/edr_runtime.py experiment stop --root <root> --pid <pid>
python3 scripts/edr_runtime.py experiment stop --root <root> --pid <pid> --crash
python3 scripts/edr_runtime.py experiment cleanup --root <root>
```

On the Mac mini the same file is read from standard input:
`ssh macmini "python3 - experiment cleanup --root <root>" < scripts/edr_runtime.py`.

`stop` ends one registered resource and leaves the rest running. `--crash`
is a loss under test (a power-loss rehearsal), not a way to stop: the same
verification, then one SIGKILL, and a server's socket file is left behind. `cleanup`
verifies every entry again before it stops anything: the socket path
is inside the root, the server that answers on it reports the registered pid,
and that pid still has the registered start time and executable. One entry that
does not hold refuses the whole cleanup. Exit 3 means nothing was stopped; exit
1 means something that was never registered still runs from the root. A refusal
is looked into. It is never worked around with a broader command, a `pkill`
pattern, or a kill by a pid read from a listing.

A tmux server is never addressed by its socket name in a cleanup. A release
build names its socket `deck` wherever it runs, and `tmux -L deck` is resolved
in the calling shell's `TMUX_TMPDIR`. On 2026-10-11 a cleanup that ran
`tmux -L deck kill-server` without the experiment's socket directory stopped
the Mac mini's installed Deck shell service; its sessions could not be brought
back (`docs/evidence/closeout-20261011/README.md`, "Incident"). A pid alone, a
path prefix alone, a socket name, or a server missing from a listing is not
proof that a target is the experiment's. The regressions, and the register of
every tracked script that stops a tmux server, are in
`scripts/test_edr_runtime.py`.
