# Commit ids cited in the evidence documents

Nineteen commits of this history were recreated on 2026-10-11, before anything
was published, because ONE of them (the first row below) carried a
machine-local fallback as its author and committer identity instead of the
identity every other commit carries. Only that identity changed. Every
recreated commit has the same tree, the same message and the same author and
committer dates as the commit it replaces; the ids differ because a commit's
id covers its parent and its identity lines.

Two documents written before the correction cite the EARLIER ids and were
left as written: `docs/evidence/first-party-extensions-20261009/README.md`
and `docs/codex-signal-certification-20261008.md`. The raw records of those
rounds, which no checkout has, use them too.
`docs/evidence/closeout-20261011/README.md` cited earlier ids until the final
round of 2026-10-11 and was then brought to the ids of this history; where a
run is identified by the commit it ran on, it names the earlier id beside
the current one. `reentry-acceptance.md` cites no commit id. An earlier id
does not exist in the published history; this table is how to find the
commit it meant. Documents written after the correction cite the ids of the
right column.

| Cited as (earlier id) | Id in this history | Subject |
| --- | --- | --- |
| `de60a7b` | `92e9050` | chore(extensions): log the count of service owners started at launch |
| `b1c4547` | `47151ea` | test(slack): env-gated real-service certification through the registered integration |
| `2059695` | `fb9b43b` | docs(evidence): first-party extensions phase 1 evidence index |
| `a869fbe` | `a7de6d5` | test(slack): transport faults after a live connection through the registered integration, with the status vector Settings renders |
| `4a80053` | `a973db9` | docs(evidence): closure round — coverage gate, channel fault assertions, evidence applicability |
| `ea5a065` | `2a129db` | fix(extensions): an identity two entries share is used by no consumer; the Slack re-announcement is tested through the tables |
| `d922cf4` | `7d36560` | docs(evidence): correction round — registration clashes, re-announcement seam, tmux statement |
| `abd325b` | `b6ad043` | feat(reentry): reopen selected Claude sessions after shell service restart |
| `9c3cf5c` | `77abc57` | docs(signal): record Codex 0.161.0 attribution certification |
| `cd31e24` | `93aa48f` | feat(reentry): admit Claude by verified identity instead of release digests |
| `a0fe806` | `f1cad64` | docs(claude-md): trim the App and Invariants sections to rules and pointers |
| `9e33294` | `d54d7ce` | fix(build): one writer for the build outputs every cargo context shares |
| `0ec2ea0` | `06fb0b5` | fix(reentry): say that observation is off instead of an unverifiable session |
| `e2abd57` | `02fa839` | test(coverage): the helpers' process runs reach the coverage report |
| `ee548d5` | `b7518af` | test(smoke): Slack against the real service, and the hook switches |
| `68ac43b` | `e25774d` | docs(evidence): a close-out summary a checkout can read; links leave reports/ |
| `f9df0ca` | `9e0bad3` | fix(build): the staged frontend takes no dot entries from the source tree |
| `5274067` | `c9d78f5` | test(smoke): the restart with re-entry in the real window, with real Claude clients |
| `49a03d1` | `0aeee77` | docs(evidence): the follow-up — one more staging gap, and the transaction in a real window with real clients |

Full ids, in the same order:

```
de60a7b9615282a98338a66e055e5d2a096866d2 -> 92e905057cf812bd8e2a8c85922a2cc4304d0229
b1c4547641443a496127511a3ad2bb0089168ff2 -> 47151eabf4cdb2da2c990b1ccc9d07779e46a036
20596950c0844ef98aaff67b8b2773ba5e9313fd -> fb9b43b1a6b829003a1339ac1d5956c57c6d85e2
a869fbe5ff524e056d334c5337cb41aa828967a1 -> a7de6d577e38fedc8dd2a92a32a040f4ce8a5aea
4a800530cc4cafb3f8f5391873a20d7c397ab60e -> a973db905375d7f69a8fdff2ff254b5db684ee30
ea5a065a151b7bdf75343b1140ef51392994943c -> 2a129db7abb5cb3c01dc4b5f0095104ec6cfd601
d922cf443c92ddd6d0f50dd7dff3cb1d303f4bf6 -> 7d365603227248848f2d053b11a1f2252e4d81d3
abd325bf7b02ae7b0844bc5cd1df2c69bf82c8e3 -> b6ad04315277e14b0b7e4ad7a0be64ad6370d1af
9c3cf5c9ffb248ac785d291a63472f595ea1a4df -> 77abc570c6b3d8f2a6c2d5d2d89ec7173a5493a3
cd31e24fe423d1269359834312142c08044d634f -> 93aa48f57f673d4b6ff35ba127ee5b6ca88904fd
a0fe80685fb8f684e4658238cfa3a8c8fa018abb -> f1cad6466da8b56035a97f0a7d138ad0cd0a746b
9e33294af0e331f1bb5018a449beae7a04be1855 -> d54d7cebdf53c06b7e7139dc7a13093a5524116b
0ec2ea0bba3cdfefc48477141db56903721d1e95 -> 06fb0b55262d6427488c9a55942f96ac387593b1
e2abd57f32ca260b1e12aa52075bade6736bbf47 -> 02fa8393892c47f2c93e8c3e782d5934d60bd5bb
ee548d53cb0e05b664689477c14f70563a5d0d9c -> b7518af32012f354fb837e9aedc5229af73dd199
68ac43b3e65c9347dc7a27b09920951efb22959f -> e25774da7ad74335e874a42c408a88e77b4cc9a8
f9df0ca5654ba1d3da5474e9119cbbd3608ffa70 -> 9e0bad357acbaf6585649090f55f7b6628a169ab
5274067301aec657e52b2d1f1264fceb1ab99cb3 -> c9d78f5db0066637ea950e7a10668d369f3643d6
49a03d150114ed3da38a5d153d342943842bc1e7 -> 0aeee77db883d29422f1ba4afb5a540b6b60962f
```
