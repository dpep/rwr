# UX research — designing rwr for three audiences

**What this is.** `rwr` is a Ruby structural search-and-rewrite CLI — `rg`/`sed` for Ruby
*programs* rather than Ruby *text*. It has three distinct consumers, and they pull in
different directions: a **human at a terminal**, a **coding agent in a loop**, and
**automated CI / git-hook** runs. This document is the evidence base for making one surface
serve all three.

It covers four questions:

1. Ruby LSP shipped an official Claude Code integration in March 2026. How much of rwr's
   planned semantic layer (Phase 2) does it already cover?
2. If rwr ships an MCP server, what should it expose?
3. What concrete UX mechanisms should rwr copy, and from whom?
4. Where do the three audiences actually conflict, and how is each conflict resolved?

Companion docs: `DESIGN.md`, `docs/cli-conventions.md`, `docs/prior-art.md`,
`docs/decisions.md`, `docs/open-questions.md`. This document **answers Q9** in
`open-questions.md` and **challenges three decisions** recorded in `cli-conventions.md`
(exit-code layout, write-by-default, and `--json` as a homogeneous array) — see §3.

Everything here is tied to a source that was read, not recalled. Claims that could not be
verified are marked as such in §6.

---

## 1. Ruby LSP assessment

### 1.1 Two things get conflated

