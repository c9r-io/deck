# Stable and Nightly release channels

deck has two update channels with one application and data identity. Stable is
the default for every existing and new installation. Nightly is an explicit
opt-in for maintainers and testers who want to exercise a production-signed
candidate before it is promoted.

| Channel | Client feed | GitHub Release | Intended use |
| --- | --- | --- | --- |
| Stable | `releases/latest/download/latest.json` | `vMAJOR.MINOR.PATCH`, not a prerelease | normal use |
| Nightly | `releases/download/nightly-feed/latest.json` | immutable `nightly-vMAJOR.MINOR.PATCH-YYYYMMDD-SHA`, prerelease | candidate testing |

The application contains these two HTTPS endpoints in a closed Rust enum. The
webview passes only `stable` or `nightly`; it cannot provide a URL. Each check
constructs one Tauri updater with exactly one endpoint, so a missing or damaged
Nightly feed is an error and never falls through to Stable or an unverified
source. Download, minisign verification, installation and relaunch continue to
use Tauri's updater implementation.

## Version and identity model

Every installable build uses one strictly increasing numeric
`MAJOR.MINOR.PATCH` version. A rejected candidate consumes its version; the next
candidate uses a larger one. Nightly labels belong only in the Git tag and
Release metadata, never in `CFBundleShortVersionString`, `CFBundleVersion`,
`tauri.conf.json`, `Cargo.toml`, or the updater manifest. This follows Apple's
requirement that the short version is three period-separated integers and lets
Tauri retain its normal monotonic semver comparison.

Nightly and Stable deliberately share product name `deck`, bundle identifier
`io.c9r.deck`, Developer ID, updater key, `~/.deck`, scheduler queue and tmux
socket. Nightly replaces the installed Stable application; the two must not run
at once. This is how a candidate exercises real upgrades, migrations, tmux
survival and scheduler data before the exact same binary is promoted.

Prepare a candidate commit with:

```sh
scripts/release-version set 0.4.37
scripts/release-version check
```

The tool accepts only the three-number form, updates the two source manifests
and the deck package entry in `Cargo.lock`, and rejects a version that is not
higher than local Stable or Nightly tags. The network-free parser and mutation
logic has fixture tests; the workflow separately compares the requested version
with GitHub Releases so a shallow or stale local tag set cannot weaken the gate.
The version change is committed before publishing. CI never overrides it.

The app shows `vVERSION · Stable|Nightly · COMMIT`. `COMMIT` is a short,
hex-only build identity embedded from the resolved candidate SHA (or the local
Git commit for a development build); it contains no path, account, secret or
user data.

## Publishing a candidate

Run the **nightly** workflow manually with `ref`, `version`, optional public
notes and `publish_feed`. The workflow:

1. validates all inputs, resolves `ref` once to a full immutable SHA, and
   requires that SHA to be reachable from `origin/main`;
2. verifies source versions and strict monotonicity against every Stable and
   candidate Release, checks tag/Release conflicts, then runs the complete test
   workflow gate;
3. builds Apple Silicon exactly once using the same Developer ID, Apple
   notarization credentials and updater minisign key as Stable;
4. notarizes and staples the final DMG, then runs codesign, stapler and
   Gatekeeper verification;
5. creates an annotated candidate tag at the resolved SHA and a draft
   prerelease, uploads the DMG, updater archive, signature, candidate manifest,
   `SHA256SUMS` and closed `provenance.json`, and downloads them into a fresh
   directory for byte/hash/manifest verification;
6. publishes the immutable prerelease only after all assets pass.

The provenance records version, full commit, tag, workflow run/attempt and UTC
time; filename, byte size and SHA-256 for every immutable artifact; bundle ID,
updater target, test result, public Team ID/identity label and the three Apple
verification results. It never records credentials, tokens, local paths or user
data.

If `publish_feed` is true, feed publication happens last. The workflow first
uploads and verifies a versioned `latest-VERSION.json` asset on the mutable
`nightly-feed` prerelease. It then replaces the short `latest.json`. A saved
copy of the prior pointer is restored on an ordinary command failure. An abrupt
runner loss can leave the short asset temporarily absent, which makes clients
report a failed check; it cannot point them at a partial candidate. The previous
versioned manifest remains available for a manual repair. Neither operation
touches `/releases/latest/`.

To reject a candidate, leave its prerelease and assets intact, document the
reason (a closed `rejected.json` marker may be added), prepare a larger numeric
version and run the workflow again. Moving the verified feed to that larger
candidate makes the old candidate superseded and ineligible for promotion.
Candidate Releases are retained as audit evidence; they are never reused or
overwritten.

