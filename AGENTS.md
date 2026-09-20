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
