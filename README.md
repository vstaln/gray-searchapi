<p align="center">
  <img src="assets/gray-logo.svg" alt="gray" width="96">
</p>
<h1 align="center">gray-searchapi</h1>
<p align="center">Keyed web search — Brave, SerpApi, and Linkup behind one results shape.</p>
<p align="center">
  <a href="https://github.com/vstaln/gray-searchapi/blob/main/LICENSE"><img alt="MIT License" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
  <img alt="gray plugin" src="https://img.shields.io/badge/gray-plugin-7aa2f7.svg">
  <img alt="rust" src="https://img.shields.io/badge/built%20with-rust-orange.svg">
</p>

Three API-keyed search providers, one tool each. Every tool answers the same
shape — `{"results":[{"title","url","snippet"}]}` — so a caller can switch
providers without re-parsing. Keyless engines live in
[gray-web](../gray-web) (`web_query`); this plugin covers the keyed side.

## Tools

| tool | key env var | endpoint | extras |
|---|---|---|---|
| `brave_web_search` | `BRAVE_API_KEY` | `api.search.brave.com/res/v1/web/search` | `count` 1–20 (default 10) |
| `serpapi_search` | `SERPAPI_KEY` | `serpapi.com/search.json` | `engine` (default `google`), `count` 1–100 |
| `linkup_search` | `LINKUP_API_KEY` | `api.linkup.so/v1/search` (POST) | `depth` `standard`\|`deep`, `count` |

All take a required `query` string. Result content is compact JSON:

```json
{"results":[{"title":"…","url":"…","snippet":"…"}]}
```

## Keys

Each provider reads its key from the environment first, then
`~/.gray/searchapi/config.json`:

```sh
export BRAVE_API_KEY=…        # or persist:
searchapi set --brave KEY --serpapi KEY --linkup KEY
```

`set` writes `{"brave_api_key","serpapi_key","linkup_api_key"}` with mode
0600. A missing key is a tool error naming the env var — no silent fallbacks.

## Command

`/searchapi status` — one line per provider showing whether its key is set
and where it came from (`env` or `config`).

## Wire methods

`plugin/manifest`, `plugin/tools` (protocol 1.3), `plugin/shutdown`,
`tool/call`, `command/run`. No hooks, no host capabilities. All network I/O
shells out to curl (15s cap).

## Install

```sh
gray plugin install searchapi
```

## Develop

```sh
cargo test
python3 ../.port-tasks/qa/drive.py target/release/gray-searchapi
gray account check      # entry point + manifest handshake
gray account publish    # check → build → release → publish to the gray registry
```

Bump `version` in `Cargo.toml` before each `publish`; the registry refuses to
republish a version.

---
Part of the [gray](https://github.com/vstaln/gray) plugin ecosystem —
the open-source AI agent harness. <https://gray.alignment.id>
