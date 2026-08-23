# rwr documentation

`rwr` is `rg`/`sed` for Ruby *programs* rather than Ruby *text*. It parses with
Prism, so a comment, a string literal, or a heredoc body that happens to contain
your pattern is not a match.

- **[Getting started](getting-started.md)** — install it, find something, change
  something.
- **[Writing rules](writing-rules.md)** — the rule file, `where:` predicates,
  and the fixtures that pin what a rule does.
- **[rwr in GitHub Actions](github-actions.md)** — SARIF, annotations on a pull
  request, and the three settings that decide whether it works.
- **[Suppressing findings](suppressing.md)** — the three ways to stop a finding
  failing a run, and which one you actually mean.
- **[The shipped pack](../rules/README.md)** — what `check all` runs, and the
  safety notes on the rules it holds back.

The [README](../README.md) is the tour; these are the details.

Design notes, decisions and research live in [internal/](internal/) — the
reasoning behind the tool rather than instructions for using it.
