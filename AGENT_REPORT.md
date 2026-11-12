# Issue #661: split-window p99 ten times its p90 on a session that only splits

Worktree `agent-a5468c5f96a91e9a8`, base master 37e2990. Everything below comes
from runs on this machine on 2026-09-18 with `WT\target\release\psmux.exe`,
built only with `cargo build --release`.

## 1. Reproduction and the depth trajectory

Harness: one session under `-L i661<tag>`, one attached client started by
`Start-Process` and closed by pid, the pool warmed for 4000 ms first (a fresh
`-L` namespace starts with a COLD pool), then 40 `split-window -v` about 250 ms
apart, each timed from the command going out to the NEW pane id being active
AND a prompt visible in it. `PSMUX_WARM_TRACE=1` throughout.

40 splits one second apart do NOT reproduce it (p50 15.3, p90 18.4, max 33.7):
a refill matures in ~290 ms, so a one second cadence is never a run. The report
is about a run, and 250 ms is the cadence in it (36 splits, misses at i=2 and
i=31, depth 7 from split 4 to 28).

A new env gated line was added for this, four times a second, because a claim
line only says what one creation found: `pool: sample depth=.. ready=..
inflight=.. target=.. eff=.. surging=..` (src/server/mod.rs:1570, commit
396a56c). Kept, it is what the regression suite reads.

The collapse, from `warm_pre1.log` (server pid 54500, unfixed build):

```
[ 12736.402 pid=54500] claim(split): pane=31 spare_age=2046.2ms pool_left=7 ready_left=6
[ 12839.661 pid=54500] pool: surge over, trimmed 6 surplus spare(s) back to target=2
[ 12840.356 pid=54500] pool: sample depth=2 ready=2 inflight=0 target=2 eff=2 surging=false
[ 13002.912 pid=54500] claim(split): pane=32 spare_age=2056.4ms pool_left=1 ready_left=1
[ 13267.629 pid=54500] claim(split): pane=33 spare_age=2055.7ms pool_left=1 ready_left=0
[ 13535.188 pid=54500] pool: refill scheduled pane=42 (depth=1 ready=0 inflight=1 target=2 surging=false)
```

Six settled spares were killed 103 ms after one split and 160 ms before the
next, with the user still splitting. From there the pool alternates depth 2/1
and ready 1/0, and the claims are served by spares 460 to 470 ms old that have
not finished starting:

```
[ 15851.468 pid=54500] claim(split): pane=46 spare_age=462.4ms pool_left=1 ready_left=0
[ 16931.694 pid=54500] claim(split): pane=49 spare_age=467.7ms pool_left=1 ready_left=0
```

Trace summary, unfixed: mid run trims 1, minimum sampled depth 1, median spare
age at claim 838.8 ms (run 1) and 1188.3 ms (run 2).

## 2. Causal chain

`WarmPool::note_claim` (src/types.rs:650) wrote `surge_until` only for a claim
that MISSED, so a run the surge was serving well renewed nothing and
`WARM_SURGE_HOLD` counted down from a miss six seconds in the past; five
seconds after that one miss `trim_surplus` (src/types.rs:703, called from the
server loop at src/server/mod.rs:1555) killed six ready spares in the middle of
a run of splits arriving every 265 ms, and the pool fell back to alternating one
ready spare with none, which is the bimodal creation the pool exists to remove.

## 3. Fix (547a5f5)

One function, `WarmPool::note_claim`. Given a claim that follows another within
`WARM_BURST_WINDOW`, the surge window is now opened or renewed by three things
instead of one:

* a miss, as before;
* a claim that takes the LAST READY spare, because the next creation of that run
  is then certain to miss and a spare needs ~400 ms to become useful, so
  reacting on the miss is reacting one creation too late;
* the run CONTINUING while a surge is serving it, so the window decays from the
  last CREATION, which is what `WARM_SURGE_HOLD`'s own doc comment always
  described.

Resource discipline is unchanged and was measured, not assumed: nothing renews
the window once the user stops creating panes. In `warm_idle.log` the last
creation is at 7498.4 ms and

```
[ 12596.862 pid=51524] pool: surge over, trimmed 6 surplus spare(s) back to target=2
```

is 5.1 s later, after which the samples read `depth=2 ready=2 target=2 eff=2
surging=false` for the remaining 7.5 s of idle. The surge ceiling
(`WARM_POOL_SURGE_MAX`), the standby cap (`effective_target(standby)`),
`warm-pool-size 0` / `PSMUX_NO_WARM`, `trim_to`, `reap_dead`, `kill_all` and
every existing teardown path are untouched.

## 4. Before and after, same harness, same tree

40 `split-window -v`, 250 ms apart, one live session, one attached client, pool
warmed first. Two runs each.

| build | run | p50 | p90 | p99 | max | splits > 150 ms | claims that waited on a warming spare | mid run trims | min pool depth |
|---|---|---|---|---|---|---|---|---|---|
| unfixed | pre1 | 125.6 | 482.7 | 581.6 | 581.6 | 17 of 40 | 1 | 1 (6 spares) | 1 |
| unfixed | pre2 | 15.5 | 202.0 | 266.0 | 266.0 | 5 of 40 | 2 | 1 (5 spares) | 1 |
| fixed | post1 | 16.6 | 18.7 | 326.5 | 326.5 | 1 of 40 | 1 | 0 | 2 |
| fixed | post2 | 14.0 | 16.3 | 19.1 | 19.1 | 0 of 40 | 0 | 0 | 2 |

Median spare age at claim: 838.8 / 1188.3 ms unfixed, 2081.9 / 2094.0 ms fixed.
Surging in 19 and 25 of the depth samples unfixed, 38 and 39 fixed.

