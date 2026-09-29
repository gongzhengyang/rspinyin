# rspinyin

<!-- OB-1: Slint attribution badge, required by the Slint Royalty-free 2.0 licence
     (ADR-0000, decision 1). An input method has neither an "About" dialog nor a
     splash screen, so a badge on this public page is the only path the licence
     leaves open. The link has to stay reachable: `scripts/gen-licenses.sh --check`
     asserts the badge and the link, and `--check-links` probes https://slint.dev. -->
[![Built with Slint](https://img.shields.io/badge/built%20with-Slint-4C9AFF)](https://slint.dev)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#licence)

An offline-first Linux pinyin input method: an in-process Fcitx5 addon with a fully
self-drawn candidate window.

[中文文档](README.zh.md)

## What it is

rspinyin is a Chinese pinyin input method for Linux desktops. It is a Fcitx5 addon
rather than a separate program: Fcitx5 loads it into its own process, and there is no
daemon, no socket and no client to install beside it.

The candidate window is drawn by rspinyin itself, not by Fcitx5's ClassicUI. Layout,
theme, spring animation and rasterisation are ours, which is what makes the window
look and behave the same on X11 and on Wayland.

Everything runs offline. The dictionary is a compiled container read through a
zero-copy `mmap`, the words you commit are recorded in a local key-value store, and no
code path in the product can open a network connection. That last point is a promise
the build asserts, not a description of the current code: `scripts/check-no-network.sh`
rejects any network crate in the dependency closure, and `scripts/runtime-socket-check.sh`
asserts that a live Fcitx5 session holding the plugin has no IP socket at all.

## Requirements

- **Linux** with **Fcitx5 5.1.7 or newer**. Both addon descriptors declare
  `core:5.1.7`, and the two must stay equal: a UI addon that loads where the engine
  does not leaves you with half an input method.
- **A display server: X11 or Wayland.** Both frontends are listed as optional
  dependencies, so either one works and neither is required at build time. The
  candidate window positions itself through the X11 or the Wayland backend that is
  present.
- **To build from source:**
  - Rust. The toolchain is pinned by `rust-toolchain.toml` (1.98.0) and the minimum
    supported version is 1.85.
  - A C++ compiler and `pkg-config` with the Fcitx5 development packages
    (Debian/Ubuntu: `libfcitx5core-dev`) for the `fcitx5-host` feature, which is what
    links the real Fcitx5 C ABI. Without the feature the Rust side still builds and
    tests on a machine that has no Fcitx5 installed.
  - `python3` 3.11 or newer for the audit scripts under `scripts/`.

## Install

```bash
# 1. Fetch the dictionary sources and compile them (writes data/compiled/base.dict).
data/fetch.sh
cargo run -p xtask -- dictc

# 2. Build and install. The script builds as you and elevates only the file copies.
bash packaging/install.sh
```

`packaging/install.sh` resolves every destination from the target system's own Fcitx5
installation with `pkg-config` instead of hardcoding a path: Fcitx5's addon directory
is `/usr/lib/x86_64-linux-gnu/fcitx5` on Debian and Ubuntu, `/usr/lib64/fcitx5` on
Fedora and `/usr/lib/fcitx5` on Arch, and a hardcoded path installs the plugin where
Fcitx5 never looks for it.

Useful options: `--dry-run` prints the plan and changes nothing, `--skip-build`
installs what is already in `target/`, `--prefix PATH` and `--destdir PATH` relocate
the installation, and `--no-sudo` never elevates. `just install` is the same command.

Installed files:

| File | Destination |
|---|---|
| `librspinyin.so` | `<libdir>/fcitx5/` — the engine addon |
| `librspinyin_ui.so` | `<libdir>/fcitx5/` — the candidate-window addon |
| `rspinyin.conf`, `rspinyin-ui.conf` | `<datadir>/fcitx5/addon/` |
| `rspinyin.conf` (input method) | `<datadir>/fcitx5/inputmethod/` |
| `base.dict` | `<datadir>/rspinyin/` |
| `fcitx-rspinyin.svg`, `fcitx-rspinyin.png` | `<datadir>/icons/hicolor/` |

Then restart Fcitx5 and add "Rust Pinyin" to the input method list. To remove the
plugin again, `bash packaging/uninstall.sh` restores whatever the installation
replaced.

## Configuration

The configuration file is `$XDG_CONFIG_HOME/rspinyin/config.toml`
(`~/.config/rspinyin/config.toml` by default), created `0600` inside a `0700`
directory. It is written with every key present and a comment above each one the first
time the plugin starts without one, so the file is its own documentation.

```toml
schema_version = 1

[engine]
punct_mode = "chinese"          # "chinese" or "english"
full_width = false
auto_english_on_uppercase = true
passthrough_url = true
max_raw_len = 64                # 1..=64

[ui]
client_preedit = false          # show the composing text in the application instead
max_per_row = 5                 # 3..=9
max_width_dp = 720              # 220..=1200
base_alpha = 217                # 0..=255

[theme]
scheme = "auto"                 # "auto", "light" or "dark"
accent = "#4C9AFF"

[keys]
digit_zero = "passthrough"      # "passthrough" or "flip"
flip_keys = ["minus", "equal", "up", "down"]
highlight_keys = ["tab", "shift_tab"]

[data]
durability = "eventual"         # "eventual" batches writes, "immediate" flushes each

[diagnostics]
level = "info"
log_rotation_mb = 8
log_keep_files = 3
```

Every key is optional: an absent key keeps the built-in default shown above. The file
is read at startup and again whenever Fcitx5 asks the addon to reload, and a change
takes effect on the next keystroke without a restart. A file that cannot be read or
parsed never stops the input method: the configuration already in force is kept, the
reason is reported in the log, and an in-progress composition is never reset.

## Architecture

rspinyin ships as **two shared objects** that Fcitx5 loads independently:

```text
librspinyin.so      Category=InputMethod   the decoding engine
librspinyin_ui.so   Category=UI            the candidate window
```

They meet only inside Fcitx5. The engine decodes a keystroke and writes the preedit and
the candidate list into the host's `InputContext`; the UI addon reads that context's
`inputPanel()` and draws it. Neither library `dlopen`s the other, they share no static
state, and there is no IPC between them — which is exactly how Fcitx5's own ClassicUI
works.

Two addons rather than one, because Fcitx5 picks the active user interface from the
addons it discovered with `Category=UI`, and an addon belongs to exactly one category:
an addon registered as an input method cannot also be the user interface. The
descriptors say `Library=librspinyin` and `Library=librspinyin_ui`, and Fcitx5 appends
`.so` to the value verbatim.

Under the hood the workspace is a set of small crates with one-way dependencies
(`ime-types ← ime-core ← ime-dict ← ime-config ← ime-ui ← ime-fcitx5`, with
`ime-ui-addon` beside `ime-fcitx5`), checked by `scripts/check-deps.sh`:

- `ime-types` is the frozen contract: the error types, the ABI version, the traits the
  layers meet through.
- `ime-core` is the decoder — segmentation, Viterbi k-best, session state — as a pure
  function with no filesystem, clock or environment.
- `ime-dict` owns the dictionary format, the `mmap`-backed lexicon and the user
  frequency store.
- `ime-config` is the TOML model, the loader and the hot reload.
- `ime-ui` is the view layer: geometry, theme, spring animation, software raster, and
  the X11 and Wayland backends.
- `ime-fcitx5` and `ime-ui-addon` are the two C ABI layers, one per shared object.
- `ime-diag` is the diagnostics leaf: logging, redaction, crash records.

Two threads, physically isolated. The Fcitx5 main loop only turns a key into a decoded
frame and posts it; the UI thread owns the display connection, the raster buffers and
the renderer, and wakes through an `eventfd` rather than a polling timer. Rendering
never runs on the host thread, and the host's `InputContext` is never touched from the
UI thread.

## Privacy

rspinyin is offline by construction, and what it keeps on your disk is small and
documented. The full account — data flow, every file it writes with its path and
permissions, the redaction rules, and the limits of what we can protect — is in
[`docs/dev/privacy.md`](docs/dev/privacy.md).

The short version:

- **No network.** No outbound connection, no update check, no telemetry. The build
  asserts the absence of network crates, and a runtime check asserts that a live
  session holds no IP socket.
- **Nothing about what you type is logged.** Input text, preedit, candidate text and
  commit text are never passed to a log event in the first place; a redaction layer
  withholds them a second time if one is ever passed by mistake.
- **Learning is local and small.** Committing a word updates a local store under
  `$XDG_DATA_HOME/rspinyin/`, mode `0600`. It is yours: deleting the file loses the
  learning and nothing else.
- **Password boxes are recognised on a best-effort basis.** We rely on the application
  telling Fcitx5 that the field is a password field. An application that does not set
  that flag cannot be recognised as a password box by us, so please do not treat
  rspinyin as a last line of defence for a password.

## Licence

rspinyin is licensed under **MIT OR Apache-2.0**, at your option. The full texts are in
[`LICENSE-MIT`](LICENSE-MIT) and [`LICENSE-APACHE`](LICENSE-APACHE).

The candidate window is built with Slint under the
`LicenseRef-Slint-Royalty-free-2.0` licence. The obligations that licence places on
this project are recorded item by item in
[`docs/dev/licenses.md`](docs/dev/licenses.md); in summary:

- **`OB-1` — attribution.** This page carries the Slint attribution badge above and
  links to <https://slint.dev>. An input method has no "About" dialog and no splash
  screen, so a badge on a public page is the path the licence leaves open, and it has
  to stay reachable.
- **`OB-2` — no standalone distribution.** Slint is not distributed on its own. It is
  statically linked into `librspinyin_ui.so`, and the project builds and packages no
  Slint library of its own.
- **`OB-3` — no embedded use.** **This licence does not cover embedded systems.**
  Deploying rspinyin on an appliance panel, a point-of-sale terminal or an in-vehicle
  system requires a GPL-3.0 or a commercial Slint licence, obtained separately. A
  Linux desktop input method is a desktop application and is inside the granted scope;
  those deployments are not.
- **`OB-4` — no exposed Slint API.** The candidate window is not a programmable Slint
  surface: no Slint type appears in `ime-ui`'s public API, so a third party cannot
  program against it. `scripts/check-slint-leak.sh` enforces this.
- **`OB-5` — notices kept.** Slint's licence notices and the licence text shipped
  inside the Slint package are not removed or altered, and the text's digest is
  recorded in `docs/dev/licenses.md`.
- **`OB-6` — as is.** Slint is provided **"as is", without warranty of any kind**,
  either express or implied, including without limitation any warranties of
  merchantability or fitness for a particular purpose. The same applies to rspinyin
  itself, under both of its licences.

See [`docs/dev/licenses.md`](docs/dev/licenses.md) for the full audit and
[`docs/dev/adr/0000-upstream-decisions.md`](docs/dev/adr/0000-upstream-decisions.md)
for the decision record.

## Credits

- **[Slint](https://slint.dev)** (SixtyFPS GmbH) — the candidate window's UI toolkit,
  used under the Slint Royalty-free 2.0 licence.
- **[Fcitx5](https://github.com/fcitx/fcitx5)** (Fcitx developers) — the input method
  framework this plugin is an addon of, under LGPL-2.1-or-later. It is dynamically
  linked and not redistributed: your distribution provides it.
- **Dictionary sources** — [`mozillazg/pinyin-data`](https://github.com/mozillazg/pinyin-data)
  (MIT) for character readings, the Unicode
  [Unihan database](https://www.unicode.org/Public/UCD/latest/ucd/Unihan.zip) (Unicode
  License) for cross-checking them, and [`fxsjy/jieba`](https://github.com/fxsjy/jieba)
  (MIT) for words and word frequencies.
- Every Rust dependency, its declared licence and the branch the project relies on are
  listed in [`docs/dev/licenses.md`](docs/dev/licenses.md); the third-party notice
  shipped with a release is [`docs/dev/NOTICE`](docs/dev/NOTICE).
