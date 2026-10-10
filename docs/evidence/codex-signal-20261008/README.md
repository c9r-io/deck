# 2026-10-08 certification evidence

This directory publishes metadata derived from the 2026-10-08 investigation.
Documentation preparation on 2026-10-10 started no new experiment.
It retains metadata only. No prompts, conversations, model output,
terminal captures, full environments, credentials, transcripts, or private repository
content are retained. Schema property names describe the public upstream protocol;
they are not recorded input values.

- `identity.json`: historical release, annotated tag, peeled commit, binary version/digests and baseline hashes. The local installation path is a labeled placeholder.
- `features.txt`: selected feature values from the isolated CLI.
- `schema-summary.json`: six public command-input schema definitions and baseline equality.
- `events.jsonl`: kernel peer PID, one in-memory process-table sample per helper,
  ancestry, pane/foreground process generations, consistently aliased session/turn IDs and experiment markers.
- `analysis.json`: turn correlation and independently calculated continuity results.
- `verify-evidence.py`: offline metadata/continuity verification; run with Python 3.
- `pre-cleanup.json`, `stop-result.json`, `cleanup.json`: historical scoped cleanup evidence. Local socket/alias paths in the first file are labeled placeholders; outcomes and process records are unchanged.
- `publication.json`: original file SHA-256 values, explicit publication transformations and derived metadata digests.
- `hook-helper.py`, `listener.py`, `process-snapshot.py`: historical reviewed probe source,
  archived for inspection with an added archive-only notice. The runtime names were `helper.py`, `listener.py`, and
  `probe_lib.py`. Their temporary runtime paths/configuration have been deleted;
  these are not a ready-to-run launcher. Do not point them at production services.

A/B/E are experiment labels. They are not trusted product identity fields.
The process-table sample enumerates PIDs and reads libproc records sequentially;
it is not an atomic kernel snapshot. Helpers stay connected/alive during collection.
Recorded PID+birth and same-table foreground checks narrow this limitation; this
experiment does not certify losslessness or race handling in installed Deck.

## Publication and verification limits

Original files and the private alias map are retained in a local ignored report
archive, excluded from this documentation commit. Session/turn aliases are an
injective mapping, shared by events and analysis. They are correlation labels,
not native IDs or trusted pane ownership. No event, process relationship, timing,
attribution result or cleanup outcome was synthesized or changed.

PID, birth, tty and process-group values are retained because the historical
attribution argument depends on these exact relationships and generation equality.
They describe the historical samples, not live machine state or reusable authority.
Absolute event timestamps identify the experiment interval; they are not content.
Public upstream source paths and version/digest identities remain unchanged.

Run only `python3 docs/evidence/codex-signal-20261008/verify-evidence.py` from the
repository root for offline checks. It reads stored metadata and checks derived
file digests; it launches no Agent, tmux, collector or listener. Successful offline
verification checks consistency, not authenticity of a new kernel observation.
Original SHA-256 values identify the locally retained sources but do not, on their
own, attest their origin. The archived collectors are not a reproduction launcher.

All results retain the original scope: shared-daemon attribution is blocked;
embedded samples pass topology checks only. No installed Deck Signal end-to-end,
full attention coverage, new vendor version or release path is certified here.
