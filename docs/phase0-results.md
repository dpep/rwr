# Phase 0 results

Running record of the measurements that decide whether rwr should exist. Each entry states
what was measured, on what, and what it settles.

Corpora are local checkouts under `~/code/lib/ruby`. Reproduce with:

```sh
cargo test --test phase0_parse -- --nocapture
```

---

## (d) Cold parse throughput — **measured**

8 threads, Apple Silicon, warm page cache.

| repo | files | MB | ms | MB/s | unparsed |
|---|---:|---:|---:|---:|---:|
| rails | 3,254 | 16.2 | 188 | 86 | 0 |
| rubocop | 1,531 | 7.5 | 85 | 88 | 0 |
| graphql | 714 | 3.4 | 39 | 88 | 0 |
| ruby (CRuby) | 7,858 | 34.8 | 360-414 | 84-97 | 3 |
| **total** | **13,357** | **62.0** | **612-726** | **85-101** | **3** |

Two runs varied 612-726 ms; both had a warm page cache, so treat ~85-100 MB/s as the range
rather than either endpoint as a figure. Cold-cache numbers will be worse and are not yet
measured.

### This settles D5: Phase 1 ships no persistence

Rails — 3,254 files, 16 MB — parses **in under 200 ms**. A 1M-LOC monolith is roughly 35 MB,
so a full cold parse lands near half a second on this hardware.

That is fast enough that a parse cache would be solving a problem which does not exist. D5 was
amended to cut the cache on the *argument* that cache-vs-nothing was the real comparison and
had never been measured; this is the measurement, and it agrees. No cache, no index, no
invalidation bug class, no coherence surface.

The staff-engineer review predicted this outcome and it was right.

## D1 fidelity — **measured, and stronger than claimed**

**Zero parse failures across 5,499 application-shaped Ruby files** (rails, rubocop, graphql).

Three failures in CRuby's own tree, all explained, none a Prism gap:

| file | why |
|---|---|
| `spec/ruby/command_line/fixtures/bad_syntax.rb` | deliberately invalid — that is the fixture's purpose |
| `test/ruby/test_call.rb` | CRuby's error-path test suite |
| `tool/fetch-bundled_gems.rb` | `#!ruby -an` — the `-n` flag wraps the file in an implicit `while gets` loop at runtime, so its top-level `next` is not valid standalone Ruby. Prism is correct to reject it. |

DESIGN.md §3 argues Prism over tree-sitter on fidelity for *valid* Ruby. This is the first
evidence: across 13,357 files of real-world Ruby — including CRuby's own source, which
exercises syntax no application ever will — Prism's only rejections are files that genuinely
are not valid standalone Ruby.

It does not yet prove the converse (that tree-sitter's Ruby grammar *would* differ), which
needs a direct comparison. But it removes the possibility that Prism itself has gaps here.

---

## Still outstanding

| measurement | status | needs |
|---|---|---|
| (a) residue-reporting spike | not started | a matcher, plus real renames with hand-verified ground truth |
| (b) bare-`foo(...)` false-positive rate | **now possible** — rails is a usable corpus | a matcher |
| (c) receiver resolution for methods, no Sorbet | not started | symbol index |
| syntactic-partition scoring vs incumbents | not started | competitor binaries installed |

(b) was previously blocked on the author's private monolith. Rails is a legitimate public
substitute, and being public it makes the result reproducible — which for an OSS tool
positioned against an incumbent is worth more than a bigger private number.
