# Scaling

What rwr does when a repository is too large to parse whole, measured rather than assumed.

## The shape of the cost

```sh
rwr '$R.autoload_paths' <repo> --profile
```

| corpus | files | walk | scan | total |
|---|---:|---:|---:|---:|
| rails | 3,249 | 15 ms | 28 ms | **43 ms** |
| all local Ruby | 18,535 | 91 ms | 296 ms | **387 ms** |

The scan parses **21 files** in the second row. The other 18,514 are read, searched, and
skipped. So the cost is **reading and searching**, not parsing — which is the opposite of
what the design originally assumed, and it changes what is worth optimising.

## Parse fewer, not faster

A pattern naming `autoload_paths` cannot match a file whose bytes do not contain that text.
The prefilter (`src/pattern/prefilter.rs`) extracts the literals a pattern requires and skips
any file missing them, so **cost tracks how many files mention the identifier rather than how
large the repository is**.

On rails that is 18 files parsed instead of 3,249 — a 180x reduction, and it cut the scan
from 167 ms to 28 ms.

Two properties keep it honest:

- **Conservative by construction.** A file is skipped only when it provably cannot
  contribute, and a pattern with no literal text (`$A.$B`) filters nothing.
- **Residue is checked separately.** A match needs *every* required literal; residue needs
  only the anchor, and is reported from files a rule does *not* match — a declaration file,
  say. Checking those conjunctively would silently drop exactly the blind-spot report the
  design exists to produce.

## We are at grep speed, which is the floor

Same job — find the files containing the literal — across 18,535 files:

| tool | time |
|---|---:|
| `rg -l --type ruby autoload_paths` | 443 ms |
| `rwr '$R.autoload_paths'` | **262 ms** |

rwr is faster than ripgrep here *while also* parsing the survivors and matching structurally.

That is a floor, not a victory lap: anything that must look at every file cannot beat grep by
much. Going below it means **not touching the files at all**, which means an index.

## So: do we cache?

**Not yet, and the threshold is now a number rather than a feeling.** D5 cut persistence
because a cache was solving an unmeasured problem. It still is:

- At 18.5k files the whole run is 387 ms. A cache would save the read-and-search, perhaps
  200 ms.
- Extrapolating linearly, a repository 10x this size lands near 2.6 s — which *is* too slow to
  call in a loop.

So the honest position is that an index earns its place somewhere around **150k–200k Ruby
files**, and not before. Until a real corpus lands in that range, a cache buys a modest
constant and costs an invalidation bug class, a staleness surface, and a coherence design —
the things D5 was written to avoid.

**What would change this:** the monolith measurement. If it reports a corpus at that scale,
the index is justified by evidence rather than by anticipation.

## Leveraging Sorbet or Ruby LSP caches

**No — and this is a design decision, not a gap.**

Both maintain their own caches (`.ruby-lsp/`, Sorbet's cache directory), and both are
*private, versioned, internal formats*. Reading them would couple rwr to another tool's
implementation details, break on their upgrades, and produce silent wrongness when a format
drifted — the failure mode this design spends the most effort avoiding.

Their **outputs** are a different matter. Sorbet's RBI files are a public, stable, documented
format, and ingesting them for type information is already recorded in DESIGN.md's vision
section. That is the right seam: depend on what a tool publishes, never on what it caches.

## The hierarchy phase, and what profiling kept revealing

D52's hierarchy was the next bottleneck once the prefilter landed -- 64% of a run on the
local Ruby corpus. Fixing it took three attempts, and each one was wrong in an instructive
way:

| attempt | result | why |
|---|---|---|
| Filter files by `class` + `<` | 8,762 parsed instead of 18,535, **no faster** | parsing was not the cost |
| Only build the part reachable from the rule's class, via a worklist | **72** parsed, still no faster | the worklist re-read every file each round |
| Read once, share those bytes with the scan | **482 ms -> 12 ms** | reading was the cost, and two phases were each paying it |

The lesson generalises: every time the profiler was consulted, the bottleneck was **I/O, not
computation**, and every optimisation aimed at computation bought nothing. Parsing 72 files
instead of 8,762 changed the runtime by zero.

The worklist is worth keeping regardless of speed, because it is *exact*: a rename names one
class, only its descendants matter, and `Gold < Premium < Account` is reached in two rounds
because finding `Premium` puts that name into the next round's search set. Nothing is guessed
-- work is only deferred until a name is known to matter.

### Where the time goes now

```
  walk              72.9    27%  18535 files
  read             179.2    66%  18535 files, 81.0 MB
  hierarchy         12.3     5%  72 parsed, 18463 skipped
  scan               6.9     3%  0 files changed
  total            271.4
```

Total fell from 753 ms to 271 ms. Structural work -- the part that is actually rwr's job --
is now **8% of the run**. The rest is discovering and reading files, which is the floor for
anything without an index.

## What is already done

| technique | status |
|---|---|
| Scope to a subdirectory | shipped — `rwr <pattern> app/models`, and rules are usually class-scoped anyway |
| Literal prefilter | shipped — 180x fewer files parsed on rails |
| Parallel file walk | shipped — walk fell from ~40 ms to ~15 ms on rails |
| SIMD substring search | shipped — `memchr::memmem`, finders built once per run |
| Parallel parse and match | shipped since Phase 1 (rayon) |
| Hierarchy built only on demand | shipped — an ad-hoc query pays nothing for D52 |
| Targeted hierarchy | shipped -- worklist parses only classes reachable from the rule's, 72 instead of 8,762 |
| One shared read pass | shipped -- hierarchy and scan no longer each pay full I/O |
| Persistent index | **deferred**, with a measured threshold above |
