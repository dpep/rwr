# Installing the rwr skill

Two separate things: the **binary**, which does the work, and the **skill**,
which teaches Claude to drive it. You need both, and they install independently.

## The binary

```sh
cargo install rwr
```

No setup after that — there is no index and no cache. rwr parses, answers, and
exits, so there is nothing to warm or invalidate.

The rule pack is compiled into the binary, so `rwr check all` works from any
directory without a checkout of this repo.

## The skill

Two routes. They suit different people, so ask rather than picking.

### The marketplace plugin (the better default)

```
/plugin marketplace add dpep/myclaude
/plugin install rwr@myclaude
```

One install, and `claude plugin update rwr@myclaude` keeps it current.

Prefer this unless there's a reason not to. A skill file describing an older
binary than the one installed is the failure mode worth avoiding, and this is
the route that gets updates.

> `myclaude` is private while rwr settles. The skill moves to `code@dpep`
> alongside `rq` and `gqls` once it has been used in anger.

### A local copy

```sh
mkdir -p ~/.claude/skills/rwr
cp claude/rwr-skill.md ~/.claude/skills/rwr/SKILL.md
```

Just this skill, nothing else — right when the user wants nothing else from the
marketplace.

Either way, restart Claude Code after installation.
