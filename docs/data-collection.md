# Gathering Phase 0 data from another machine

The measurements that actually test rwr's thesis want a large private codebase, which cannot
be copied here. `rwr-phase0` runs on that machine instead and emits a JSON report you can hand
back.

## What it does and does not emit

**Emits:** file and byte counts, parse timings, unparsed-file counts, call-site totals, the
distribution of receiver shapes, and per-identifier statistics for the ~60 most-called names.

**Never emits:** source text, file contents, file paths below the corpus root, or anything
identifying beyond the corpus directory's own name. The report is aggregates only, so it can
be shared without leaking the code it measured. Skim it before sending if you like — it is
small and readable.

## Install

Needs Rust. If the machine lacks it: `brew install rustup && rustup-init -y`.

```sh
cargo install --git https://github.com/dpep/rwr
```

That installs both `rwr` and `rwr-phase0` into `~/.cargo/bin`. Re-run to update.

*(A Homebrew formula is deliberately not shipped yet: it wants a tagged release, and tagging
before Phase 0 has reported would be committing to a tool that Phase 0 might still say should
not exist.)*

## Run

```sh
rwr-phase0 --label monolith ~/path/to/monolith > phase0-monolith.json
```

Several corpora at once is fine, and each is reported separately:

```sh
rwr-phase0 --label laptop-b ~/src/monolith ~/src/other-app > phase0-laptop-b.json
```

Expect it to take roughly a second per 50 MB of Ruby on 8 cores.

## What each measurement decides

| Field | Feeds | Decides |
|---|---|---|
| `parse_ms`, `bytes` | (d) cold parse throughput | whether Phase 1 needs any persistence. Already answered *no* on public corpora; a 1M-LOC monolith is the real test |
| `unparsed` | D1 fidelity | whether Prism has gaps on real proprietary Ruby. Zero across 5,499 public application files so far |
| `receiver_shapes` | (c) receiver resolution | what fraction resolves with no type inference. ~59% on rails, and the monolith is the number that matters |
| `hot_names` | (b) bare-name collateral | how much damage a bare-name rename does, and — per D20 as amended — whether a name's *receiver-shape diversity* rather than its frequency is what makes it dangerous |

## What this still does not cover

Measurement **(a)**, the residue-reporting spike, is the one that tests the actual thesis and
it cannot be automated: it needs real renames with hand-verified ground truth, i.e. someone who
knows the codebase saying "these are the sites, and these are the ones a tool would miss."

If you want to seed it, pick three methods you have actually renamed — ideally one distinctive
name and one common Rails-shaped one — and note which call sites a bare-name search would have
got wrong.
