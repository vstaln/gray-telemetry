<div align="center">
  <img alt="gray-telemetry" src="assets/icon.svg" width="120" height="120" />
  <h1>gray-telemetry</h1>
  <p><strong>One sidecar for every local event ledger.</strong></p>
  <p>
    <a href="https://gray.alignment.id">Website</a> ·
    <a href="https://github.com/vstaln/gray-telemetry">Source</a> ·
    <a href="https://github.com/vstaln/gray">gray</a>
  </p>
  <p>
    <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-1c1c20?style=flat-square&labelColor=0a0a0b" /></a>
    <a href="https://www.rust-lang.org"><img alt="Built with Rust" src="https://img.shields.io/badge/built%20with-rust-1c1c20?style=flat-square&labelColor=0a0a0b&logo=rust&logoColor=d4a373" /></a>
    <img alt="gray plugin" src="https://img.shields.io/badge/gray-plugin-1c1c20?style=flat-square&labelColor=0a0a0b&color=7aa2f7" />
  </p>
</div>

<br/>

```bash
gray plugin install telemetry
```

`gray-telemetry` merges three plugins that each record host events
into one sidecar process. Each module keeps its own `~/.gray/<orig>/` state
files and slash commands, so existing logs, ledgers and config keep working
after you uninstall the standalone versions.

## Replaces

| Module | Former plugin | Hooks / events | Commands | State |
|---|---|---|---|---|
| observe | `gray-observe` | `event/notify` (pre_tool, post_tool, turn_end) | `/observe [tail N]` | `~/.gray/observe/<date>.jsonl` |
| stamp | `gray-stamp` | `event/notify` (pre_tool, post_tool, turn_start, turn_end), `prompt/context` | `/stamps [clear\|path]` | `~/.gray/stamps/<session>.jsonl` |
| usage | `gray-usage` | `event/notify` (turn_end) | `/usage` `/tokens` `[all\|reset]` | `~/.gray/usage/config.json`, `~/.gray/usage/live/` |

No module defines tools. Every `event/notify` goes to all three modules.
`prompt/context` only goes to stamp, which adds its one-line "last turn"
timing note. `/telemetry` prints an overview of all three ledgers.

## Build from source

```sh
cargo build --release
gray plugin check ./target/release/telemetry
```

## License

MIT
