# Contributing

## Getting started

```bash
cd app
npm install
npm run tauri dev
```

`npm run dev` alone serves the front end in a browser, which is enough to work on the island's looks. `dev/upload-preview.html` replays the file-drop animation on a loop.

## Rules

- No third-party dependencies unless there's really no other way.
- Secrets go in the Secret Service, never on disk or in git.
- No telemetry, no network calls except to services the user configured.
- Never block Claude Code: if the app doesn't answer, the hook must exit right away.
- Never write `~/.claude/settings.json` without a backup and the user's confirmation.
- 0 % CPU when the island is hidden.

## Before a pull request

```bash
cd app
npm run build
cargo test
```

One topic per PR, with a screenshot for anything visual.