## Promoting without rebuilding

After a real Nightly test period, run the **promote** workflow with the exact
candidate tag and a short public test-period description. The `production`
Environment is the human approval gate. Promotion fails closed unless the tag,
prerelease metadata, commit, source versions, provenance, required assets,
hashes, updater signature, app bundle identity/version/build commit, Apple
verification and required test run all agree. It also refuses an existing
Stable tag or Release.

The promotion job contains a static policy gate forbidding Cargo, Tauri and app
build commands. It downloads the candidate assets, verifies them, creates an
annotated `vVERSION` tag at the identical full commit, and creates a draft
Stable Release. It uploads byte-identical DMG/archive/signature assets, verifies
the copies by downloading them again, and generates only Stable metadata and a
Stable `latest.json` whose URL names `vVERSION`. Version and signature remain
unchanged. `latest.json` is uploaded last and the draft is published only after
that completeness marker passes. Release notes record the candidate, commit,
test period and hashes. Candidate assets remain untouched; promotion evidence
is added as a separate candidate asset rather than editing provenance.

The direct `vVERSION` source-build workflow remains an emergency path. Its tag
resolver accepts only the strict Stable shape, ignores candidate/feed tags, and
does not delete or overwrite an incomplete or already complete Release.
Candidate publication, promotion and direct Stable publication use per-version
concurrency groups. Repair accepts only a strict Stable tag.

## Installing, switching and recovery

Before installing Nightly, verify that important `~/.deck` data is backed up.
Select **Settings → Update channel → Nightly**, accept the one-time risk prompt,
then check for updates. Include the displayed version and commit when reporting
a problem.

Switching back to Stable changes only future checks and never downgrades the
installed app. If the installed Nightly version is newer than the latest
Stable, reinstall the Stable DMG to downgrade. A downgrade must still respect
deck's future-schema refusal and data compatibility guarantees; back up first.

Recovery is deliberately non-destructive:

- a broken Nightly candidate stays unpublished and the previous feed remains;
- a missing `nightly-feed/latest.json` can be restored from the last verified
  versioned manifest without rebuilding;
- an incomplete candidate or Stable Release is inspected and repaired by run
  ownership/provenance, never deleted automatically;
- a failed promotion leaves the candidate intact and never creates/overwrites a
  complete Stable Release; a draft created by that exact run may be resumed or
  removed only after matching its run evidence;
- Stable completeness is defined by a published, non-prerelease Release with
  its final `latest.json` marker.

## Trust boundary and references

GitHub hosts metadata and bytes but is not allowed to choose an endpoint or
bypass signature verification. The Developer ID and Apple notarization ticket
establish the macOS carrier identity. Tauri's committed public key verifies the
updater archive; its private key remains only in the protected GitHub
Environment. Promotion verifies and copies already signed/notarized bytes.

Authoritative constraints used by this design:

- [Tauri v2 updater documentation](https://v2.tauri.app/plugin/updater/)
- [`UpdaterBuilder::endpoints` and default semver comparison](https://docs.rs/tauri-plugin-updater/2.10.1/tauri_plugin_updater/struct.UpdaterBuilder.html)
- [tauri-action release and updater artifact inputs](https://github.com/tauri-apps/tauri-action)
- [GitHub latest Release excludes drafts and prereleases](https://docs.github.com/en/rest/releases/releases#get-the-latest-release)
- [Apple `CFBundleShortVersionString`](https://developer.apple.com/documentation/bundleresources/information-property-list/cfbundleshortversionstring)

## 中文说明

Stable 是所有新老用户的默认通道，只读取正式 `latest.json`；Nightly 必须
在设置中主动选择，只读取独立的 `nightly-feed`。Nightly 使用正常三段数字
版本，候选标识只写入 tag/Release。两条通道共用同一个 app、数据目录、tmux
服务、Developer ID 与 updater 签名，因此 Nightly 会替换 Stable，不能并行
运行。

安装 Nightly 前请确认重要数据已有备份。切回 Stable 只影响之后的更新检查，
不会自动降级；如果 Nightly 版本更高，需要重新安装 Stable DMG，并继续遵守
future-schema/data compatibility 规则。反馈问题时请附上设置页显示的版本、通道
和短 commit。

候选发布会完成与 Stable 相同的测试、签名、公证、staple、Gatekeeper 和
updater 签名检查。晋升只复制已经测试过的 DMG、archive 与 `.sig`，禁止重新
构建；候选与正式资产的 SHA-256、commit 和 bundle 身份必须完全一致。
