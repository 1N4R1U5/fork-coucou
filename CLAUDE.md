# Coucou for Linux — guide for AI coding agents

Unofficial Linux port of Coucou (upstream: github.com/Louis-CFM/coucou, macOS + Windows). Mochi, a small animated character at the top of the screen, shows Claude Code sessions and a few integrations, and lets the user approve, answer, chat and drop files from the island. This fork is kept separate from upstream's own Linux port: never merge upstream wholesale, only port isolated fixes by hand.

## Where things are
- `app/` — the Tauri 2 app. `app/src/` TypeScript front end (no framework), `app/src-tauri/` Rust backend, `app/hook/` the `coucou-hook` relay, `app/sounds/` the 28 WAV sounds.
- `docs/SPEC.md`, `docs/INTEGRATIONS.md` — behaviour, views, states, integrations (in French, written for the macOS app).
- `design/prototype/notch-buddy.html` — original prototype, the visual source of truth. `design/captures/` — target screenshots.
- Comments mentioning `*.swift` files refer to upstream's macOS app, which this port follows.

## Build
```
cd app && npm install && npx tauri build --bundles appimage
```
Checks: `npm run build` (tsc + vite) and `cargo test` in `app/`.

## Rules
- TypeScript + Rust (Tauri 2). No third-party dependencies unless truly unavoidable. The character is drawn in code (Canvas 2D), no Rive/Lottie/images.
- Secrets live in the Secret Service (keyring crate), never on disk or in git.
- No telemetry. Network calls only to services the user configured.
- Never block Claude Code: if the app doesn't answer, the hook exits immediately.
- Never overwrite `~/.claude/settings.json`: dated backup, merge, show the diff, write only after the user confirms.
- Never send an email or approve a Claude Code permission without an explicit click.
- Performance: 0 % CPU when the island is hidden.
- Keep the identifier `fr.louisraille.coucou` (keyring items and settings depend on it).
- Visual changes must match the prototype and the screenshots in `design/captures/`.
