# Coucou for Linux

Unofficial Linux port of [Coucou](https://github.com/Louis-CFM/coucou) by Louis Raillé.

Mochi sits at the top of the screen and shows your Claude Code sessions. From there you can approve permissions, chat with Claude and drop files.

This port is not affiliated with or supported by the original project. Report bugs here, not upstream.

## Build

Requirements: Rust, Node 20+, and the WebKitGTK development packages.

```bash
# Debian / Ubuntu
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libdbus-1-dev

cd app
npm install
npx tauri build --bundles appimage
```

The AppImage lands in `app/target/release/bundle/appimage/`. For development, run `npm run tauri dev`.

## Setup

Open **Settings** from the tray icon:

- **Claude Code → Install hooks**: backs up `~/.claude/settings.json`, shows the diff, and writes it only after you confirm.
- **Chat**: runs through Claude Code (`claude -p`) on your Claude subscription. You can add an Anthropic API key instead, which is stored in the Secret Service (GNOME Keyring, KWallet) and takes over when set.

If Coucou isn't running, the hook exits immediately, so Claude Code is never blocked.

Logs: `~/.local/share/coucou/coucou.log`.

## Layout

```
app/
  src/          front end (TypeScript, no framework)
  src-tauri/    Rust backend: window, hook socket, Claude API, integrations
  hook/         coucou-hook, the Claude Code relay
  sounds/       the 28 WAV sounds
design/         original prototype and reference captures
docs/           behaviour spec and integrations (French)
```

## License

The code is under the [MIT License](LICENSE). The names "Coucou" and "Mochi", the character, the icon and the sounds belong to Louis Raillé ([LICENSE-ASSETS.md](LICENSE-ASSETS.md)).