### Perf gates

`tests\test_creation_latency_gate.ps1` (its own namespace, detached):

| build | new-window | split -v | split -h | result |
|---|---|---|---|---|
| unfixed | med 29 p90 92 max 564 | med 17 p90 188 max 206 | med 17 p90 94 max 141 | 10 passed 0 failed |
| unfixed | med 30 p90 120 max 581 | med 17 p90 191 max 204 | med 17 p90 174 max 237 | 10 passed 0 failed |
| fixed | med 30 p90 107 max 732 | med 33 p90 440 max 685 | med 17 p90 34 max 878 | 9 passed 1 failed |
| fixed | med 30 p90 121 max 576 | med 17 p90 173 max 234 | med 17 p90 96 max 173 | 10 passed 0 failed |

The one failure is machine noise, not the change: another agent's test run was
building and running psmux on this machine throughout (its worktree
`agent-ad04093410a9c6fca`), the second fixed run of the same gate is within a
few ms of both unfixed runs, and the 40 split runs above show the fixed build
strictly better on the shape this gate is a proxy for. Sessions dying mid suite
were seen on BOTH builds during the same window.

`tests\test_pane_startup_perf.ps1`: unfixed 27/27 (new window avg 197 ms, split
avg 52 ms, new session avg 624 ms); fixed 27/27 (new window avg 198 ms, split
avg 45 ms, new session avg 540 ms). One earlier fixed run scored 26/27 when its
session was killed mid stress test; it passed 27/27 on repeat.

### Resources, from the gate's JSON (`resources` block)

| build | server ws at prompt | server private at prompt | server ws after 23 panes | server private after 23 panes | idle CPU one pane | idle CPU 23 panes |
|---|---|---|---|---|---|---|
| unfixed | 15.95 / 15.95 MB | 4.30 / 4.33 MB | 25.71 / 25.70 MB | 16.57 / 16.77 MB | 0.00 / 0.00 % | 2.08 / 3.12 % |
| fixed | 15.92 / 15.85 MB | 4.31 / 4.19 MB | 26.35 / 25.91 MB | 17.40 / 16.83 MB | 1.04 / 2.07 % | 1.04 / 1.56 % |

Idle memory is unchanged and idle CPU sits in the same 0 to 3 % sampling noise
band on both builds; the sampler measures a 3 s window at 15.6 ms resolution, so
one tick is ~0.5 %.

## 5. Tests

* `tests-rs\test_issue661_warm_pool_depth.rs`, registered in
  src/warm_pane_sync.rs with `#[cfg(test)] #[path] mod`: 7 tests, all passing.
  Opens (last ready spare taken), does not open (a lone claim that empties the
  pool, ie a cold new-session), holds (a run the surge is serving), stops
  renewing (a gap longer than the burst window), releases (surplus back once the
  run stops), plus the standby cap and a disabled pool under a long run.
* `tests\test_issue661_warm_pool_depth_holds.ps1`: 24 splits 250 ms apart, reads
  the pool's own trace. Unfixed build 4 passed 2 failed (one mid run trim of 4
  spares; 3 of 24 splits over 150 ms, p90 166.9, max 387.5). Fixed build 6
  passed 0 failed twice (no trim, minimum depth 7 and 8 in the second half of
  the run, 0 and 1 of 24 over 150 ms, max 82.9 and 158.9 ms).

Counts on the fixed build:

| suite | result |
|---|---|
| `cargo test --bin psmux issue661` | 7 passed, 0 failed |
| `cargo test --bin psmux warm` | 126 passed, 0 failed |
| `cargo test --no-run` | exit 0 |
| test_issue661_warm_pool_depth_holds.ps1 | 6 passed, 0 failed |
| test_issue459_hook_accumulation.ps1 | 3 passed, 0 failed (2/1 on an interfered run) |
| test_issue615_wsl_pane_path.ps1 | 26 passed, 0 failed |
| test_issue120_warm_server_leak.ps1 | 3 passed, 0 failed |
| test_warm_pane.ps1 | 17 passed, 0 failed |
| test_warm_off.ps1 | 34 passed, 0 failed (33/1 on an interfered run) |
| test_warm_session_claim.ps1 | 10 passed, 0 failed |
| test_warm_selfheal_startup.ps1 | 10 passed, 0 failed |
| test_warm_chain_runaway.ps1 | 4 passed, 0 failed |
| test_warm_pane_start_dir.ps1 | 5 passed, 0 failed |
| test_warm_pane_sync_options.ps1 | 8 passed, 0 failed |
| test_issue271_warm_pane_history.ps1 | 11 passed, 0 failed |
| test_issue271_warm_pane_history_proof.ps1 | 5 passed, 0 failed |
| test_issue450_dead_warm_pane.ps1 | 10 passed, 0 failed (2/8 and 5/5 on two interfered runs) |
| test_issue659_warm_claim_environment.ps1 | 8 passed, 0 failed |

## 6. Commits

* 396a56c trace(#661): sample the spare pool's depth four times a second
* 547a5f5 fix(#661): hold the spare pool's depth for as long as the run lasts
* 62a0b2a test(#661): pin the surge window's open, hold and release rules

## 7. Leftovers

Zero. `Get-CimInstance Win32_Process` filtered on an executable path under this
worktree returns nothing, no `i661*` files remain in `%USERPROFILE%\.psmux`,
every client this work started was closed by pid, and three orphaned spare
shells (parent process gone, psmux spare command line) left behind by servers
that died during the noisy window were ended BY PID. One unrelated orphan
(`pwsh -NoProfile -Command htop`) was left alone: no suite run here launches
htop, so it belongs to another agent.