- **Shopify's `ruby-lsp` gem** — a full language server. It implements `references`,
  `rename`, `prepare_rename`, `code_actions`, `diagnostics`, `definition`, `hover`,
  `workspace_symbol` and more
  ([request handlers](https://github.com/Shopify/ruby-lsp/tree/main/lib/ruby_lsp/requests)).
- **The `ruby-lsp` Claude Code plugin** (merged Mar 2026,
  [PR #106](https://github.com/anthropics/claude-plugins-official/pull/106)) — nine lines of
  config naming a binary. Verbatim from
  [`marketplace.json`](https://github.com/anthropics/claude-plugins-official/blob/main/.claude-plugin/marketplace.json):

```json
{
  "name": "ruby-lsp",
  "description": "Ruby language server for code intelligence and analysis",
  "strict": false,
  "lspServers": {
    "ruby-lsp": {
      "command": "ruby-lsp",
      "extensionToLanguage": {
        ".rb": "ruby", ".rake": "ruby", ".gemspec": "ruby", ".ru": "ruby", ".erb": "erb"
      }
    }
  }
}
```

The plugin contributes **no tools of its own.** Everything an agent can reach goes through
Claude Code's single built-in `LSP` tool.

### 1.2 What the agent can actually call

One uniform shape for every operation:

```
LSP({ operation, filePath, line, character })
```

`operation` is one of exactly nine, all navigational: `goToDefinition`, `findReferences`,
`hover`, `documentSymbol`, `workspaceSymbol`, `goToImplementation`, `prepareCallHierarchy`,
`incomingCalls`, `outgoingCalls`
([tools reference](https://code.claude.com/docs/en/tools-reference),
[issue #40282](https://github.com/anthropics/claude-code/issues/40282)).

**`rename` is not among them. Neither is `codeAction`.** They are an open feature request
filed 28 Mar 2026, whose author describes the current agent workaround as *"Grep-based
find-and-replace, which is fragile."*

The plugin's marketing page claims otherwise.
[claude.com/plugins/ruby-lsp](https://claude.com/plugins/ruby-lsp) advertises *"find
references, workspace-wide symbol search, **rename symbol**, extract-to-variable/method
refactorings, and intelligent code actions for quick fixes."* Everything after
"workspace-wide symbol search" describes the *language server's* capability, reachable from
VS Code — not anything the agent can invoke.

**It locates. It does not rewrite.** There is no path from an agent tool call to a
`WorkspaceEdit` being applied.

### 1.3 The uniform-parameter design is itself a bug source

All nine operations share `{filePath, line, character}`. That shape is wrong for at least one
of them: `workspace/symbol` needs a `query` string, so the bridge always sent an empty query
and `workspaceSymbol` returned nothing at all
([issue #30948](https://github.com/anthropics/claude-code/issues/30948), since closed).

It is also wrong for the agent's actual workflow. An agent that wants "every reference to
`calculate_payroll_tax`" does not have a cursor position — it has a **name**. It must grep
first, convert to a line/column, and only then ask. The tool is shaped for an editor cursor,
not a query.

**rwr takes a pattern. That is a genuine interface advantage, not a cosmetic one.**

### 1.4 What `findReferences` actually does for a method

This is the load-bearing finding. It is documented nowhere; it is only visible in
[`reference_finder.rb`](https://github.com/Shopify/ruby-lsp/blob/main/lib/ruby_indexer/lib/ruby_indexer/reference_finder.rb).

Ruby LSP has three reference target kinds, wildly uneven in quality:

| Target | Resolution | Quality |
|---|---|---|
| `ConstTarget` | Resolves against lexical nesting via the index, filters to entries whose FQN matches | Genuinely semantic |
| `InstanceVariableTarget` | Narrowed by `linearized_ancestors_of(receiver_type)` | Genuinely semantic |
| `MethodTarget` | **`MethodTarget.new(node.name.to_s)`** — a bare string | Name equality, nothing else |

The method matcher, verbatim:

```ruby
def on_call_node_enter(node)
  if @target.is_a?(MethodTarget) && (name = node.name.to_s) == @target.method_name
    @references << Reference.new(name, node.message_loc, declaration: false)
```

No receiver check. No arity check. No defining-class narrowing. **"Find all references to
`save`" returns every `.save` in the repository**, regardless of receiver. For method
symbols, Ruby LSP's find-all-references is `rg -w save` filtered to AST call-node and def-node
positions — better than raw grep because it skips strings and comments, and worse than grep
because it *stops* there.

That is precisely the capability `DESIGN.md` §6 assigns to rwr Phase 2 (receiver-narrowing),
and Ruby LSP does not have it for methods.

### 1.5 The "handles metaprogramming" claim does not survive reading the code

The most-cited writeup ([Damian Galarza, 13 Mar 2026](https://www.damiangalarza.com/posts/2026-03-13-ruby-lsp-claude-code/))
says find-references works *"with dynamic calls included, because the LSP understands Ruby's
metaprogramming patterns well enough to handle common cases."*

`ReferenceFinder` registers visitors for constant nodes, instance-variable nodes, `DefNode`
and `CallNode`. It registers **no visitor for `SymbolNode` or any string node**. It therefore
cannot see `send(:foo)`, `public_send`, `define_method(:foo)`, `alias_method :new, :foo`,
`delegate :foo, to:`, or a symbol in a literal array fed to a macro. **Dynamic call sites are
invisible to it, full stop.**

What *is* true is narrower: the **index** knows about declarations produced by a fixed
whitelist of macros, dispatched in
[`declaration_listener.rb#on_call_node_enter`](https://github.com/Shopify/ruby-lsp/blob/main/lib/ruby_indexer/lib/ruby_indexer/declaration_listener.rb):
`private_constant`, `attr_reader`, `attr_writer`, `attr_accessor`, `attr`, `alias_method`,
`include`, `prepend`, `extend`, `public`, `protected`, `private`, `module_function`,
`private_class_method` — plus whatever add-ons register via the `@enhancements` hook (this is
how `ruby-lsp-rails` teaches it `belongs_to`). **`define_method`, `send`, `class_eval`,
`const_get` and `method_missing` are absent.**

So: it knows some metaprogrammed methods *exist*. It cannot find where they are *called*.
Galarza's own article contains the counter-evidence — the `documentSymbol` result *"notably
omitted the `acts_as_tenant` call and the enum values — it only surfaced Rails macro calls it
recognized."*

**And it never says so.** No residue report, no skip reason, no "I saw a `send` I could not
resolve." The indexer does accumulate `@indexing_errors`, but those surface through
`ruby-lsp --doctor` and editor notifications, not through the `LSP` tool. The agent gets a
list of locations and no account of what was missed. That is `DESIGN.md` §4's entire thesis,
unoccupied.

### 1.6 `rename` — even in the editor, it is constants only

Worth knowing for the "someday they'll expose rename" scenario.
[`rename.rb`](https://github.com/Shopify/ruby-lsp/blob/main/lib/ruby_lsp/requests/rename.rb) and
[`prepare_rename.rb`](https://github.com/Shopify/ruby-lsp/blob/main/lib/ruby_lsp/requests/prepare_rename.rb)
both locate with:

```ruby
node_types: [Prism::ConstantReadNode, Prism::ConstantPathNode, Prism::ConstantPathTargetNode]
```

Anything else returns `nil`. **You cannot rename a method with Ruby LSP.** It renames classes,
modules and constants — and, nicely, the files that hold them (`collect_file_renames`). It
refuses on collision: `raise InvalidNameError, "The new name is already in use by ..."`.

The same constants-only ceiling appears in Shopify's newer
[rubydex](https://github.com/shopify/rubydex) MCP server, whose reference tool is literally
named `find_constant_references`.

Even the optimistic future — Claude Code exposes `textDocument/rename` — buys **constant**
renames, the easy case. Method renames, signature changes and argument rewriting are untouched.

### 1.7 Operational cost

`ruby-lsp` is not a file-reading binary. Launching it runs
[`SetupBundler`](https://github.com/Shopify/ruby-lsp/blob/main/lib/ruby_lsp/setup_bundler.rb),
which materializes a **composed bundle** in `.ruby-lsp/` inside the repo, runs `bundle install`,
re-execs under `BUNDLE_GEMFILE=.ruby-lsp/Gemfile`, and refreshes every four hours. Declared
failure modes: `BundleNotLocked`, `BundleInstallFailure`. Consequences:

- **A Ruby runtime, a resolvable `Gemfile.lock`, and a working `bundle install` are hard
  requirements.** A half-migrated bundle means no code intelligence at all.
- **It writes into the repository** — a `.ruby-lsp/` directory appears.

Indexing is not free either. The
[indexer config](https://github.com/Shopify/ruby-lsp/blob/main/lib/ruby_indexer/lib/ruby_indexer/configuration.rb)
indexes the workspace *plus every gem in the lockfile plus default gems*. Reported figures:
[~3-5 files/sec, "about an hour" for a Rails app with 25-30 dependencies](https://github.com/Shopify/ruby-lsp/issues/1316);
[indexing "hangs forever" at 0%](https://github.com/Shopify/ruby-lsp/issues/2288);
[35 seconds for a single find-references on a 40,190-file repo](https://github.com/Shopify/ruby-lsp/issues/3051)
(closed as stale, not fixed). At rwr's stated target of >1M LOC, a 35-second read-only query is
disqualifying inside an agent loop.

One more trap, from the same config file — the index **excludes test and spec files by
default**:

```ruby
@excluded_patterns = [
  "**/{test,spec}/**/{*_test.rb,test_*.rb,*_spec.rb}",
  "**/fixtures/**/*",
]
```

(only `test_case.rb` / `test_helper.rb` are carved back in). So `workspaceSymbol`,
`goToDefinition` and `hover` are blind to specs. `references` escapes this only because it does
its own independent `Dir.glob("**/*.rb")` — which in turn means references does **not** respect
`.gitignore`, and will happily walk `vendor/` and `node_modules/`.

Reliability is not settled either. [Issue #44767](https://github.com/anthropics/claude-code/issues/44767)
reports `goToDefinition`, `findReferences` and `workspaceSymbol` returning empty at cursor
positions where `hover` succeeds, because the bridge does not wait for the server to finish
loading before reading a response. The reporter's own workaround was to write an MCP server
wrapping the language server with correct initialization timing. Empty results are returned as
prose — `"No references found"` — **indistinguishable from a true negative.** For an agent, a
silent false negative on a rename is the worst possible failure.

### 1.8 Honest read: how much of Phase 2 does this cover?

| rwr capability | Ruby LSP status |
|---|---|
| Locate constant references, scope-resolved | **Covered, and well.** Do not rebuild. |
| Locate instance-variable references, ancestor-narrowed | **Covered.** |
| Locate *method* call sites | Covered only as name-equality; **no receiver narrowing** |
| Narrow method call sites by receiver type (Phase 2's differentiator) | **Not covered** |
| Structural pattern match (`foo($A, $B)` with constraints) | Not attempted — it is not a query engine |
| Rewrite from a pattern | **Not reachable by an agent at all**; even in-editor it is constants-only |
| Report what it could not see (residue) | **Nothing.** No skip taxonomy, no dynamic-dispatch account |
| Atomicity / overlap detection | N/A — no rewrite |
| Run without a Ruby runtime, Gemfile or index | No on all three |
| Sub-second on 1M LOC | No (35s find-refs at 40k files) |

**Conclusion: the integration is materially thinner than its billing, and it does not displace
Phase 2.** What it *does* do is remove one job from rwr's plate — constant navigation — which
rwr should treat as solved and not duplicate.

Two consequences for the plan:

- **Positioning shifts from "we do references" to "we do rewriting, and we tell you what we
  missed."** Ruby LSP owns navigate-to-constant. rwr's honest pitch is pattern-shaped queries an
  agent can express without a cursor, receiver narrowing for *methods*, the transactional
  rewrite, and the residue report.
- **The residue contract gets *more* valuable, not less.** The failure mode an agent hits today
  is a confident, silent, incomplete answer. `DESIGN.md` §4 is the antidote, and §9.4's
  "unconditionally, never behind `--verbose`" is the right call.

**One risk worth naming.** Anthropic exposing `textDocument/rename` and `codeAction` is a
plausible near-term move (#40282 is open and labelled `enhancement`). If it lands, agents get
constant renames for free. That is a reason to make sure Phase 0 measurement (c) —
receiver resolution for **methods** — is the thing rwr proves, since it is the part no plausible
LSP exposure covers.

---

## 2. MCP design recommendation

### 2.1 What the incumbents actually expose

**[ast-grep MCP](https://github.com/ast-grep/ast-grep-mcp)** — four tools, all read-only, none
annotated. The maintainer states in [issue #7](https://github.com/ast-grep/ast-grep-mcp/issues/7):
*"no, this repo is not under active development now."*

```python
dump_syntax_tree(code, language, format: "pattern"|"cst"|"ast" = "cst") -> str
    # ast-grep run --pattern <code> --lang <lang> --debug-query=<format>; returns stderr
test_match_code_rule(code, yaml) -> List[dict]
    # ast-grep scan --inline-rules <yaml> --json --stdin
find_code(project_folder, pattern, language="", max_results=0, output_format="text")
find_code_by_rule(project_folder, yaml, max_results=0, output_format="text")
```

Default output is hand-rolled prose (`"Found 2 matches:\n\npath:10-15\n<source>"`), which the
README claims is **~75% fewer tokens** than JSON. JSON mode returns ast-grep's raw
`--json=stream` objects **with no envelope** — no total, no truncation flag, no skipped-file
list. Empty result is the bare string `"No matches found"`.

**[Semgrep MCP](https://github.com/semgrep/semgrep/blob/develop/cli/src/semgrep/mcp/server.py)** —
the standalone [semgrep/mcp](https://github.com/semgrep/mcp) repo is now a single
`deprecation_notice()` tool; the live server is `semgrep mcp` inside the binary. Nine tools:
`semgrep_scan`, `semgrep_scan_remote`, `semgrep_scan_with_custom_rule`,
`get_abstract_syntax_tree`, `semgrep_rule_schema`, `get_supported_languages`,
`semgrep_findings`, `semgrep_scan_supply_chain`, `semgrep_whoami`.

Its result model is the shape rwr wants and ast-grep lacks
([models.py](https://github.com/semgrep/semgrep/blob/develop/cli/src/semgrep/mcp/models.py)):

```python
class SemgrepScanResult(BaseModel):
    version: str
    results: list[dict[str, Any]]
    errors: list[dict[str, Any]]        = Field(default_factory=list)
    paths: dict[str, Any]               # carries `scanned` and `skipped`
    skipped_rules: list[str]            = Field(default_factory=list)
```

That is the *mechanical* half of residue reporting, already standardized — "I could not process
these bytes." rwr's claim is that the *semantic* half ("this file scanned fine and there is still
a `send(name)` on line 47") does not exist anywhere. **That claim holds.**

Semgrep ships **no `ToolAnnotations`, no `outputSchema`, and no truncation or pagination on any
scan tool.** The only limit in the whole server is `LIMIT_FIELD = Field(default=10)` on
`semgrep_findings`, which is remote-API pagination, not a local-scan cap. It *does* ship
elicitation — one prompt per finding, three-way enum (`true_positive` / `false_positive` / `skip`)
— gated behind a feature flag.

**[Shopify rubydex](https://github.com/shopify/rubydex)** — six tools, all read-only:
`search_declarations`, `get_declaration`, `get_descendants`, `find_constant_references`,
`get_file_declarations`, `codebase_stats`. Its
[`base_tool.rb`](https://github.com/Shopify/rubydex/blob/main/lib/rubydex/mcp_server/tools/base_tool.rb)
is the best small-scale reference in the survey — see §3.

**[Serena](https://github.com/oraios/serena)** (52 tools, LSP-backed) is the closest thing to a
precedent for rwr's write path, and it arrived at rwr's plan/apply contract independently. From
`replace_in_files`, verbatim:

> Recommended protocol whenever there is ANY risk of unintended replacements:
> 1. Call with `dry_run=True`: every prospective change is returned as a minimal line diff with an
>    occurrence id; nothing is modified.
> 2. Call again with `dry_run=False`, passing the ids you want in `occurrence_ids` (omit it to
>    apply all). You pick the desired replacements from the list — no counting, no needle-crafting.
>
> For clearly unambiguous bulk replacements you may skip the dry run; pass `expected_count` as a
> guard. If the actual number of matches differs, NOTHING is changed and the diff list is returned.

The occurrence id is **content-addressed** — `'<path>:<index>@<digest>'` — and staleness is
diagnosed by kind (`malformed id` / `no matches in this file` / `fewer matches than at dry-run
time` / `matched text changed`). It even has rwr's overlap refusal under a different name: an
occurrence is *ambiguous* when the pattern matches again inside its own match.

### 2.2 The large-result-set finding

**No structural-codemod MCP server handles this well, and one maintainer rejected the fix on
principle.**

The concrete failure, from [ast-grep-mcp issue #13](https://github.com/ast-grep/ast-grep-mcp/issues/13)
— a real Claude Code user, one structural query:

```
Error: MCP tool "find_code" response (194537 tokens) exceeds maximum allowed tokens (25000).
Please use pagination, filtering, or limit parameters to reduce the response size.
```

7.8x over the cap. The maintainer's response:

> **HerringtonDarkholme:** No, I don't want to introduce additional behaviors like pagination,
> which requires additional maintenance and LLM understanding the tool usage. I also don't see
> ripgrep tool has such behavior.

> **sebthom:** … **the client does not know in advance how long the result is** so from my view
> paging is the only option to handle this gracefully.

The resolution ([#14](https://github.com/ast-grep/ast-grep-mcp/issues/14),
[#31](https://github.com/ast-grep/ast-grep-mcp/issues/31)) was `max_results` plus `--json=stream`
with a count-don't-deserialize early exit — a good mechanism:

```python
def parse_matches(stdout, max_results=0):
    matches, total_lines = [], 0
    for line in stdout.splitlines():
        if not line or not line.startswith("{"): continue
        total_lines += 1
        if not max_results or len(matches) < max_results:
            matches.append(json.loads(line))
    return matches, total_lines
```

**But `max_results` defaults to `0`, meaning unlimited**, so out of the box it still blows the
cap. And in `output_format="json"` the truncation notice — which lives only in the text header —
is dropped, so a JSON caller gets a **silently** truncated list.

| Server | Result cap | Default | Truncation signalled? | Pagination |
|---|---|---|---|---|
| ast-grep MCP | `max_results` | **0 = unlimited** | text mode only, in prose | rejected by maintainer |
| Semgrep `semgrep_scan` | **none** | — | no | no |
| Semgrep `semgrep_findings` | `limit` | 10 | no | remote API only |
| Codemod `dump_ast` | **none** | — | no | no |
| rubydex | `limit`/`offset` per tool | 50 (max 100–200) | **yes — `total` alongside `results`** | **yes** |

**MCP gives you nothing at the protocol level.** Per the
[pagination spec](https://modelcontextprotocol.io/specification/2026-07-28/server/utilities/pagination),
the paginated operations are `resources/list`, `resources/templates/list`, `prompts/list`,
`tools/list`. **`tools/call` is not on the list.** Result pagination must be built into your own
`inputSchema`.

The one worked design that gets this right is
[ast-grep-mcp PR #36](https://github.com/ast-grep/ast-grep-mcp/pull/36) — 1,927 additions, **closed
unmerged** because the repo is dormant. Its summary reads like a spec for rwr:

> **MCP searches require finite limits; exhaustive scans remain a CLI responsibility.** JSON
> search output is wrapped as `matches`, `returned`, `truncated`, and `limit`.

```python
DEFAULT_MAX_RESULTS: Final = 50
HARD_MAX_RESULTS: Final = 500
class SearchResults(TypedDict):
    matches: list[dict[str, Any]]; returned: int; truncated: bool; limit: int
```

Serena adds the mechanism the caps lack — a **degradation ladder** rather than a hard cut
([`tools_base.py`](https://github.com/oraios/serena/blob/main/src/serena/tools/tools_base.py)):

```python
if (n_chars := len(result)) > max_answer_chars:
    too_long_msg = (f"The answer is too long ({n_chars} characters). "
                    "You can adjust your query or raise the max_answer_chars parameter.")
    if shortened_result_factories is not None:
        for make_shorter in shortened_result_factories:
            candidate = f"{too_long_msg}\n{make_shorter()}"
            if len(candidate) <= max_answer_chars:
                return candidate
    result = too_long_msg
```

`search_for_pattern` supplies five rungs: full lines → truncated lines → line numbers only →
per-file counts → summary. Default budget `150_000` chars.

### 2.3 "Dump the AST" tools: useful, but a resource beats a tool

Every structural-code MCP ships one. Direct evidence for their necessity is decent — from
[ast-grep's own blog on AI-generated rules](https://ast-grep.github.io/blog/ast-grep-agent.html):
**OpenAI O3 "hallucinated with wild abandon" and "invented syntax that looked more like CodeQL or
jscodeshift"**; **Gemini borrowed "syntax from a related but more established tool, Semgrep"**;
**Claude 4 "correctly identified ast-grep and produced syntactically valid rules" but "struggled
with subtle semantic details."**

But the fix was not the dump tool. It was a **procedure** — *"a simple, five-step plan: understand
the request, write example code, create a rule matching it, test against the example, then search
the codebase"* — which *"turned erratic geniuses into more reliable assistants."*

The failure mode is documented in
[issue #29, "llm seems struggle with the syntax?"](https://github.com/ast-grep/ast-grep-mcp/issues/29)
— a real Codex transcript:

```
• Called ast-grep.find_code({"pattern":"fn return_type_from_annotation($A)", ...})
  └ No matches found
• Called ast-grep.find_code({"pattern":"return_type_from_annotation", ...})
  └ Found 1 matches: .../solve.rs:4052
```

The pattern was wrong, the tool said **"No matches found"** and nothing else, and the agent
degraded to a bare-identifier search — it fell back to grep. **A silent zero is
indistinguishable from a correct negative.** This is the strongest single argument in the whole
survey for `DESIGN.md` principle 4 and for `--explain`: the tool had the information to say "your
pattern parsed as a call, not a definition" and said nothing.

On cost, [Codemod](https://github.com/codemod-com/codemod/tree/main/crates/mcp) is instructive. Its
`dump_ast` emits **kinds only** — no source text, no ranges — and it ships a precomputed grammar
summary instead. [`ruby.txt`](https://raw.githubusercontent.com/codemod-com/codemod/main/crates/mcp/src/data/node_types/ruby.txt)
is **103 lines / 6,595 bytes (~1.7k tokens)** for the *entire* Ruby node vocabulary:

```
if_guard: condition=_expression
do_block: body=body_statement?, parameters=block_parameters?
binary: left=_expression,_simple_numeric, right=_expression
```

**Conclusion:** the whole Ruby node vocabulary fits in ~1.7k tokens, once. Repeatedly dumping
trees of real code costs more and teaches less. Ship the vocabulary as an **MCP resource**; keep a
dump capability but scope it to *a pattern and a snippet side by side*, opt-in, and only once a
match has already failed. (No first-party telemetry on agent AST-dump usage exists in either
direction — flagged in §6.)

### 2.4 The space is empty

The survey found **zero** MCP servers for comby, jscodeshift or OpenRewrite, and no official
Moderne server. The entire structural-codemod MCP population is: ast-grep-mcp (read-only,
dormant), a one-tool Go wrapper [dgageot/mcp-ast-grep](https://github.com/dgageot/mcp-ast-grep),
and two LSP/Roslyn refactoring servers. `DESIGN.md` §8's *"that is an opening"* is understated —
the opening is the entire category.

**But there is a counter-argument worth recording.** From
[oh-my-openagent #5313](https://github.com/code-yeongyu/oh-my-openagent/issues/5313), proposing to
replace the ast-grep MCP with a bundled skill: the MCP wrapper is *"a thin, lossy slice over the
real `sg` CLI"* carrying *"its own bundled-runtime + install plumbing,"* while a skill gives *"the
full CLI surface … with better guidance."* ast-grep's own
[AI-tools guide](https://ast-grep.github.io/advanced/prompting.html) presents three tiers — CLI
prompting for everyday tasks, an `llms.txt` doc dump for context, and the MCP only for *"more
sophisticated and dedicated code analysis tasks."*

This is why nobody ships an apply tool: **the CLI already had one, and every MCP author decided the
write path wasn't worth wrapping.** rwr's MCP earns its keep only if it does something the CLI plus
a skill cannot — and the candidate is **the transaction** (plan → hash-checked apply), not the
search.

### 2.5 Protocol facts worth knowing

**Tool annotations.** [`schema/2026-07-28/schema.ts`](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/schema/2026-07-28/schema.ts),
byte-identical to 2025-06-18 apart from a typo fix:

```typescript
export interface ToolAnnotations {
  title?: string;
  readOnlyHint?: boolean;      // Default: false
  destructiveHint?: boolean;   // Default: true   (meaningful only when readOnlyHint == false)
  idempotentHint?: boolean;    // Default: false  (meaningful only when readOnlyHint == false)
  openWorldHint?: boolean;     // Default: true
}
```

with the caveat that *"all properties in `ToolAnnotations` are **hints**"* and clients *"MUST
consider tool annotations to be untrusted unless they come from trusted servers."*

**Are they respected?** In the installed Claude Code 2.1.237 binary, the MCP tool wrapper contains,
verbatim:

```js
isConcurrencySafe(){return I.annotations?.readOnlyHint??!1},
isReadOnly(){return I.annotations?.readOnlyHint??!1},
isDestructive(){return I.annotations?.destructiveHint??!1},
isOpenWorld(){return I.annotations?.openWorldHint??!1},
```

So `readOnlyHint` gates **tool-call parallelism** and read-only classification; `destructiveHint`
and `openWorldHint` are read; **`idempotentHint` is not read anywhere**; and Claude Code defaults
`destructiveHint` to `false` where the spec defaults it to `true` — so declare it explicitly.
Whether these change a *permission prompt* is unconfirmed (see §6); the docs and
[#87452](https://github.com/anthropics/claude-code/issues/87452) say they do not.
[VS Code definitively honors `readOnlyHint`](https://code.visualstudio.com/api/extension-guides/ai/mcp).

**Structured content.** 2025-06-18 introduced `structuredContent` + `outputSchema`; 2026-07-28
widened `structuredContent` from objects-only to any JSON value. The spec says a tool returning
structured content **SHOULD** also return serialized JSON in a `TextContent` block. Claude Code
validates `outputSchema` with Ajv, **2020-12 only** — a draft-07 `$schema` disables the whole
server ([#86142](https://github.com/anthropics/claude-code/issues/86142)).

Two open bugs matter: [#79944](https://github.com/anthropics/claude-code/issues/79944) — text
content block dropped when `structuredContent` is also present (works in Cursor) — and
[#86032](https://github.com/anthropics/claude-code/issues/86032) — error results drop
`structuredContent`. Serena ships the workaround, in
[`contexts/claude-code.yml`](https://github.com/oraios/serena/blob/main/src/serena/resources/config/contexts/claude-code.yml):

```yaml
# Claude Code has a bug (it does not unpack structured tool output, causing a lot of unnecessary escaping
# owing to the wrapped structure), so we disable it here.
structured_tool_output: false
```

**Result size limits in Claude Code**, cross-checked against the
[docs](https://code.claude.com/docs/en/mcp) and the binary:

| Knob | Value |
|---|---|
| Warning threshold | **10,000 tokens — fixed, not configurable** |
| Default cap | **25,000 tokens** (`MAX_MCP_OUTPUT_TOKENS` overrides) |
| Default per-tool char cap | **100,000 chars** |
| `_meta["anthropic/maxResultSizeChars"]` | up to **500,000 chars** |
| Built-in Grep tool, for calibration | `maxResultSizeChars: 20000` |

Tool descriptions and server instructions are truncated at **2KB each** — put the critical
sentence first.

**Elicitation** is supported in Claude Code (since v2.1.76), with `requestedSchema` restricted to
*"a restricted subset of JSON Schema. Only top-level properties are allowed, without nesting."*
Form elicitation over Streamable HTTP is reported broken
([#85442](https://github.com/anthropics/claude-code/issues/85442)), dialogs intermittently fail to
render ([#84207](https://github.com/anthropics/claude-code/issues/84207)), long messages don't
scroll ([#84602](https://github.com/anthropics/claude-code/issues/84602)) — which matters if you
wanted to show a diff. And the tool timeout runs while the human deliberates.

New in 2026-07-28: **`InputRequiredResult`** — a stateless "I need more input" result carrying an
opaque `requestState` blob the client passes back on retry. Claude Code 2.1.237 negotiates
`2026-07-28` and has the machinery. **rwr should still not use it for ambiguity**: rwr's contract
is *refuse rather than guess*, and converting a deterministic refusal into a live human judgment
call makes the result unreproducible and unrecorded. Reserve interactive confirmation for
*authorization* of an already-computed plan.

**`_meta` namespacing.** Per the [2026-07-28 basic spec](https://modelcontextprotocol.io/specification/2026-07-28/basic#meta),
prefixes must be reverse-DNS labels ending in `/`. rwr's keys should be
`com.github.dpep.rwr/...`.

### 2.6 Anthropic's tool-writing guidance

From [Writing effective tools for AI agents](https://www.anthropic.com/engineering/writing-tools-for-agents):

- *"More tools don't always lead to better outcomes."* — *"Build a few thoughtful tools targeting
  specific high-impact workflows."* The worked example: prefer one `schedule_event` over
  `list_users` + `list_events` + `create_event`.
- *"Tool implementations should take care to return only high signal information back to agents."*
- *"We suggest implementing some combination of pagination, range selection, filtering, and/or
  truncation with sensible default parameter values."*
- A `response_format` enum with `concise` / `detailed`; `concise` costs roughly **one third** the
  tokens.
- *"prompt-engineer your error responses to clearly communicate specific and actionable
  improvements, rather than opaque error codes or tracebacks."*
- *"think of how you would describe your tool to a new hire on your team."* … *"instead of a
  parameter named `user`, try a parameter named `user_id`."*

### 2.7 Proposed rwr MCP tool set

**Five tools, fat, verb-prefixed, split on the read/write boundary.** Few fat tools per
Anthropic — but **not** one fat `rwr(mode: …)` tool, for a reason specific to MCP: **annotations
are per-tool, not per-argument.** A single tool with a write mode must be annotated
`readOnlyHint: false, destructiveHint: true`, which permanently taints the read path — no parallel
execution, no plan-mode use, a confirmation prompt on every search. The read/write boundary is the
one axis the protocol can express.

This is also `DESIGN.md` principle 5 applied to the API surface: **make the unsafe operation
unrepresentable.** `rwr_apply` cannot be called without a `plan_id` that only `rwr_plan` mints.
Dry-run stops being a flag someone forgets and becomes the only path to a write.

| Tool | readOnly | destructive | idempotent | openWorld | Purpose |
|---|---|---|---|---|---|
| `rwr_find` | true | false | true | false | Bounded structural search + coverage + residue |
| `rwr_check` | true | false | true | false | Validate a rule; show trees; explain a non-match |
| `rwr_plan` | true | false | true | false | Compute the full edit plan. Touches nothing. |
| `rwr_apply` | **false** | **true** | **false** | false | Execute a plan atomically, digest-guarded |
| `rwr_info` | true | false | true | false | Caps, capabilities, coordinate conventions |

Plus two **resources**, not tools (following Codemod's migration of 12 `get_*_instructions` tools
to `jssg://instructions` resources):

- `rwr://grammar/ruby` — the Prism node vocabulary, ~1.7k tokens, loaded once on demand
- `rwr://pattern-syntax` — metavariables, min/max occurrence counts, the `where:` catalogue

On `destructiveHint` for `rwr_apply`: the spec says *"If false, the tool performs only additive
updates."* An in-place source rewrite is not additive. rwr's whole thesis is refusing to overstate
what it knows; understating the risk of its own write path would be off-brand. **Set it `true`.**
(Serena derives exactly this: `readOnlyHint = not can_edit`, `destructiveHint = can_edit`.)
`idempotentHint: false` on apply is load-bearing even though Claude Code ignores it: re-running the
same plan against already-rewritten files must fail the precondition, not silently no-op.

#### `rwr_find`

```json
{
  "name": "rwr_find",
  "title": "Find Ruby code by structure",
  "description": "Structural search over Ruby source using Prism. Returns bounded, complete-or-explicitly-truncated results, plus an account of files that could not be parsed and (for name-anchored patterns) occurrences of the target identifier the structural match did not cover. Read `rwr://pattern-syntax` for the pattern language. Use this instead of Grep when you care about structure rather than text; use Grep when you want text. For exhaustive repo-wide scans use the `rwr` CLI — this tool is deliberately bounded.",
  "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false },
  "inputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "required": ["pattern"],
    "additionalProperties": false,
    "properties": {
      "pattern": { "type": "string",
        "description": "Ruby source with $METAVARS, e.g. `foo($A, $B)`. Must parse as valid Ruby." },
      "where": { "type": "object",
        "description": "Constraints on captures. See rwr://pattern-syntax. Example: {\"keywords\":\"none\",\"receiver_type\":\"PayrollService\"}." },
      "paths": { "type": "array", "items": { "type": "string" }, "default": [],
        "description": "Repo-relative files or directories. Empty means the whole repo scope. Mirrors the CLI's --path." },
      "include_globs": { "type": "array", "items": { "type": "string" } },
      "exclude_globs": { "type": "array", "items": { "type": "string" },
        "description": "gitignore-style, without a leading '!'. .gitignore, vendor/, node_modules/ and db/schema.rb are excluded by default." },
      "max_results": { "type": "integer", "minimum": 1, "maximum": 500, "default": 50,
        "description": "Hard schema ceiling is 500. An operator may configure a lower cap; rwr_info reports it as max_results_cap and calls above it are rejected with the cap named." },
      "max_answer_chars": { "type": "integer", "minimum": 1000, "default": 60000,
        "description": "Output budget. When exceeded, results degrade in resolution rather than failing: full matches -> excerpts -> locations only -> per-file counts -> totals. `coverage` and `residue` never degrade." },
      "response_format": { "enum": ["concise", "detailed"], "default": "concise",
        "description": "concise: file:line-range plus matched source. detailed: adds capture bindings, byte offsets and nesting metadata. concise costs roughly a third as much." },
      "residue": { "type": "boolean", "default": true,
        "description": "For name-anchored patterns, enumerate remaining occurrences of the target identifier the structural match did not account for, classified by syntactic context. No effect on patterns with no target identifier." }
    }
  },
  "outputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "required": ["status", "matches", "returned", "total", "truncated", "limit", "coverage"],
    "properties": {
      "status": { "enum": ["ok", "no_matches"] },
      "matches": { "type": "array", "items": { "$ref": "#/$defs/match" } },
      "returned": { "type": "integer" },
      "total": { "type": "integer", "description": "Total matches found, counted even when not returned." },
      "truncated": { "type": "boolean" },
      "resolution": { "enum": ["full","excerpt","locations","counts","summary"],
        "description": "Which rung of the degradation ladder produced this result." },
      "limit": { "type": "integer" },
      "coverage": {
        "type": "object",
        "description": "What rwr could not see. Always present, never behind a verbosity flag, never degraded.",
        "required": ["files_scanned", "files_skipped"],
        "properties": {
          "files_scanned": { "type": "integer" },
          "files_skipped": { "type": "array", "items": {
            "type": "object",
            "required": ["file", "reason"],
            "properties": {
              "file": { "type": "string" },
              "reason": { "enum": ["parse_error","excluded_vendored","excluded_generated","gitignored","unreadable","not_ruby"] },
              "detail": { "type": "string", "description": "Prism diagnostic, verbatim, for parse_error." }
            }}}
        }},
      "residue": {
        "type": ["object", "null"],
        "description": "Absent (null) for patterns with no target identifier. Never degraded.",
        "properties": {
          "identifier": { "type": "string" },
          "enumerated": { "type": "boolean",
            "description": "false when the identifier is too common to enumerate honestly; `count` is still reported." },
          "count": { "type": "integer" },
          "note": { "type": "string", "description": "e.g. \"identifier too common; 4812 occurrences, not enumerated\"" },
          "occurrences": { "type": "array", "items": {
            "type": "object",
            "required": ["location", "kind"],
            "properties": {
              "location": { "$ref": "#/$defs/location" },
              "kind": { "enum": ["symbol_literal","string_literal","interpolation_fragment",
                                 "send_argument","define_method_argument","delegate_argument",
                                 "alias_method_argument","literal_array_element"] },
              "text": { "type": "string" }
            }}}
        }},
      "next_action": { "type": "string",
        "description": "One sentence naming the caller's best next step. Present on every non-ok status and on truncation." }
    },
    "$defs": {
      "location": { "type": "object",
        "required": ["file","line","col","byte_start","byte_end"],
        "properties": { "file":{"type":"string"}, "line":{"type":"integer"}, "col":{"type":"integer"},
                        "byte_start":{"type":"integer"}, "byte_end":{"type":"integer"} }},
      "match": { "type": "object",
        "required": ["location","text"],
        "properties": {
          "location": { "$ref": "#/$defs/location" },
          "text": { "type": "string" },
          "captures": { "type": "object", "additionalProperties": { "$ref": "#/$defs/capture" } },
          "nesting": { "type": "object",
            "properties": { "depth": {"type":"integer"}, "outermost": {"type":"boolean"} }}
        }},
      "capture": { "type": "object",
        "required": ["text","location"],
        "properties": { "text": {"type":"string"}, "location": {"$ref":"#/$defs/location"},
                        "node_kind": {"type":"string"} }}
    }
  }
}
```

Notes: `location` is the identical `{file, line, col, byte_start, byte_end}` shape
`cli-conventions.md` mandates — the MCP `outputSchema` and the CLI JSON should be the **same
schema generated from the same Rust types**, so they cannot drift. `coverage` is *required*, not
optional. `total` alongside `returned` is ast-grep's stream trick: count every match, deserialize
only the kept ones. `next_action` is Anthropic's "errors that teach" applied to non-errors — on
truncation: `"3,412 matches; 50 returned. Narrow with paths or where:, or run 'rwr find' in the
terminal for the full set."`

#### `rwr_check` — the whole rule-debugging loop in one call

Prior servers split this into two or three tools. But the ast-grep finding was that what fixed LLM
rule authoring was the **procedure** — example, rule, test, then search — and one tool that does
the middle two steps collapses two round trips into one and makes the silent-zero of issue #29
structurally impossible.

```json
{
  "name": "rwr_check",
  "title": "Validate a pattern against an example",
  "description": "Check that a pattern matches (or deliberately does not match) an example snippet, before running it over a repo. On a non-match, reports WHY: which constraint rejected it, and how rwr parsed the pattern versus the code. Always call this before rwr_find or rwr_plan on a pattern you have not used before.",
  "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false },
  "inputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "required": ["pattern", "code"],
    "additionalProperties": false,
    "properties": {
      "pattern": { "type": "string" },
      "where":   { "type": "object" },
      "rewrite": { "type": "string",
        "description": "Optional. If given, also shows the rewritten snippet. Nothing is written to disk." },
      "code":    { "type": "string", "description": "Ruby snippet to test against." },
      "expect":  { "enum": ["match", "no_match", "any"], "default": "match",
        "description": "Declare the intent so a negative probe is a pass, not a silent zero." },
      "show_tree": { "enum": ["none", "pattern", "code", "both"], "default": "none",
        "description": "Prism tree dump. Use 'both' only when a match fails and the diff between the two trees is the answer." }
    }
  },
  "outputSchema": {
    "type": "object",
    "required": ["status", "matched", "match_count"],
    "properties": {
      "status": { "enum": ["ok","unmet_expectation","pattern_parse_error","code_parse_error","invalid_constraint"] },
      "matched": { "type": "boolean" },
      "match_count": { "type": "integer" },
      "matches": { "type": "array", "items": { "$ref": "#/$defs/match" } },
      "rewritten": { "type": "string", "description": "Present only when `rewrite` was supplied." },
      "rejection": {
        "type": "array",
        "description": "Present when matched=false. Why each candidate node was rejected — the --explain surface.",
        "items": { "type": "object",
          "required": ["location", "reason"],
          "properties": {
            "location": { "$ref": "#/$defs/location" },
            "reason": { "enum": ["shape_mismatch","constraint_failed","metavar_arity","ast_inequality"] },
            "constraint": { "type": "string", "description": "Which key in `where:` rejected it." },
            "detail": { "type": "string", "description": "e.g. \"pattern parsed as CallNode; code node is DefNode\"" }
          }}},
      "trees": { "type": "object",
        "properties": { "pattern": {"type":"string"}, "code": {"type":"string"} }},
      "diagnostics": { "type": "array", "items": {"type":"string"},
        "description": "Prism diagnostics, verbatim." },
      "next_action": { "type": "string" }
    }
  }
}
```

`rejection[].detail` is the whole point: *"pattern parsed as CallNode; code node is DefNode"* is
the answer the Codex transcript needed and never got. `show_tree` defaults to `none` — the tree is
opt-in and only worth its tokens once a match has already failed.

#### `rwr_plan` — dry-run as a tool, not a flag

```json
{
  "name": "rwr_plan",
  "title": "Plan a Ruby rewrite (no files touched)",
  "description": "Compute the complete edit set for a rewrite and return it as a diff, without modifying anything. Reports overlaps, ambiguities and refusals up front. Returns a plan_id and per-edit ids that rwr_apply requires — there is no way to write without planning first. vs Edit/sed: those match text and will corrupt heredocs and nested calls; this matches Ruby structure and aborts rather than emitting unparseable output.",
  "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false },
  "inputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "required": ["pattern", "rewrite"],
    "additionalProperties": false,
    "properties": {
      "pattern": { "type": "string" },
      "where":   { "type": "object" },
      "rewrite": { "type": "string", "description": "Ruby source with $METAVARS. Empty string deletes the match." },
      "paths":   { "type": "array", "items": { "type": "string" }, "default": [] },
      "include_globs": { "type": "array", "items": { "type": "string" } },
      "exclude_globs": { "type": "array", "items": { "type": "string" } },
      "max_files": { "type": "integer", "minimum": 1, "maximum": 500, "default": 100,
        "description": "Refuse to plan a rewrite spanning more files than this, rather than returning a diff too large to review." },
      "max_answer_chars": { "type": "integer", "minimum": 1000, "default": 120000,
        "description": "Output budget. Degrades full diff -> per-hunk one-liners -> per-file edit counts -> totals. `refusals` and `coverage` never degrade." },
      "response_format": { "enum": ["concise", "detailed"], "default": "concise" },
      "on_conflict": { "enum": ["abort", "report"], "default": "abort",
        "description": "abort: any partial overlap refuses the whole plan. report: still refuses, but enumerates every conflict so you can see them all at once." }
    }
  },
  "outputSchema": {
    "type": "object",
    "required": ["status", "plan_id", "summary", "coverage"],
    "properties": {
      "status": { "enum": ["ok", "no_matches", "retryable", "refused"],
        "description": "ok: plan is applicable. no_matches: clean negative, stop. retryable: some matches sat inside a rewritten range; re-plan after applying and progress will be made. refused: ambiguity that re-running will not resolve." },
      "plan_id": { "type": ["string","null"],
        "description": "Opaque handle. Null unless status is ok or retryable. Expires; rwr_info reports plan_ttl_seconds." },
      "expires_at": { "type": ["string","null"], "format": "date-time" },
      "summary": { "type": "object",
        "required": ["matches","edits","files","conflicts"],
        "properties": {
          "matches": { "type": "integer" }, "edits": { "type": "integer" },
          "files": { "type": "integer" }, "conflicts": { "type": "integer" },
          "residual_matches": { "type": "integer",
            "description": "Matches suppressed because they were contained in a rewritten range. Non-zero implies status=retryable. rwr never auto-fixpoints." }
        }},
      "diff": { "type": "string", "description": "Unified diff of the whole plan." },
      "edits": { "type": "array", "items": {
        "type": "object",
        "required": ["edit_id","location","before","after"],
        "properties": {
          "edit_id": { "type": "string",
            "description": "Content-addressed: '<path>:<index>@<digest>' where digest covers the matched text. Pass a subset to rwr_apply to apply only those edits. Goes stale safely: if the matched text changed, that id fails and names why." },
          "location": { "$ref": "#/$defs/location" },
          "before": { "type": "string" }, "after": { "type": "string" }
        }}},
      "files": { "type": "array", "items": {
        "type": "object", "required": ["file","edits"],
        "properties": { "file": {"type":"string"}, "edits": {"type":"integer"}, "diff": {"type":"string"} }}},
      "refusals": { "type": "array", "items": {
        "type": "object",
        "required": ["location","kind","explanation"],
        "properties": {
          "location": { "$ref": "#/$defs/location" },
          "kind": { "enum": ["partial_overlap","crossing_deletions","different_replacements",
                             "swallowed_insertions","ambiguous_receiver","heredoc_detachment",
                             "reparse_mismatch"] },
          "explanation": { "type": "string", "description": "Why, in one sentence, in terms the caller can act on." },
          "conflicting_with": { "$ref": "#/$defs/location" }
        }}},
      "coverage": { "$ref": "#/$defs/coverage" },
      "residue": { "type": ["object","null"] },
      "next_action": { "type": "string" }
    }
  }
}
```

#### `rwr_apply` — the tool nobody else ships

```json
{
  "name": "rwr_apply",
  "title": "Apply a planned Ruby rewrite (writes files)",
  "description": "Execute a plan from rwr_plan atomically. Every edit's content digest is checked before any byte is written; a single mismatch aborts the whole transaction and nothing is modified. Requires a plan_id — you cannot apply a rewrite you have not planned.",
  "annotations": { "readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": false },
  "_meta": { "anthropic/requiresUserInteraction": true },
  "inputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "required": ["plan_id"],
    "additionalProperties": false,
    "properties": {
      "plan_id": { "type": "string", "description": "From rwr_plan. Expires." },
      "edit_ids": { "type": "array", "items": { "type": "string" },
        "description": "Content-addressed edit ids from the plan, exactly as returned. Omit to apply every edit. Passing a subset is how you accept some sites and leave others — no counting, no re-crafting the pattern. A stale id fails the whole transaction and names which kind of staleness occurred." },
      "expected_edits": { "type": "integer",
        "description": "Optional guard for the confident path. If the plan's edit count differs, NOTHING is applied and the edit list is returned, so a failed guard costs one call and gives you the list to select from." }
    }
  },
  "outputSchema": {
    "type": "object",
    "required": ["status","files_written","edits_applied"],
    "properties": {
      "status": { "enum": ["applied","aborted_stale","aborted_conflict",
                           "aborted_verify_failed","aborted_expired","aborted_guard","retryable"] },
      "files_written": { "type": "integer" },
      "edits_applied": { "type": "integer" },
      "files": { "type": "array", "items": {
        "type": "object", "required": ["file","edits"],
        "properties": { "file":{"type":"string"}, "edits":{"type":"integer"} }}},
      "stale_edits": { "type": "array",
        "description": "Present on aborted_stale. NOTHING was written.",
        "items": { "type": "object",
          "required": ["edit_id","problem"],
          "properties": {
            "edit_id": { "type": "string" },
            "problem": { "enum": ["malformed_id","file_no_longer_matches","fewer_matches_than_planned","matched_text_changed"] },
            "detail": { "type": "string" }
          }}},
      "residual_matches": { "type": "integer",
        "description": "Matches suppressed by a rewritten range. Non-zero means status=retryable: re-plan and re-apply, and it will make progress. rwr never auto-fixpoints within one call." },
      "verify": { "type": "object",
        "properties": {
          "reparsed": { "type": "boolean" },
          "mismatch": { "type": ["object","null"],
            "description": "Present on aborted_verify_failed. The transformation was discarded." }
        }},
      "next_action": { "type": "string" }
    }
  }
}
```

The content-addressed `edit_id` comes directly from Serena's `replace_in_files`. It is strictly
better than a per-file content hash — which `DESIGN.md` §7 currently specifies — because file-level
hashing is all-or-nothing: one unrelated line changed anywhere kills the whole transaction, whereas
edit-level digests fail only where the *matched text* actually moved. It is also how an agent
accepts a subset after reviewing the plan, which is what §4 wants for the "ambiguous sites need
judgment" case, without elicitation and without abandoning determinism. **Keep whole-transaction
atomicity over the selected set.**

#### `rwr_info`

No arguments. Returns `rwr_version`, `prism_version`, `repo_root`, `allowed_roots`,
`default_max_results`, `max_results_cap`, `max_files_cap`, `default_max_answer_chars`,
`plan_ttl_seconds`, `default_exclusions`, `capabilities` (e.g. `{receiver_narrowing: false}` until
Phase 2), and **`coordinate_conventions`** (`{lines: "1-based", columns: "1-based-chars", bytes:
"0-based-utf8"}`). Straight from PR #36's `get_server_info`. A self-describing tool that tells the
agent whether line numbers are 0- or 1-based is a small idea with a large payoff — ast-grep's JSON
uses **zero-based** `line` and `column` and users file it as an off-by-one bug
([ast-grep#1232](https://github.com/ast-grep/ast-grep/issues/1232)).

### 2.8 How the CLI contract maps onto MCP

**Exit codes become a `status` enum, not `isError`.** MCP gives one boolean where the CLI has five
states; collapsing them loses the retryable-vs-terminal distinction that `cli-conventions.md` calls
"the important one."

| CLI outcome | MCP `status` | `isError` | Rationale |
|---|---|---|---|
| matched / rewrote | `ok` / `applied` | false | |
| no matches | `no_matches` | **false** | A clean negative is an answer. PR #36 calls this "preserve valid negative probes." |
| retryable | `retryable` | **false** | With `next_action` naming the re-invocation. `isError: true` pushes some clients to hide the payload the agent needs. |
| refused | `refused` | **false** | The refusal *is* the deliverable and carries `refusals[]`. Marking it an error invites the agent to treat it as transient and retry. |
| usage / internal | — | **true** | Only here — the case where the agent must change its invocation. |

**Invariant, from Chrome DevTools MCP** (which catches into `response.setError(err)` *before*
rendering, so a failure ships with the snapshot and console state that explain it): every result
carries ambient state, **including error results**. A `refused` or `aborted_*` result must still
carry `coverage` and `residue`. Since #86032 drops `structuredContent` on errors, that state must
also be in `content`.

**Dry-run** is not a flag but a separate tool, because MCP annotations are per-tool. This is
stronger than the CLI's `--dry-run`, and the CLI should arguably converge: `rwr plan` emitting a
plan file that `rwr apply` consumes gives the terminal user the same guarantee.

**`--explain`** is not a flag either — it is `rwr_check`'s `rejection[]` and `rwr_plan`'s
`refusals[].explanation`, always on. An agent will not think to pass `--explain`; the explanation
must be in the default result or it does not exist.

**Output channel.** Put the authoritative compact text in `content` (ast-grep's format, ~75%
cheaper than JSON) and the machine mirror in `structuredContent` with a 2020-12 `outputSchema`.
Given #79944 and #86032: **make `content` self-sufficient — never put a fact only in
`structuredContent`** — and default `structuredContent` **off** when the client identifies as
Claude Code, on elsewhere, with an `RWR_MCP_STRUCTURED` override in both directions.

**Budgets.** Default `max_results: 50`, hard ceiling 500. Target **under 10,000 tokens** per result
(that warning threshold is fixed). Degrade rather than fail past the budget, and never degrade
`coverage` or `residue`. Declare `_meta["anthropic/maxResultSizeChars"]` on `rwr_plan` only (diffs
are legitimately large), 150,000–200,000. Keep every description under 2KB with the critical
sentence first, and **interpolate the constants into the descriptions** (JetBrains' `- Limits
output to $maxLineCount lines`) so docs cannot drift from behavior.

### 2.9 What not to build

- **A repo-wide `rwr_audit` MCP tool.** `DESIGN.md` already rejects the repo-wide dynamic-dispatch
  inventory as carrying zero per-query information, and it is exactly the shape that blows the
  token cap. *"Exhaustive scans remain a CLI responsibility."*
- **A bare `rwr_dump_tree` tool.** Fold it into `rwr_check` as `show_tree`, opt-in; ship the node
  vocabulary as a resource.
- **Elicitation for ambiguity.** Prefer `_meta["anthropic/requiresUserInteraction"]` for
  *authorization* of an already-computed plan.
- **Dynamic tool registration.** GitHub built it and
  [tore it out](https://github.com/github/github-mcp-server/pull/2512) — *"it carried real
  complexity… three meta-tools, and a chunk of conformance/CI matrix."* If rwr's surface ever
  exceeds five tools, use JetBrains' static router-passthrough pattern instead.

---

## 3. UX mechanisms to adopt, ranked

Each names its source tool and what it buys. The first three **contradict decisions currently
recorded in `docs/cli-conventions.md`** and should be resolved before v0.1, since that doc is a
public contract from day one.

### 3.1 Default to *not writing*; one verb means "touch disk" — biome, ast-grep, semgrep, ruff (unanimous)

Every peer defaults to preview and requires an explicit flag to mutate:

| Tool | Default | Escalation to write |
|---|---|---|
| ast-grep `run -p X -r Y` | prints a diff, writes nothing | `-U/--update-all` or `-i/--interactive` ([run reference](https://ast-grep.github.io/reference/cli/run.html)) |
| semgrep | suggests fixes only | `--autofix` (with `--dryrun` to suppress the write) |
| ruff `check` | reports only | `--fix`, then `--unsafe-fixes` |
| biome `check` | reports only | `--write`, then `--write --unsafe` |
| rubocop | inspects only | `-a` / `-A` |

`cli-conventions.md` currently says *"`--dry-run` on every mutating command,"* which implies
write-by-default. **Invert it: `rwr rewrite` prints the diff and JSON plan by default; `--write` is
the single uniform verb that touches disk.** A forgotten `--dry-run` is *unrecoverable*; a
forgotten `--write` is *recoverable* — the agent sees no files changed, reads the diff, retries.
The asymmetry is the whole argument. It also deletes a flag: with preview as the default,
`--dry-run` becomes unnecessary.

Take the naming from biome, which renamed `--apply`/`--apply-unsafe` → `--write`/`--write --unsafe`
in v1.8 ([#2267](https://github.com/biomejs/biome/issues/2267)) for exactly this reason:

> "For the sake of consistency and discoverability I propose to use a unique name for indicating to
> a command to modify the code."

Ranking of the four naming schemes for an agent reading `--help` cold: dprint (`fmt` vs `check` —
the *subcommand* says whether it mutates) > biome (`--write` + `--unsafe`) > ruff
(`--fix`/`--fix-only`/`--unsafe-fixes`/`--diff` is a 4-flag matrix) > rubocop (`-a` vs `-A` differ
only by case — trivially mistyped by an LLM composing a shell string, and visually indistinguishable
in a log).

### 3.2 Fix the exit-code layout — `2` must mean "error", and hook mode needs opposite polarity

Two independent problems with the table in `cli-conventions.md`.

**(a) `2 = retryable` collides with a near-universal `2 = error`.** grep, ripgrep, ruff, rubocop,
biome, jq, difftastic and semgrep all agree that 2 means something went wrong. An agent that has
learned `rg` — which `docs/decisions.md` D11 explicitly wants — will misread it.

**(b) Exit 1 = "no matches" breaks every hook rwr is wired into.** pre-commit's contract is
literally *"The hook must exit nonzero on failure or modify files"*
([new-hooks.md](https://github.com/pre-commit/pre-commit.com/blob/main/sections/new-hooks.md)). A
rule that correctly matches nothing in the staged files — the overwhelmingly common case — would
block the commit. This is a **polarity** problem, not a flag problem: in search mode "no match" is
a negative result; in enforcement mode "no match" *is* the success state. The same code cannot mean
both.

There is direct precedent for splitting polarity by verb in one binary: **ast-grep `run` exits 0 on
≥1 match (grep semantics) while ast-grep `scan` exits 1 on ≥1 rule match (lint semantics)** — same
tool, opposite conventions, chosen per subcommand.

**Recommended layout:**

| Verb | Semantics | 0 | 1 | 2 | 3 | 4 | 5 |
|---|---|---|---|---|---|---|---|
| `rwr find` | search | matched | no match | error (I/O, internal, usage) | pattern parse error | — | — |
| `rwr rewrite` | search + mutate | matched/rewrote | no match | error | pattern parse error | retryable | refused |
| `rwr apply --check` | enforcement | clean | would rewrite | error | pattern parse error | — | refused |
| `rwr apply` | enforcement + mutate | done (incl. no-op) | remaining refusals | error | pattern parse error | retryable | refused |

The **pattern-error-gets-its-own-code** idea comes from jq, the closest analogue to rwr's failure
taxonomy: jq exits **3** on a *program* (query) compile error and **5** on a *runtime/data* error.
Verified in [`src/main.c`](https://github.com/jqlang/jq/blob/master/src/main.c):

```c
enum { JQ_OK = 0, JQ_OK_NULL_KIND = -1, JQ_ERROR_SYSTEM = 2,
       JQ_ERROR_COMPILE = 3, JQ_OK_NO_OUTPUT = -4, JQ_ERROR_UNKNOWN = 5 };
```

That maps exactly onto rwr's pattern-parse-error vs file-parse-error split, which the current
conventions doc doesn't distinguish at all. **Cautionary note attached:** jq's own
[manual](https://jqlang.org/manual/) documents 0, 2, 3 and 4 and never mentions 5. Document every
code in `rwr help exit-codes`, not just in a design doc.

Add `--exit-zero-on-no-match` (name family from ruff's `--exit-non-zero-on-fix`) plus
`RWR_EXIT_ZERO_ON_NO_MATCH`, and bake it into the shipped hook definitions. Do not make it the
default.

**One nuance to decide explicitly.** ripgrep's
[`main.rs`](https://github.com/BurntSushi/ripgrep/blob/master/crates/core/main.rs):

```rust
Ok(if matched && (args.quiet() || !messages::errored()) { ExitCode::from(0) }
   else if messages::errored() { ExitCode::from(2) }
   else { ExitCode::from(1) })
```

Error wins over match. If rwr matches in 80 files and one vendored file fails to parse, `DESIGN.md`
§4 says unparseable files are "reported and skipped" — so **0, with the skip in the JSON**. That is
the right call and it *differs* from rg. Say so in the docs, because it is exactly the kind of
thing a caller assumes wrong.

### 3.3 A tagged NDJSON event stream, not an array of matches — cargo, ripgrep, semgrep

`cli-conventions.md` specifies `--json` as *"one pretty-printed JSON array (multi-row)."* A
homogeneous array cannot carry what rwr must emit: matches, *and* skipped files, *and* residue
occurrences, *and* conflicts, *and* a summary. Those are heterogeneous records. Ship both shapes:

**(a) `--ndjson` → tagged event stream, cargo's model.**
[`cargo --message-format json`](https://doc.rust-lang.org/cargo/reference/external-tools.html#json-messages)
emits one object per line with a `reason` discriminator: `compiler-artifact`, `compiler-message`,
`build-script-executed`, `build-finished`. For rwr: `match`, `edit`, `skip`, `residue`, `conflict`,
`error`, `finished`.

**The terminator matters more than it looks.** An agent reading a truncated NDJSON stream (killed
process, full disk) cannot distinguish "done, no more matches" from "died halfway" — unless the
last event says so. ripgrep has the same thing as its `summary` message. Also steal cargo's
defensive-parsing advice verbatim: consumers should *"only interpret a line as JSON if it starts
with `{`"*, because subprocesses you shell out to may write to the same stdout.

**(b) `--json` → semgrep's multi-array object.** Confirmed against
[`semgrep_output_v1.jsonschema`](https://github.com/semgrep/semgrep-interfaces/blob/main/semgrep_output_v1.jsonschema),
the top level is `results[]` / `errors[]` / `paths{scanned[], skipped[]}` plus a `version` string.
That three-way separation **is `DESIGN.md` §4's safety contract, already in JSON form**. rwr's
version wants a fourth: `residue[]`. Ship `{schema_version, results[], residue[], errors[],
skipped[], summary{}}`.

### 3.4 Hard-fail on a malformed pattern — the ast-grep failure *not* to repeat

The cleanest differentiator against the closest competitor, documented in their own tracker.
ast-grep tolerates tree-sitter `ERROR` nodes inside patterns by design, so a malformed pattern
silently returns **zero matches, exit 1** — indistinguishable from "your pattern was fine, nothing
matched." From [ast-grep#575](https://github.com/ast-grep/ast-grep/issues/575), maintainer
HerringtonDarkholme:

> "`ERROR` is deliberately tolerated in the CLI for better ergonomics… But I do think in your
> example, the `ERROR` makes no match and it makes users confused. I do think adding a soft error
> will be a good idea if no matches are found."

A *soft warning* was added in 0.10.1. That is still the wrong shape for an agent: a warning on
stderr with exit 1 is not something an agent branches on. **rwr should exit 3 (pattern error) with
a structured diagnostic naming the parse failure position.** For a Prism-based tool this is nearly
free — Prism reports diagnostics, which is the same argument as `DESIGN.md` §3, one level up. It
discharges principles 2 and 9 at the CLI boundary.

### 3.5 Defeat the `$METAVAR` shell-quoting trap — three defenses

Highest-severity *silent corruption* risk in the design, because rwr uses `$METAVARS` and its
primary user composes shell strings. From ast-grep's own docs:

> "The pattern must be enclosed in single quotes `'` to prevent the shell from interpreting the `$`
> sign." — with double quotes, `ast-grep -p "$PROP && $PROP()"` "would be interpreted as
> `ast-grep -p " && ()"` after shell expansion."

The shell has already destroyed the evidence by the time `argv` arrives, and the pattern that lands
is *still syntactically plausible* — it just means something else. Agents default to double quotes
constantly.

1. **Take the pattern off the command line.** `--pattern-file FILE`, `-` for stdin
   ([clig.dev](https://clig.dev/): *"If input or output is a file, support `-` to read from
   `stdin`"*), and the YAML rule file D2 already specifies. An agent writing a file never touches
   shell quoting.
2. **jq's `--arg`/`--argjson` model** for parameterization: pass values as named typed bindings
   rather than interpolating into the pattern string. Eliminates an injection/quoting bug class.
3. **Detect the collapse.** If the pattern contains `(`/`)`/`&&` but zero `$`, and the rule's
   `where:` block references metavariables absent from `match:` — that is the signature. Say so by
   name: *"pattern references `$A` in `where:` but contains no metavariables; if you used double
   quotes, the shell expanded them — use single quotes or `--pattern-file`."*

### 3.6 Announce degradation in the output, with position — difftastic

Principle 9 is "never silently degrade." difftastic is the only surveyed tool that implements the
*announcement*, and its format is directly copyable
([tricky-cases manual](https://difftastic.wilfred.me.uk/tricky_cases.html)):

> "By default, difftastic falls back to a line-oriented diff whenever parse errors are
> encountered." — "a conservative choice to ensure that difftastic never claims that two
> syntactically different files are the same."

```
sample_files/syntax_error_2.js --- Text (2 JavaScript parse errors, exceeded DFT_PARSE_ERROR_LIMIT, first at 3:1)
```

Two mechanisms in one line: (a) the **parser actually used** is printed (`Text`, not `JavaScript`),
so degradation is visible without reading prose; (b) `DFT_PARSE_ERROR_LIMIT` is a **tunable
tolerance**, not a hardcoded cliff. Both belong in rwr's skipped-file records.

### 3.7 Refuse to mutate a dirty tree — `cargo fix`

Direct precedent for `DESIGN.md` §7's read-write race, and it sharpens the flag naming.
`cargo fix`'s exact message:

> "the working directory of this package has uncommitted changes, and `cargo fix` can potentially
> perform destructive changes; if you'd like to suppress this error pass `--allow-dirty`,
> `--allow-staged`, or commit the changes to these files"

Three things in one sentence: what's wrong, *why it matters*, and three concrete remedies. Note the
naming — **`--allow-dirty` / `--allow-staged`, not a blanket `--force`.** Two graded escape hatches,
each naming the specific condition it waives.

`cargo package` goes further and leaves an audit trail even when overridden, writing
`.cargo_vcs_info.json` with `{"git": {"sha1": "...", "dirty": true}}`. rwr's equivalent: stamp
`{"vcs": {"sha": "...", "dirty": true}}` into the JSON summary whenever it wrote against a dirty
tree. The escape hatch leaves evidence rather than silently proceeding. This complements — does not
replace — the per-edit content digest, which guards a narrower and more important race.

### 3.8 The summary line names the next command — ruff, rubocop

Cheapest high-value mechanism in the whole survey. Ruff:

```
src/numbers/calculate.py:3:8: F401 [*] `os` imported but unused
Found 1 error.
[*] 1 fixable with the `--fix` option.
```

RuboCop: `2 files inspected, 16 offenses detected, 14 offenses corrected, 2 more offenses can be
corrected with rubocop -A`.

The tail of the sentence *is the next command*. For rwr:

```
83 matches, 83 rewritable. Run with --write to apply.
81 rewritten, 2 skipped (contained in a rewritten range). Rerun to make progress.
79 rewritten, 4 refused (overlapping edits at app/models/widget.rb:14). Needs judgment; see --explain.
```

Orthogonal to `--json`, but the "state the residual and the command that clears it" discipline
should shape the JSON summary record too.

### 3.9 Bounded lists with a true total, and a degradation ladder — rubydex, Serena, PR #36, Claude Code's own Grep

This one is load-bearing for a reason specific to rwr's primary harness. From the
[Claude Code tools reference](https://code.claude.com/docs/en/tools-reference):

| Result | What the agent gets |
|---|---|
| Valid | Inline up to ~30,000 chars; past that, **a file path plus a short preview** it can Read/Grep |
| Failure | Inline up to ~10,000 chars; past that, a head-and-tail excerpt, **with no file path** |

`BASH_MAX_OUTPUT_LENGTH` (default 30,000, ceiling 150,000) widens the read-back window but **does
not raise the inline ceilings**.

The consequence is sharp: **rwr's refusal output and its residue report are exactly the output most
likely to be large *and* most likely to be classified as a failure.** A refusal that dumps 2,000
residue occurrences gets head-and-tail-truncated with no way to recover the middle. §9.4's "report
what you couldn't see, unconditionally" is right, but **unconditional must not mean unbounded.**

Therefore:

- Cap every list in human output (default ~20 rows) and **always print the true total**, so
  truncation never destroys the count. The harness itself uses this pattern — its Grep tool "caps
  at 100 files" but its `count` mode's *"total covers every match even when the tool's `head_limit`
  or `offset` parameters truncate the listed per-file entries."* rubydex's MCP tools do the same
  with `{results, total}` and a per-tool `max_limit`.
- Make the overflow path explicit and actionable in the message itself: `… and 1,847 more. Re-run
  with -J > residue.ndjson, or narrow with --path.`
- Prefer Serena's **degradation ladder** over a hard cut where the result has natural resolutions:
  full matches with source → `file:line` + one-line excerpt → locations only → per-file counts →
  `"3,412 matches in 214 files"`. **`coverage` and `residue` never degrade** — they are small and
  they are the point.
- Keep `-J/--ndjson` uncapped by default (the caller asked for a stream and can redirect it), but
  honor `--limit N` on every listing command.

### 3.10 Errors carrying a machine code, a message, and a remediation — rubydex

From
[`base_tool.rb`](https://github.com/Shopify/rubydex/blob/main/lib/rubydex/mcp_server/tools/base_tool.rb):

```ruby
Error.new(
  "not_found",
  "Declaration '#{name}' not found",
  "Try search_declarations with a partial name to find the correct FQN",
)
```

Three fields: a stable machine code, a human message, and **the next call to try**. This is
`cli-conventions.md`'s "error messages name the fix," promoted from a printed string to a typed
field. rwr's refusal contract should use exactly this shape — a refusal is not an exception, it is
a result with a code, a message, and a suggested next move.

Same family, from ast-grep and Semgrep respectively: `"No matches found for the given code and
rule. Try adding \`stopBy: end\` to your inside/has rule."` and `"Error fetching findings: … Try
reducing the limit parameter."` And from gh: `gh pr list --json x` returns `Unknown JSON field:
"x"` **followed by the full list of valid fields** — errors that double as documentation let an
agent self-correct in one round trip. Apply that to rwr's `where:` constraint keys and
`--output-format` values.

### 3.11 Progressive disclosure: `-h` short and categorized, `--help` long — ripgrep via clap

Measured on live binaries: `rg -h` = **135 lines**, `rg --help` = **1,617 lines**. `fd -h` = 38,
`fd --help` = 369.

ripgrep's mechanism is data-driven: each flag has `doc_short()`, `doc_long()` and `doc_category()`,
and the same source generates `-h`, `--help` *and* the man page. Categories
([`flags/mod.rs`](https://github.com/BurntSushi/ripgrep/blob/master/crates/core/flags/mod.rs)):
`Input`, `Search`, `Filter`, `Output`, `OutputModes`, `Indexing`, `Logging`, `OtherBehaviors`. The
template also contains the discoverability line worth stealing verbatim: **`Use -h for short
descriptions and --help for more details.`**

**Most of this is free in clap.** Per the
[derive reference](https://docs.rs/clap/latest/clap/_derive/index.html): a doc comment's **first
line** becomes `about`/`help` (used for `-h`); the **whole comment, when it contains a blank line**,
becomes `long_about`/`long_help` (used for `--help`). Category grouping comes from
`next_help_heading` — fd, on stock clap without headings, has a flat `Options:` list, so the
grouping is the part you have to ask for.

Rationale, from [ripgrep#189](https://github.com/BurntSushi/ripgrep/issues/189) — BurntSushi on why
not to hide flags: *"I'm not opposed to doing this, but I'm not 100% sure we should because I'm not
a fan of making flags harder to discover."* And on paging: *"FWIW, `rg --help | less` should help."*
**Never page your own help.**

### 3.12 Help *topics* for cross-cutting docs — gh

`gh help` exposes a tier that is neither per-command help nor a man page:
[`gh help exit-codes`](https://cli.github.com/manual/gh_help_exit-codes),
[`gh help formatting`](https://cli.github.com/manual/gh_help_formatting),
[`gh help environment`](https://cli.github.com/manual/gh_help_environment) — real fetchable topics
listed under `HELP TOPICS` in `gh --help`.

rwr has exactly this shape of content: `rwr help exit-codes`, `rwr help patterns`, `rwr help json`,
`rwr help residue`. Discoverable from `--help`, needs no man page install (BurntSushi's objection:
*"man pages aren't available everywhere"*), and — for an agent — is a documented, self-contained way
to fetch the exit-code contract without a web search. Given jq's undocumented exit 5, this is the
direct fix.

### 3.13 Confidence grades on rewrites — rustc's `Applicability`

rustc tags every machine-readable suggestion with one of four values
([JSON format](https://doc.rust-lang.org/rustc/json.html)):

| Variant | Meaning |
|---|---|
| `MachineApplicable` | can be applied mechanically |
| `HasPlaceholders` | contains placeholder text, needs human fill-in |
| `MaybeIncorrect` | produces valid code but may not match intent |
| `Unspecified` | we don't know which of the above |

`cargo fix` consumes exactly this and applies only `MachineApplicable`. That is the whole pipeline:
engine emits span + replacement + grade; driver filters by grade.

This is a *graded* answer where ruff, biome and rubocop all ship a *binary* safe/unsafe. Given that
rwr's value proposition is honest reporting of what it couldn't see, a graded applicability is more
in character than a boolean — and **`Unspecified` is the honest bucket the binary schemes lack**.
Worth noting rubocop has two independent axes the others collapse: `Safe` (can the cop
false-positive?) and `SafeAutoCorrect` (does the correction preserve semantics?).

### 3.14 Fixpoint is viable if you cap and verify — ruff (verified in source)

D15 says "no auto-fixpoint inside one invocation," citing RuboCop's 200-iteration cap as the
fallback. Ruff's implementation is the better model, in
[`crates/ruff_linter/src/linter.rs`](https://github.com/astral-sh/ruff/blob/main/crates/ruff_linter/src/linter.rs):

```rust
pub(crate) const MAX_ITERATIONS: usize = 100;
// As an escape hatch, bail after 100 iterations.
```

and — the part that matters — it reparses on every iteration and **discards the entire
transformation** if a fix introduced a syntax error:

```rust
if has_valid_syntax && has_no_syntax_errors {
    if let Some(error) = parsed.errors().first() {
        report_fix_syntax_error(path, transformed.source_code(), error, fixed.keys());
        return Err(anyhow!("Fix introduced a syntax error"));
    }
}
```

That is `DESIGN.md` §7's reparse-verify already shipped in production, with the exact
revert-the-whole-transformation semantics rwr specifies. Two takeaways: **cite it** as prior art,
and note that D15's "no fixpoint" is a *choice*, not a necessity — "we can't" and "we chose not to"
reverse differently.

### 3.15 Deterministic ordering, unconditionally

`cli-conventions.md` doesn't mention output ordering, and rwr will use rayon.

- **ripgrep** makes determinism opt-in and states its cost: `--sort <none|path|modified|...>`, with
  *"sorting results currently always forces ripgrep to abandon parallelism and run in a single
  thread."* From [ripgrep#152](https://github.com/BurntSushi/ripgrep/issues/152), BurntSushi refused
  to fake it: *"Simply being better than `-j1` is good enough to put it behind a flag, but it's not
  good enough to make it default."*
- **ruff** is explicitly non-deterministic under rayon —
  [ruff#22891](https://github.com/astral-sh/ruff/issues/22891) documents a real race where exclude
  patterns applied inconsistently.

**But ripgrep's tradeoff doesn't apply to rwr.** rg streams results as it finds them, so sorting
forces serialization. rwr must collect all matches before applying edits anyway (D13's action tree
is global; D15's outermost-only resolution needs the full set). Once you hold the complete result
set, sorting by `(file, byte_start)` is essentially free. **Make deterministic ordering
unconditional and say so** — it is a differentiator none of the three peers offers, it costs
nothing given the architecture, and principle 1 already claims it.

### 3.16 Color and TTY — the verified algorithm

Order, from [`anstream`](https://github.com/rust-cli/anstyle)'s `auto.rs`:

```rust
if anstyle_query::no_color()            { Never }
else if anstyle_query::clicolor_force() { Always }
else if clicolor_disabled               { Never }        // CLICOLOR=0
else if raw.is_terminal()
     && (anstyle_query::term_supports_color()
         || clicolor_enabled || anstyle_query::is_ci())  { Always }
else                                    { Never }
```

with an explicit `--color` flag short-circuiting above all of it — ruff's implementation is the
clean reference: *"Cli arguments should take precedence over env vars."*

Specs, exactly: [no-color.org](https://no-color.org) — *"when present and not an empty string
(regardless of its value)"*, so `NO_COLOR=0` still disables color;
[bixense.com/clicolors](https://bixense.com/clicolors/) — `CLICOLOR_FORCE` set (and `NO_COLOR`
unset) forces on, `CLICOLOR=0` forces off, *"Empty variables are treated as though they were
unset."* **Rust stack: `anstream` + `anstyle-query` + `colorchoice-clap`. Don't hand-roll it.**

Who gets it right: ripgrep honors `NO_COLOR` and `TERM=dumb` and suppresses color under
`--json`/`--vimgrep`, but does **not** implement `CLICOLOR`/`CLICOLOR_FORCE` (verified: zero code
hits). gh honors all three plus `GH_FORCE_TTY`. ruff additionally forces color **off** whenever
`--output-file` is set — a nice touch: writing to a file is itself a signal.

**On CI detection**, the citable opinion is [npm/ci-detect](https://github.com/npm/ci-detect)
(isaacs): *"Since any program can set or unset whatever environment variables they want, this is not
100% reliable"* and *"If your program does different behavior in CI/test/deployment than other
places, then there's a good chance that you're doing something wrong!"* Its recommendation — use CI
detection only for *"little niceties like setting colors or other output parameters, or logging"* —
is exactly how `anstream` uses it. **Never branch rwr's behavior on `CI`.** Only its cosmetics.
Note that lefthook does branch on it deliberately, via `fail_on_changes: ci|non-ci` — that is the
*runner's* call to make, not the tool's.

### 3.17 Smart defaults with a graded escape ladder — ripgrep's `-u`/`-uu`/`-uuu`

`cli-conventions.md` already says "respect `.gitignore` by default" and "exclude generated and
vendored code by default." Two additions from ripgrep's experience.

**The ladder:** `-u` = `--no-ignore`; `-uu` = `--no-ignore --hidden`; `-uuu` = also `--binary`; with
the framing *"`rg -uuu` should search the same exact content as `grep -r`."* A stackable single flag
with a documented terminal state beats N independent booleans.

**The controversy and what it teaches.** [ripgrep#645](https://github.com/BurntSushi/ripgrep/issues/645)
is a large monorepo where files are `.gitignore`d yet still git-tracked, so ripgrep silently skipped
files `git grep` finds. BurntSushi: *"I do agree that there is an impedance mismatch here between
git and ripgrep for exactly the reason you state."* He did **not** change the default; he added
composable escape hatches. For rwr the stakes are higher — a silently-skipped file during a rename
is a *correctness* failure, not a search miss. §4 already has the answer (skipped files are
reported), but **ignore-driven skips must land in the same `skipped[]` array as parse failures**,
with a `reason` distinguishing them.

### 3.18 Explicit-path exclusion semantics — rubocop's `--force-exclusion`, decided the other way

Verbatim from
[`options.rb`](https://github.com/rubocop/rubocop/blob/master/lib/rubocop/options.rb):
`--force-exclusion` = *"Any files excluded by `Exclude` in configuration files will be excluded,
even if given explicitly as arguments."* The rationale from
[rubocop#893](https://github.com/rubocop/rubocop/issues/893): *"Because command line arguments
override configuration. It's convenient to be able to inspect excluded files by naming them
explicitly."*

That is a **human** ergonomics argument. But a hook runner *always* passes explicit paths, so
`Exclude:` becomes dead config and the hook edits vendored code that CI never touches. Live and
unfixed in Biome as of 2025 ([#7394](https://github.com/biomejs/biome/discussions/7394)), and
rubocop's own fix is incomplete — [#12667](https://github.com/rubocop/rubocop/issues/12667): it
still *loads* `.rubocop.yml` from inside an excluded `vendor/` because it *"checks exclusion only
after loading the corresponding config file."*

**Recommendation for a new mutating tool: make exclusion apply by default**, provide `--no-exclude`
for the human override, and apply exclusion **before** resolving per-file config. rubocop's default
is a 2013 compatibility artifact, not a design.

### 3.19 Accept a file list from stdin — typos, dprint

typos has `--file-list` — *"Read the list of newline separated paths from file or stdin (if `-`)"*.
dprint 0.55+ added `--stdin-files` with the reasoning spelled out: *"Unlike piping through xargs,
this handles file paths containing spaces since the only delimiter is the newline… It also avoids
the command line length limits."*

This removes an entire class of caller bug (ARG_MAX, quoting, chunking) and makes
`git diff --name-only -z … | rwr apply --file-list -` a one-liner. Also offer rg's
`-0/--null` when printing a bare list of changed files — Ruby repos do contain paths with spaces.

### 3.20 `--isolated` / `--no-config`, shipped in v1 — typos, ruff, dprint, eslint

Ship the reproducibility escape hatch **before there is a config to ignore**, so agent harnesses can
pass it unconditionally and forward-compatibly. typos `--isolated` (*"Ignore implicit configuration
files"*), ruff `--isolated`, eslint `--no-config-lookup`. The best-designed version is dprint's
single knob: `--config-discovery={default,ignore-descendants,global,false}` with a matching
`DPRINT_CONFIG_DISCOVERY` env var. Tools that grew it late can point at the ugly workaround it
replaced — rubocop's `--force-default-config` exists because people were writing `--config
/dev/null`. **cargo has no such flag, which is a standing CI reproducibility complaint.**

### 3.21 Print what you resolved — and what you're about to touch

Config: ESLint `--print-config`, ruff `check --show-settings` and the `ruff config` subcommand,
dprint `resolved-config`, ast-grep `--inspect summary`, typos `--dump-config -`, lefthook `dump`.
Missing in rubocop, biome and stable cargo.

File set: **only rubocop has it** — `-L/--list-target-files`. Given that discovery failures are the
*silent* ones, this is arguably the more valuable of the two. ripgrep's `--files` is the analogue.
rwr wants both, plus the plan-level version (which edits *would* be applied), which under §3.1 is
the default output anyway.

Also: **put `config: {path, source}` in every JSON payload.** No tool in the survey does this; all
make you run a second command. It costs one field and means every agent transcript carries the
evidence to explain a surprising diff after the fact.

### 3.22 Split "color" from "interactive elements" — hyperfine's `--style`

hyperfine's `--style` takes six values, not three: `auto | basic | full | nocolor | color | none` —
two independent axes (colorize / animate) instead of one switch. Useful because the three consumers
want different combinations: an agent wants neither, a terminal human wants both, and a human piping
to `less -R` wants color without redraws.

Also worth copying: hyperfine's `--export-json` / `--export-csv` / `--export-markdown` produce
machine output **alongside**, not instead of, the human terminal summary, in one run. For rwr a
human can watch the diff scroll by while the agent parses `--export-json out.json` — no double
invocation, no ambiguity about whether the two runs saw the same tree.

### 3.23 Version and pin the machine format — semgrep, cargo (ripgrep's gap)

Nothing in ripgrep's `--json` stream identifies the schema; a consumer can't detect a format change
without checking `rg --version` out of band, and there is **no formal stability promise** anywhere
(verified absent, not merely unfound). Contrast semgrep, whose output carries `version` and ships a
checked-in JSON Schema generated from an
[`.atd` interface definition](https://github.com/semgrep/semgrep-interfaces/blob/main/semgrep_output_v1.atd),
and `cargo metadata`, which has an explicit `--format-version` flag.

**rwr should carry a `schema_version` in the summary record and a `--format-version` flag to pin
it**, and generate the schema from Rust types (`schemars`) so it is a checked artifact rather than
prose. That single mechanism also makes the MCP `outputSchema` and the CLI JSON the same contract
(§2.8).

### 3.24 ripgrep's arbitrary-data union — the best idea in its schema

Every path/text field in ripgrep's JSON is an object with exactly one of two keys:

```json
{"path": {"text": "/home/ubuntu/lib.rs"}}
{"path": {"bytes": "L2hvbWUvdWJ1bnR1L2xpYv8ucnM="}}
```

Rationale, verbatim from
[the printer source](https://github.com/BurntSushi/ripgrep/blob/master/crates/printer/src/json.rs):

> "The printer could silently ignore such things completely, or even lossily transcode invalid UTF-8
> to valid UTF-8 by replacing all invalid sequences with the Unicode replacement character. However,
> this would prevent consumers of this format from accessing the original data in a non-lossy way."
> "The printer guarantees that the `text` field is used whenever the underlying bytes are valid
> UTF-8."

It makes lossy transcoding **unrepresentable** rather than merely discouraged — the same move as
`DESIGN.md` §7's `effective_range()`. **Relevance to rwr:** Ruby source carries non-UTF-8 encodings
(`# encoding: euc-jp` magic comments are still in the wild), and macOS/Linux paths are arbitrary
bytes. The moment rwr emits a captured `$METAVAR`'s source text as a JSON string it inherits this
problem. Adopt the union for every field carrying file bytes or paths.

### 3.25 Concrete hook integrations to ship

**lefthook** ([configuration docs](https://lefthook.dev/configuration/run/)):

```yaml
# lefthook.yml
pre-commit:
  parallel: true
  jobs:
    # Fixer: apply the project's rewrite rules, re-stage what changed.
    - name: rwr-fix
      glob: "*.rb"
      run: rwr apply --rules .rwr/rules --quiet {staged_files}
      stage_fixed: true
      fail_text: "rwr refused a rewrite. Run `rwr apply --rules .rwr/rules --explain` to see why."

    # Checker: refuse-only, never writes. Same rules, opposite polarity.
    - name: rwr-check
      glob: "*.rb"
      run: rwr apply --rules .rwr/rules --check {staged_files}
      fail_text: "rwr found rewritable code. Run `rwr apply --rules .rwr/rules` to fix."

pre-push:
  jobs:
    - name: rwr-check
      glob: "*.rb"
      run: rwr apply --rules .rwr/rules --check {push_files}
```

The mechanics that matter: `stage_fixed: true` makes lefthook `git add` the modified files after the
run, **so it does not need rwr's exit code to know files changed**; `glob` both filters *and skips*
(*"If no files left, the command will be skipped"*), so lefthook already solves "zero relevant
files"; `fail_text` is a one-line remediation string printed on failure — cheap and high-value.
`skip: [- run: "! which rwr"]` makes a hook self-disabling when the binary isn't installed. CI
entrypoint is `lefthook run pre-commit --all-files`. **Gotcha:** lefthook's `**` matches *1 or more*
directories unlike everyone else — `src/**/*.js` does not match `src/file.js`; opt into standard
behavior with `glob_matcher: doublestar`.

**pre-commit** — a repo must ship `.pre-commit-hooks.yaml` **at its root**. Invocation shape is
**args first, then filenames**, so rwr must accept a trailing positional path list after all flags
(a CLI whose pattern is positional cannot be a pre-commit hook without moving the rule into a flag).

```yaml
# .pre-commit-hooks.yaml  (repo root)
- id: rwr
  name: rwr
  description: "Apply Ruby structural rewrite rules"
  entry: rwr apply --force-exclude --exit-zero-on-no-match
  language: python              # prebuilt wheels; see the typos setup.py trick below
  types: [ruby]
  args: []
  require_serial: true
  stages: [pre-commit, pre-merge-commit, pre-push, manual]
  minimum_pre_commit_version: "3.2.0"

- id: rwr-check
  name: rwr check
  description: "Report Ruby code that rwr rules would rewrite"
  entry: rwr apply --check --force-exclude
  language: python
  types: [ruby]
  args: []
  require_serial: true
  stages: [pre-commit, pre-merge-commit, pre-push, manual]
  minimum_pre_commit_version: "3.2.0"

- id: rwr-src
  name: rwr
  description: "Apply Ruby structural rewrite rules (built from source)"
  entry: rwr apply --force-exclude --exit-zero-on-no-match
  language: rust
  types: [ruby]
  args: []
  require_serial: true
  stages: [pre-commit, pre-merge-commit, pre-push, manual]
  minimum_pre_commit_version: "3.2.0"
```

Four deliberate choices, each with a precedent:

- **`--force-exclude` lives in `entry:`, not `args:`.** From
  [ruff-pre-commit#19](https://github.com/astral-sh/ruff-pre-commit/issues/19): *"If it's true that
  running in pre-commit always needs `--force-exclude` to work properly, then you can set it as part
  of the entry… This will then always include this option when running, regardless of what `args` is
  set to."* RuboCop puts it in `args` and users silently drop it.
- **`require_serial: true`**, exactly as ruff and biome do — a tool with internal rayon parallelism
  should not also be fanned out by the runner.
- **`stages:` explicitly listed**, per pre-commit's own advice: *"a reasonable setting for a linter
  or code formatter would be `stages: [pre-commit, pre-merge-commit, pre-push, manual]"*.
- **`language: rust` is the fallback, not the primary.** It compiles the crate from source on every
  user's first run. [typos](https://github.com/crate-ci/typos) — a Rust CLI that mutates files and
  ships via cargo + brew, i.e. rwr's closest structural twin — solves this with a `setup.py` at the
  repo root pointing at a PyPI package of prebuilt platform wheels, and documents the choice in
  [docs/pre-commit.md](https://github.com/crate-ci/typos/blob/master/docs/pre-commit.md): *"The
  `typos` id installs a prebuilt executable from GitHub releases. If one does not exist for the
  target platform, or if one built from sources is preferred, use `typos-docker` … or `typos-src`."*
  Semgrep does the same, with the reasoning in a comment in its `.pre-commit-hooks.yaml`: *"This
  hook … is significantly faster (especially on macOS)."*

**Note both runners batch the file list across multiple invocations of the binary**
([pre-commit#3397](https://github.com/pre-commit/pre-commit/issues/3397); lefthook *"splits your
files list to fit in the limit and runs few commands sequentially"*). rwr's safety contract — "83
matches, 83 rewritten, 0 conflicts" plus name-scoped residue — is a **repo-wide** claim, and under a
hook runner it is computed over an arbitrary subset of an arbitrary batch. **Either residue
reporting is suppressed in hook mode, or the numbers are lies.** Decide this explicitly; §4 records
the recommendation.

**Also flag in the README:** [overcommit](https://github.com/sds/overcommit), the Ruby-native hook
manager rwr's audience may well be using, has a README heading — **"WARNING: pre-commit hooks cannot
have side effects"** — *"pre-commit hooks currently do not support hooks with side effects (such as
modifying files and adding them to the index with `git add`). This is a consequence of Overcommit's
pre-commit hook stashing behavior."* rwr's fixer mode is unusable there.

### 3.26 Changed-files-only: `--since <rev>`, failing loud

Implement as `git diff --name-only -z --diff-filter=ACMR --merge-base <rev> --`. Attribution:
semgrep's `--baseline-commit` (*"the ideal value is the git merge-base between the branch being
scanned and the target branch"*) and lefthook's own
[`repo.go`](https://github.com/evilmartians/lefthook/blob/master/internal/git/repo.go), which uses
exactly `--diff-filter=ACMR`. `ACMR` excludes deletions — a deleted file can't be rewritten. Decide
`--cached` explicitly; for an agent mid-edit, *omitting* it is probably right, a deliberate
divergence from semgrep.

**Fail loud when a ref can't be resolved.** Semgrep does `git cat-file -e` → `SemgrepError`.
Negatively attributed to [golangci-lint#3320](https://github.com/golangci/golangci-lint/issues/3320),
which fails *open* and floods the user with every lint error in the repo. For a mutating tool,
failing open on `--since` means rewriting the whole repo when the user asked for three files.

**Use `git worktree`, never `git stash`, for any baseline checkout.** Semgrep's
[`git.py`](https://github.com/semgrep/semgrep/blob/develop/cli/src/semgrep/git.py) comments: worktree
*"works even if there are changes in tracked files or staged changes. Suitable for pre-commits"* and
*"we don't need to git stash anything."* Rust gotcha you will hit: **scrub `GIT_INDEX_FILE` from the
child env** — inherited, it *"re-resolves against the new cwd and breaks index locking."*

Semgrep's finding-level diffing (subtract on `(rule_id, path, syntactic_context, index)`, **not line
numbers**, with rename re-keying) is the best-in-class design, but **prototype before committing**:
its own PR admits *"if a finding matches on a large section of lines (with a `...`, for example) some
file changes will still show up as a `new` finding"*
([semgrep#4571](https://github.com/semgrep/semgrep/pull/4571)). rwr patterns routinely span large
spans with metavariables, which makes `syntactic_context` an unstable key. Real risk, not a
formality.

### 3.27 Config files: ship none in v1

**Recommendation: no config file for v0.1.** Ship the query on argv, ship `--rules PATH` for a rule
file, and design so adding a project config later is additive.

**(a) The query is not configuration.** rwr's core input is a pattern and a rewrite, which change
every invocation — [clig.dev](https://clig.dev/)'s kind 1, *"Likely to vary from one invocation of
the command to the next. Use flags."* clig.dev's taxonomy is organized by *rate of change*, and its
operational test is the one to apply to every candidate setting: **can this differ between two
consecutive runs by the same person on the same project?** If yes, it's a flag.

ast-grep proves this works for a structural rewriter: *"`run` command does not require a
`sgconfig.yml` file and will still search code without it, but `scan` command will report an error
if project config is not found"*
([project-config](https://ast-grep.github.io/guide/project/project-config.html)). Its `sgconfig.yml`
holds only project *facts* — `ruleDirs`, `customLanguages`, `languageGlobs` — never the query.
semgrep splits the same way: `--config` names *rules*; `--autofix`/`--dryrun`/`--json` are flags only.

**(b) Hierarchical config is the highest-cost mechanism surveyed.** ESLint's own postmortem is the
strongest evidence any tool has produced against itself
([new-config-system-part-1](https://eslint.org/blog/2022/08/new-config-system-part-1/)):

> "Most frequently, people wouldn't realize that they had a config file in an ancestor directory of
> the project they were working on."
> "`extends` inside of an `overrides` config would use an AND operator to merge `files` and
> `excludedFiles`. If you're not sure what exactly that means, you're not alone. It's confusing even
> to us."
> "No one really understood all of the different permutations around calculating the final config
> for any given file."

The nuance that matters: ESLint killed the **cascade**, not the **upward search**. v10 restored
per-file lookup while keeping nearest-wins-no-merge. Ruff independently landed in the same place:
*"Unlike ESLint, Ruff does not merge settings across configuration files; instead, the 'closest'
configuration file is used, and any parent configuration files are ignored."* Two teams converging
on nearest-wins-no-merge is meaningful. **Merging across the hierarchy is what broke ESLint.**

And ruff, having avoided the worst, *still* accumulated a six-issue cluster around hierarchical
`exclude` ([#1220](https://github.com/astral-sh/ruff/issues/1220),
[#2034](https://github.com/charliermarsh/ruff/issues/2034),
[#4127](https://github.com/astral-sh/ruff/issues/4127),
[#9023](https://github.com/astral-sh/ruff/issues/9023), and others). #2034's symptom is the archetype
and the exact failure rwr must never have: *"In a repository where there are no excludes, or
extend-excludes, running `ruff --force-exclude .` claims there are no Python files."* Exit 0. Nothing
done. No signal.

**(c) Don't walk past the repo root, and ship no global config.** rubocop, biome and cargo all walk
to `/` then fall through to `$HOME`. This is precisely the failure ESLint named, and it is strictly
worse for an agent that did not create `~/.rubocop.yml`, cannot see it, and will produce a diff that
doesn't reproduce anywhere else. dprint's own behavior is the tell: it *"prompts for confirmation
before formatting to prevent accidental usage"* under a global config. Even the tool that ships the
feature treats it as dangerous.

typos states the requirement as a design constraint rather than discovering it later, in
[docs/design.md](https://github.com/crate-ci/typos/blob/master/docs/design.md): *"Machine-independent,
repo-specific configuration — as compared to layered config with the users system or the
command-line."* **Stopping the upward walk at the VCS root is what everyone means by "my project" and
nobody implements.** It's cheap and it terminates somewhere an agent can inspect.

**(d) Config affecting DISCOVERY is categorically more dangerous than config affecting BEHAVIOR.**
Behavior failures are loud and local — a rule fires, the output names it. Discovery failures are
silent — the tool exits 0 having done nothing, and success is indistinguishable from a no-op. For a
**mutating** tool that is the worst outcome in the design space.

dprint states this as a security boundary, the strongest form of the argument: *"The `includes`
property of extended remote configuration is ignored for security reasons out of an abundance of
caution (to disallow the dprint cli pulling in sensitive files)."* Behavior config from an untrusted
source is a bug; discovery config from an untrusted source is a file-exfiltration primitive.
semgrep's version of the trap: its default ignore set silently excludes `test/`, `tests/`,
`*_test.go`, `vendor/`, `dist/`, and platform ignores are additive so you can't remove them. With
zero config, semgrep does not scan your tests, and nothing in the invocation says so.

**If and when a config is added:** contents limited to project *facts* — Ruby parser dialect/target
version, default include/exclude globs and whether to respect `.gitignore`, rule directories. Kept
out: output format, check-vs-apply, concurrency, color, verbosity, and above all the pattern and the
rewrite. Shape: single file, one upward walk, **stop at the VCS root**, nearest-wins, no merge. If
composition is ever wanted, copy Biome 2.x (nesting opt-in via explicit `"root": false`, inheritance
via explicit `"extends": "//"`) or ESLint flat's single ordered array — not cargo's silent six-level
merge, and not rubocop's cascade.

**Array semantics must be decided on day one.** rubocop got this right and said why: arrays
**override** because *"if they were merged, there would be no way to remove elements in child
files"*, with `inherit_mode: {merge: [Exclude]}` to opt back in per key. cargo concatenates with
**no removal mechanism at all**.

### 3.28 Two small ones

**Binary name.** `rwr` is clean. Worth noting *why* that matters: ast-grep shipped as `sg`, which
collides with util-linux's `sg` (setgroups), and three years later it's still open —
[#56](https://github.com/ast-grep/ast-grep/issues/56),
[#778](https://github.com/ast-grep/ast-grep/issues/778),
[#1659](https://github.com/ast-grep/ast-grep/issues/1659). From #1659: *"renaming core utilities is
dangerous & should not be a default behavior"* and *"things like this that introduce some risk with
no instantly obvious solution increases the barrier to entry."*

**Ship an agent-facing skill alongside the binary.** The recurring pattern in agent-CLI writing is
that a tool should hand the agent its own usage guidance rather than assume it knows — a
`SKILL.md`/`AGENTS.md` with the rules that matter (*always `rwr check` a new pattern first; always
plan before you apply; exit 3 means stop, exit 4 means retry*). This is also the counter-argument to
the MCP in §2.4: for many workloads a skill over the CLI beats a server.

---

## 4. Three-audience matrix

The three audiences are not three products. They are three *pressures* on one surface, and
they mostly agree. Where they disagree, the disagreement is specific and small enough to name.

| | Human at a terminal | Coding agent in a loop | CI / git hook |
|---|---|---|---|
| **Wants** | To understand what happened | To decide the next call | To pass or fail, fast and quietly |
| **Reads** | The first screen | The exit code, then the JSON | The exit code |
| **Hates** | A wall of undifferentiated output | Prose it must parse; a hang; a silent partial answer | Slowness; a tool that mutates when asked to check |
| **Failure it causes** | Ignores a real warning | Loops, or acts on a false negative | Blocks every commit, gets disabled |

### What each audience must get from rwr

**Human.** Scannable capped output with a true total (§3.9). `-h` fits a screen, `--help` is the
reference, `rwr help <topic>` carries the cross-cutting contracts (§3.11, §3.12). Errors name the
fix and offer graded escape hatches, not a blanket `--force` (§3.7, §3.10). Every summary line ends
in the next command (§3.8). `--explain` is the debugging surface for the refusal contract — without
it "rwr refused" is unactionable.

**Agent.** Branches on the exit code before parsing anything, so the retryable-vs-terminal split is
the highest-value distinction rwr makes — but it must not squat on `2` (§3.2). NDJSON with typed
record variants and a terminating `finished` event (§3.3). Byte offsets *and* line/col, with
documented 0-vs-1-based conventions. Deterministic ordering, unconditionally (§3.15). Every list
bounded, with a true total that survives truncation, and `coverage`/`residue` that never degrade
(§3.9). A malformed pattern is a hard error, never a silent zero (§3.4). Never a prompt, never a
spinner, never a partial answer that looks complete.

**CI / hook.** One invocation that means "check, do not write, tell me if you would have," with an
exit code the runner understands unconfigured (§3.2). Accepts an explicit file list — from argv
trailing positionals for pre-commit, from `--file-list -` for large sets (§3.19) — and *still honors
its own exclusions* when given one (§3.18). Handles zero files gracefully. Needs no config file
present (§3.27). Fast on a handful of files, which means no index warm-up on the critical path — an
argument in favor of D5-amended's "no persistence."

### Where the audiences genuinely conflict, and how each resolves

**1. Exit 1 for "no matches."** Correct for search (`rg`'s convention, and D11 wants that
consistency). Wrong for enforcement: pre-commit's contract is *"exit nonzero on failure or modify
files"*, so a rule that correctly matches nothing would block every commit.
**Resolution: split polarity by verb, as ast-grep does across `run` and `scan` (§3.2).** `rwr find`
is search; `rwr apply --check` is enforcement. Ship `--exit-zero-on-no-match` for the boundary cases
and bake it into the shipped hook definitions.

Secondary, harness-specific: Claude Code treats exit 1 as a *failure* for any command not on a
hardcoded allowlist (`grep`, `rg`, `find`, `diff`, `git diff`, …), which drops the output ceiling
from ~30,000 to ~10,000 chars **with no file-path escape hatch**. Not a reason to change the
convention — it is a harness detail that will move — but it *is* a reason the no-match message must
be tiny and self-explaining, and the refusal report bounded (§3.9).

**2. Unconditional residue reporting vs. quiet-by-default in hooks.** §9.4 says the blind-spot
account is the product and must never hide behind `--verbose`. A hook that prints a residue report
on every commit trains people to ignore it — and worse, under a hook runner the report is
*arithmetically wrong*, because both lefthook and pre-commit batch the file list across multiple
invocations (§3.25). "83 matches, 83 rewritten" computed over an arbitrary subset of an arbitrary
batch is not a claim rwr is entitled to make.
**Resolution: the completeness claim is scoped to the invocation's file set, and residue is
suppressed whenever that set was supplied externally** (`--file-list`, trailing positionals,
`--check`). This is not a violation of the principle — `rwr apply --check` makes no repo-wide claim,
so it owes no repo-wide account. State the exception in the docs so it is a decision rather than a
drift, and have the JSON summary carry `scope: "repo" | "supplied"` so a caller can always tell
which kind of answer it holds.

**3. Color, progress, and interactivity.** No real conflict once §3.16's algorithm is in place
(`anstream` + `NO_COLOR`/`CLICOLOR_FORCE`, flag beats env, suppress under `--json`/`--ndjson`,
force off when writing to a file). hyperfine's two-axis `--style` (§3.22) covers the one remaining
case — a human piping to `less -R` wants color without redraws. **Never branch behavior on `CI`,
only cosmetics.**

**4. Config file.** A human and a CI job want project settings; an agent dropped into an unknown
directory wants reproducibility. **Resolution: ship no config in v1 (§3.27), and ship `--isolated`
anyway (§3.20)** — before there is a config to ignore — so harnesses can pass it unconditionally and
forward-compatibly. Put `config: {path, source}` in every JSON payload so a surprising diff is
explicable after the fact.

**5. Write-by-default.** Nobody wants it. The human wants to see the diff, the agent wants a plan it
can inspect, the hook wants an explicit fixer mode distinct from its checker. §3.1's inversion —
preview by default, `--write` to touch disk — is the rare change that serves all three at once, and
it deletes `--dry-run` rather than adding a flag.

---

## 5. Anti-patterns

Each names the tool that made the mistake.

1. **Marketing the engine's capability, not the interface's.** The [Ruby LSP plugin page](https://claude.com/plugins/ruby-lsp)
   advertises "rename symbol… and intelligent code actions." None of it is reachable through the
   agent-facing `LSP` tool. A user who reads that page and asks Claude to rename a method gets
   grep-and-replace with extra confidence. rwr's README must describe **what rwr can be invoked to
   do**, and Q12's "concrete-syntax transformations are out of reach" must be stated there, not
   discovered.
2. **A silent zero.** ast-grep tolerates `ERROR` nodes in patterns, so a malformed pattern returns
   "No matches found" and exit 1 — indistinguishable from a correct negative
   ([#575](https://github.com/ast-grep/ast-grep/issues/575)). The consequence is on the record: in
   [ast-grep-mcp#29](https://github.com/ast-grep/ast-grep-mcp/issues/29) an agent got a silent zero
   and degraded to a bare-identifier search — it fell back to grep. Ruby LSP has the same failure in
   a worse place ([#44767](https://github.com/anthropics/claude-code/issues/44767)): `"No references
   found"` returned as prose when the bridge read a loading notification as the answer. **A false
   negative on a rename is the worst failure a rewriting tool can have.**
3. **One parameter shape forced onto operations with different natural inputs.** Claude Code's
   `LSP({operation, filePath, line, character})` had nowhere to put `workspaceSymbol`'s required
   `query`, so it shipped permanently broken
   ([#30948](https://github.com/anthropics/claude-code/issues/30948)). It also forces an agent that
   has a *name* to grep for a *position* first. This is the argument against a single fat
   `rwr({mode, …})` tool.
4. **Silent partial coverage, with the exclusions buried in source.** Ruby LSP's index excludes
   `*_spec.rb`/`*_test.rb` by default and never says so, so `goToDefinition` is blind to specs.
   rwr's own defaults (`vendor/`, `db/schema.rb`, gitignored files) create the identical hazard. Fix
   is cheap: **ignore-driven skips land in the same `skipped[]` array as parse failures**, with a
   distinguishing `reason`, and the summary names the active exclusions.
5. **A second code path with its own file discovery.** Ruby LSP's `references` bypasses the index
   with its own `Dir.glob("**/*.rb")`, so it ignores `.gitignore`, walks `vendor/`, reparses
   everything, and takes [35s on a 40k-file repo](https://github.com/Shopify/ruby-lsp/issues/3051).
   Scoping must be one shared mechanism used by every command.
6. **Rejecting pagination on principle.** ast-grep-mcp's maintainer declined it — *"I also don't see
   ripgrep tool has such behavior"* — and the tool returns 194,537-token results against a
   25,000-token cap ([#13](https://github.com/ast-grep/ast-grep-mcp/issues/13)). The rebuttal in the
   thread is the right one: **the client cannot know the result size in advance.** The partial fix
   that followed defaults `max_results` to `0` (unlimited) and drops the truncation notice entirely
   in JSON mode — silently truncating a machine consumer.
7. **Always exit 0.** Comby. A tool whose exit code carries no information forces every caller to
   parse output to learn whether anything happened, and CI to invent its own failure detection.
8. **`-a` vs `-A`.** RuboCop's autocorrect flags differ only by case — trivially mistyped by an LLM
   composing a shell string, and visually indistinguishable in a log. One of them is safe and one
   is not.
9. **Config that silently changes file discovery.** ruff `--force-exclude .` reporting *"no Python
   files"* in a repo with no excludes at all
   ([#2034](https://github.com/charliermarsh/ruff/issues/2034)) — exit 0, nothing done, no signal.
   For a mutating tool this is the worst outcome in the design space, and it is why §3.27 keeps
   discovery config out of v1.
10. **A config cascade nobody can predict.** ESLint's own postmortem: *"No one really understood all
    of the different permutations around calculating the final config for any given file,"* and
    people *"wouldn't realize that they had a config file in an ancestor directory."* Note they
    killed the **merge**, not the upward search — and ruff, which never had the merge, still
    accumulated six issues around hierarchical `exclude`.
11. **`--force-exclusion` off by default.** RuboCop's exclusion is waived for explicitly-named
    paths, which is fine for a human and wrong for a hook runner — which *always* passes explicit
    paths, making `Exclude:` dead config. Live and unfixed in Biome
    ([#7394](https://github.com/biomejs/biome/discussions/7394)); rubocop's own fix is incomplete
    ([#12667](https://github.com/rubocop/rubocop/issues/12667)).
12. **An unversioned machine format.** ripgrep's `--json` carries nothing identifying its schema and
    has no stability promise; a consumer cannot detect a format change without checking
    `rg --version` out of band. Contrast semgrep's `version` field plus a checked-in schema
    generated from an `.atd` definition.
13. **Squatting on a common binary name.** ast-grep shipped as `sg`, colliding with util-linux's
    `sg`; three years and three issues later it is still open. Not rwr's problem, but worth knowing
    why the name matters.
14. **Dynamic tool registration.** GitHub built it and
    [tore it out](https://github.com/github/github-mcp-server/pull/2512) — *"three meta-tools, and a
    chunk of conformance/CI matrix."*
15. **A prompt that can hang.** Any confirmation flow that blocks in a non-TTY is fatal in CI and in
    an agent loop. This is the reason §2.9 rejects MCP elicitation for ambiguity: rwr's contract is
    *refuse rather than guess*, and converting a deterministic refusal into a live human judgment
    call makes the result unreproducible, unrecorded, and — with the tool timeout running while the
    human deliberates — hangable.

---

## 6. Claims that could not be verified

Stated plainly so nobody treats them as established:

- **Whether MCP tool annotations change a Claude Code *permission prompt*.** The binary demonstrably
  reads `readOnlyHint` (for concurrency and read-only classification), `destructiveHint` and
  `openWorldHint`; `idempotentHint` is read nowhere. Whether any of them gate a confirmation dialog
  is unconfirmed — the docs and [#87452](https://github.com/anthropics/claude-code/issues/87452)
  suggest they do not.
- **First-party telemetry on whether agents benefit from AST-dump tools.** None exists in either
  direction. §2.3's conclusion rests on ast-grep's published rule-authoring failures and on token
  arithmetic, not on measured usage.
- **`_meta["anthropic/requiresUserInteraction"]` and `_meta["anthropic/maxResultSizeChars"]`** are
  observed in Claude Code's own tool definitions; they are vendor extensions, not MCP spec, and
  their exact semantics are inferred.
- **Ruby LSP indexing and find-references timings** are user-reported figures from issue threads
  ([#1316](https://github.com/Shopify/ruby-lsp/issues/1316),
  [#3051](https://github.com/Shopify/ruby-lsp/issues/3051)), not benchmarks run for this document.
  The source-level findings in §1.4–§1.7 *were* read directly and are not in this category.
- **Whether Anthropic will expose `textDocument/rename`.** [#40282](https://github.com/anthropics/claude-code/issues/40282)
  is open and labelled `enhancement`; no maintainer commitment either way.

---

## 7. Sources

**Ruby LSP and the Claude Code integration**
- [Shopify/ruby-lsp](https://github.com/Shopify/ruby-lsp) — [request handlers](https://github.com/Shopify/ruby-lsp/tree/main/lib/ruby_lsp/requests), [`reference_finder.rb`](https://github.com/Shopify/ruby-lsp/blob/main/lib/ruby_indexer/lib/ruby_indexer/reference_finder.rb), [`rename.rb`](https://github.com/Shopify/ruby-lsp/blob/main/lib/ruby_lsp/requests/rename.rb), [`prepare_rename.rb`](https://github.com/Shopify/ruby-lsp/blob/main/lib/ruby_lsp/requests/prepare_rename.rb), [`declaration_listener.rb`](https://github.com/Shopify/ruby-lsp/blob/main/lib/ruby_indexer/lib/ruby_indexer/declaration_listener.rb), [`configuration.rb`](https://github.com/Shopify/ruby-lsp/blob/main/lib/ruby_indexer/lib/ruby_indexer/configuration.rb), [`setup_bundler.rb`](https://github.com/Shopify/ruby-lsp/blob/main/lib/ruby_lsp/setup_bundler.rb)
- [Ruby LSP design and roadmap](https://shopify.github.io/ruby-lsp/design-and-roadmap.html) · [add-ons](https://shopify.github.io/ruby-lsp/add-ons.html)
- Issues: [#1316 indexing speed](https://github.com/Shopify/ruby-lsp/issues/1316) · [#2288 hangs at 0%](https://github.com/Shopify/ruby-lsp/issues/2288) · [#3051 35s find-references](https://github.com/Shopify/ruby-lsp/issues/3051)
- [claude-plugins-official PR #106](https://github.com/anthropics/claude-plugins-official/pull/106) · [`marketplace.json`](https://github.com/anthropics/claude-plugins-official/blob/main/.claude-plugin/marketplace.json) · [plugin page](https://claude.com/plugins/ruby-lsp)
- Claude Code issues: [#40282 expose rename/codeAction](https://github.com/anthropics/claude-code/issues/40282) · [#30948 workspaceSymbol query](https://github.com/anthropics/claude-code/issues/30948) · [#44767 empty results](https://github.com/anthropics/claude-code/issues/44767)
- [Damian Galarza, "Ruby LSP Now Has Official Claude Code Support"](https://www.damiangalarza.com/posts/2026-03-13-ruby-lsp-claude-code/)
- [Shopify/rubydex](https://github.com/shopify/rubydex) — [`base_tool.rb`](https://github.com/Shopify/rubydex/blob/main/lib/rubydex/mcp_server/tools/base_tool.rb)

**MCP**
- [MCP 2026-07-28 schema](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/schema/2026-07-28/schema.ts) · [pagination](https://modelcontextprotocol.io/specification/2026-07-28/server/utilities/pagination) · [`_meta`](https://modelcontextprotocol.io/specification/2026-07-28/basic#meta)
- [ast-grep-mcp](https://github.com/ast-grep/ast-grep-mcp) — [#7 dormant](https://github.com/ast-grep/ast-grep-mcp/issues/7) · [#13 token cap](https://github.com/ast-grep/ast-grep-mcp/issues/13) · [#14](https://github.com/ast-grep/ast-grep-mcp/issues/14) · [#29 LLM syntax struggle](https://github.com/ast-grep/ast-grep-mcp/issues/29) · [#31](https://github.com/ast-grep/ast-grep-mcp/issues/31) · [PR #36 (closed unmerged)](https://github.com/ast-grep/ast-grep-mcp/pull/36)
- [Semgrep MCP server](https://github.com/semgrep/semgrep/blob/develop/cli/src/semgrep/mcp/server.py) · [models.py](https://github.com/semgrep/semgrep/blob/develop/cli/src/semgrep/mcp/models.py) · [semgrep/mcp (deprecated)](https://github.com/semgrep/mcp)
- [oraios/serena](https://github.com/oraios/serena) — [`tools_base.py`](https://github.com/oraios/serena/blob/main/src/serena/tools/tools_base.py) · [claude-code context](https://github.com/oraios/serena/blob/main/src/serena/resources/config/contexts/claude-code.yml)
- [codemod-com/codemod MCP](https://github.com/codemod-com/codemod/tree/main/crates/mcp) · [ruby.txt node types](https://raw.githubusercontent.com/codemod-com/codemod/main/crates/mcp/src/data/node_types/ruby.txt)
- [github/github-mcp-server PR #2512](https://github.com/github/github-mcp-server/pull/2512) · [dgageot/mcp-ast-grep](https://github.com/dgageot/mcp-ast-grep)
- Claude Code MCP issues: [#79944 dropped text block](https://github.com/anthropics/claude-code/issues/79944) · [#86032 error structuredContent](https://github.com/anthropics/claude-code/issues/86032) · [#86142 draft-07 schema](https://github.com/anthropics/claude-code/issues/86142) · [#85442](https://github.com/anthropics/claude-code/issues/85442) · [#84207](https://github.com/anthropics/claude-code/issues/84207) · [#84602](https://github.com/anthropics/claude-code/issues/84602) · [#87452](https://github.com/anthropics/claude-code/issues/87452)
- [Anthropic, Writing effective tools for AI agents](https://www.anthropic.com/engineering/writing-tools-for-agents)
- [Claude Code MCP docs](https://code.claude.com/docs/en/mcp) · [tools reference](https://code.claude.com/docs/en/tools-reference)
- [oh-my-openagent #5313 (MCP vs skill)](https://github.com/code-yeongyu/oh-my-openagent/issues/5313)

**CLI UX**
- [clig.dev — Command Line Interface Guidelines](https://clig.dev/)
- ripgrep: [`main.rs`](https://github.com/BurntSushi/ripgrep/blob/master/crates/core/main.rs) · [`flags/mod.rs`](https://github.com/BurntSushi/ripgrep/blob/master/crates/core/flags/mod.rs) · [`printer/json.rs`](https://github.com/BurntSushi/ripgrep/blob/master/crates/printer/src/json.rs) · [#152 sorting](https://github.com/BurntSushi/ripgrep/issues/152) · [#189 help](https://github.com/BurntSushi/ripgrep/issues/189) · [#645 gitignore vs git-tracked](https://github.com/BurntSushi/ripgrep/issues/645)
- ast-grep: [run reference](https://ast-grep.github.io/reference/cli/run.html) · [project config](https://ast-grep.github.io/guide/project/project-config.html) · [AI prompting guide](https://ast-grep.github.io/advanced/prompting.html) · [agent blog](https://ast-grep.github.io/blog/ast-grep-agent.html) · [#575 ERROR tolerance](https://github.com/ast-grep/ast-grep/issues/575) · [#1232 zero-based lines](https://github.com/ast-grep/ast-grep/issues/1232) · [#56](https://github.com/ast-grep/ast-grep/issues/56) / [#778](https://github.com/ast-grep/ast-grep/issues/778) / [#1659](https://github.com/ast-grep/ast-grep/issues/1659) name collision
- semgrep: [`semgrep_output_v1.atd`](https://github.com/semgrep/semgrep-interfaces/blob/main/semgrep_output_v1.atd) · [jsonschema](https://github.com/semgrep/semgrep-interfaces/blob/main/semgrep_output_v1.jsonschema) · [`git.py`](https://github.com/semgrep/semgrep/blob/develop/cli/src/semgrep/git.py) · [PR #4571 baseline diffing](https://github.com/semgrep/semgrep/pull/4571)
- ruff: [`linter.rs` MAX_ITERATIONS](https://github.com/astral-sh/ruff/blob/main/crates/ruff_linter/src/linter.rs) · [#22891 rayon nondeterminism](https://github.com/astral-sh/ruff/issues/22891) · exclude cluster [#1220](https://github.com/astral-sh/ruff/issues/1220) / [#2034](https://github.com/charliermarsh/ruff/issues/2034) / [#4127](https://github.com/astral-sh/ruff/issues/4127) / [#9023](https://github.com/astral-sh/ruff/issues/9023) · [ruff-pre-commit #19](https://github.com/astral-sh/ruff-pre-commit/issues/19)
- rubocop: [`options.rb`](https://github.com/rubocop/rubocop/blob/master/lib/rubocop/options.rb) · [#893 force-exclusion rationale](https://github.com/rubocop/rubocop/issues/893) · [#12667](https://github.com/rubocop/rubocop/issues/12667)
- biome: [#2267 `--write` naming](https://github.com/biomejs/biome/issues/2267) · [#7394 explicit paths vs exclude](https://github.com/biomejs/biome/discussions/7394)
- [jq `src/main.c` exit codes](https://github.com/jqlang/jq/blob/master/src/main.c) · [jq manual](https://jqlang.org/manual/)
- [cargo JSON messages](https://doc.rust-lang.org/cargo/reference/external-tools.html#json-messages) · [rustc JSON / Applicability](https://doc.rust-lang.org/rustc/json.html)
- [difftastic tricky cases](https://difftastic.wilfred.me.uk/tricky_cases.html)
- [clap derive reference](https://docs.rs/clap/latest/clap/_derive/index.html) · [anstyle / anstream](https://github.com/rust-cli/anstyle) · [no-color.org](https://no-color.org) · [bixense CLICOLOR](https://bixense.com/clicolors/) · [npm/ci-detect](https://github.com/npm/ci-detect)
- [gh help exit-codes](https://cli.github.com/manual/gh_help_exit-codes) · [gh help formatting](https://cli.github.com/manual/gh_help_formatting) · [gh help environment](https://cli.github.com/manual/gh_help_environment)
- [ESLint: new config system, part 1](https://eslint.org/blog/2022/08/new-config-system-part-1/)
- [golangci-lint #3320 fail-open on bad ref](https://github.com/golangci/golangci-lint/issues/3320)

**Hooks and packaging**
- [lefthook configuration](https://lefthook.dev/configuration/run/) · [`internal/git/repo.go`](https://github.com/evilmartians/lefthook/blob/master/internal/git/repo.go)
- [pre-commit: new hooks](https://github.com/pre-commit/pre-commit.com/blob/main/sections/new-hooks.md) · [#3397 file batching](https://github.com/pre-commit/pre-commit/issues/3397)
- [crate-ci/typos](https://github.com/crate-ci/typos) — [docs/pre-commit.md](https://github.com/crate-ci/typos/blob/master/docs/pre-commit.md) · [docs/design.md](https://github.com/crate-ci/typos/blob/master/docs/design.md)
- [overcommit — "pre-commit hooks cannot have side effects"](https://github.com/sds/overcommit)
