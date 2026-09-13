# Herdr Command Palette

A macOS Herdr plugin that brings commands, installed plugin actions, and live
workspace, tab, and agent search into one keyboard-first popup.

## Features

- Search context-valid Herdr commands and installed plugin actions.
- Search and focus live workspaces, tabs, and agents without retargeting stale
  results by name.
- Rename the focused pane or the agent in it from the palette.
- Keep input, rendering, subprocess output, retries, and polling bounded.

## Requirements and security model

- macOS
- Herdr 0.7.4 or newer, available as `herdr`
- Rust 1.88 or newer for source builds

Plugins execute local commands with your user permissions and are not sandboxed.
Read this README and [`herdr-plugin.toml`](herdr-plugin.toml) before
installing a plugin. Use only sources and tags you trust.

## Install

Install the reviewed, immutable source release:

```bash
herdr plugin install sendhil/herdr-command-palette --ref v0.1.1
```

Herdr clones the source, runs the manifest's locked release build, and links the
resulting plugin. Verify the registration:

```bash
herdr plugin list --plugin herdr.command-palette --json
```

Confirm the result reports plugin ID `herdr.command-palette`, version
`0.1.1`, an enabled registration, the expected source, and its manifest path.

## Install with a coding agent

Give a coding agent this request exactly:

> Install `sendhil/herdr-command-palette` at exactly `v0.1.1` using Herdr's
> plugin installer. First read this README and `herdr-plugin.toml`. Verify that
> this is macOS, Herdr is 0.7.4 or newer, and Rust is 1.88 or newer. Before
> changing anything, query the existing `herdr.command-palette` registration.
> Do not use `sudo`, `curl | sh`, force flags, or unrelated package managers.
> Do not overwrite or unlink a registration from another source without my
> explicit approval. After reviewing Herdr's install preview, run
> `herdr plugin install sendhil/herdr-command-palette --ref v0.1.1 --yes`
> because coding-agent command execution is non-interactive, then verify the
> plugin ID, enabled state, source, version, and manifest path. Resolve my real
> Herdr configuration source, including symlinks or managed dotfiles,
> and inspect existing keybindings. Ask my permission before adding or
> changing `cmd+shift+p`; never silently overwrite a conflict. If I use WezTerm,
> inspect its real configuration and ask before adding the forwarding rule from
> this README. Reload Herdr and terminal configuration only after I approve the
> changes. Report every command result and every changed path.

## Configure `Cmd-Shift-P`

After checking for conflicts and with your approval, add this to the real Herdr
key configuration file:

```toml
[[keys.command]]
key = "cmd+shift+p"
type = "plugin_action"
command = "herdr.command-palette.open"
description = "open command palette"
```

Some terminals need an explicit modified-key mapping. WezTerm uses
`Ctrl-Shift-P` for its own command palette and does not send `Cmd-Shift-P` to
Herdr in a distinguishable form by default. The following entry assumes your
existing configuration already defines `wezterm` with `require("wezterm")` and
a `config.keys` table; merge the entry into that table rather than replacing
your other bindings:

```lua
{
  key = "p",
  mods = "CMD|SHIFT",
  action = wezterm.action_callback(function(window, pane)
    local process = pane:get_foreground_process_name() or ""
    if process:match("/herdr$") then
      pane:send_text("\x1b[112:80;10u")
    else
      window:perform_action(
        wezterm.action.SendKey({ key = "p", mods = "CMD|SHIFT" }),
        pane
      )
    end
  end),
},
```

This preserves WezTerm's `Ctrl-Shift-P` command palette and passes
`Cmd-Shift-P` through normally when Herdr is not the foreground process. Reload
Herdr and WezTerm after changing their configuration files.

## Usage and search scopes

Run the `Open command palette` action, then type to search. Press Enter on a
command or plugin action to run it. Press Enter on a live workspace, tab, or
agent result to focus that exact target and close the popup.

- `>` searches commands and plugin actions.
- `space:` searches current-session workspaces.
- `tab:` searches tabs across all workspaces.
- `agent:` searches agents across all workspaces.

An empty search shows scope hints. The palette keeps the snapshot captured when
it opened. If a selected live target is stale, it refreshes once and retries
only the same typed stable ID; it never chooses another result with the same
label.

## Focused-pane rename behavior

`Rename focused pane` appears whenever the runtime snapshot has a focused pane.
Its field starts with that pane's current custom label. Submit a nonempty value
to rename that exact pane, or submit an empty field to clear its custom label.
This is separate from the agent name managed by `Rename focused agent`.

## Focused-agent rename behavior

`Rename focused agent` appears only when the focused pane has an agent. Its
field starts with that agent's current custom name. Submit a nonempty value to
rename that exact pane, or submit an empty field to clear its custom name.

## Update and uninstall

To update to a reviewed release, rerun the install command with the desired
explicit tag and verify the resulting JSON registration:

```bash
herdr plugin install sendhil/herdr-command-palette --ref v0.1.1
herdr plugin list --plugin herdr.command-palette --json
```

To remove the installed plugin:

```bash
herdr plugin uninstall herdr.command-palette
```

For a local development link, use `herdr plugin unlink herdr.command-palette`
only after confirming that the existing registration is the local checkout you
intend to remove.

## Compatibility and known limitation

The plugin supports macOS and requires Herdr 0.7.4 or newer. `agent.prompt`
appears only when the Herdr API schema advertises it. `agent.start` appears only
when the schema advertises its newer `name`, `kind`, `pane_id`, and `timeout_ms`
contract; the older 0.7.4 argv-shaped contract is intentionally hidden.

A plugin action that synchronously opens another popup can race palette teardown
and receive `ui_busy`. Retry after the palette closes.

## Development and validation

```bash
git clone https://github.com/sendhil/herdr-command-palette.git
cd herdr-command-palette
cargo build --release --locked
herdr plugin link "$PWD"
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
cargo test --release --locked --test performance -- --ignored --nocapture
```

`herdr plugin link` registers the local directory but does not run the manifest
build command, so build the release binary first. Rebuild and relink after
source changes.

## Contributing, security, and license

See [CONTRIBUTING.md](CONTRIBUTING.md), [SECURITY.md](SECURITY.md), and
[LICENSE](LICENSE).
