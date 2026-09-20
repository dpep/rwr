# rwr documentation

`rwr` is `rg`/`sed` for Ruby *programs* rather than Ruby *text*. It parses with
Prism, so a comment, a string literal, or a heredoc body that happens to contain
your pattern is not a match.

- **[Getting started](getting-started.md)** — install it, find something, change
  something.
- **[Writing rules](writing-rules.md)** — the rule file, `where:` predicates,
  and the fixtures that pin what a rule does.
- **[rwr on a pull request](github-actions.md)** — inline suggestions a
  reviewer can apply, and the two settings that decide whether it works.
- **[Suppressing findings](suppressing.md)** — the three ways to stop a finding
  failing a run, and which one you actually mean.
- **[The shipped pack](../rules/README.md)** — what `check all` runs, and the
  safety notes on the rules it holds back.

The [README](../README.md) is the tour; these are the details.

Design notes, decisions and research live in [internal/](internal/) — the
reasoning behind the tool rather than instructions for using it.

The shell examples in these guides, in `README.md` and in the Claude skill are **run by the test
suite** (`tests/docs_examples.rs`) against a fixture repo, and each must produce the effect its verb
promises — not merely exit 0. Two examples had shipped broken before that gate existed. If an
example is illustrative rather than runnable, mark its fence ```sh ignore; a `<placeholder>` in the
line, or a line that does not start with `rwr`, is skipped already.
