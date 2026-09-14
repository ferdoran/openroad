# Settings that apply live

*Mechanism reference for #647. The enforced version of this document is
`client/src/plugins/settings/live.rs`; if the two disagree, the code is right
and this file is stale.*

## The problem it solves

openroad has two settings resources and, until #647, no shared rule about how a
change in either reaches the thing it configures:

| Resource | File | Edited by |
|---|---|---|
| `ClientConfig` | `config.yaml` (untracked; copy `config.example.yaml`) | the user, in an editor |
| `GameOptions` | `user_settings.yaml` (machine-written) | the in-game options window |

Each option was wired ad hoc. Some were read per frame and were live by
accident; some were read once in `Plugin::build` and could never change; some
were parsed, persisted and read by nothing at all — the dead wires the tree has
been fixing one at a time (#373 audio, #332 auto-potion, #605 mouse
shortcut-swap, #379 camera pane). Without one rule, every new option repeats the
same three bugs.

## The rule

> A value derived from a settings resource is produced by an `apply_*` system,
> gated on `resource_changed`, living in the module that owns the consumer —
> **never** in `Plugin::build`.

Concretely:

```rust
impl Plugin for ChatPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChatColors>().add_systems(
            PreUpdate,
            apply_chat_colors.run_if(crate::plugins::settings::live::config_changed),
        );
    }
}

pub fn apply_chat_colors(config: Res<ClientConfig>, mut colors: ResMut<ChatColors>) {
    *colors = config.chat.colors.resolved();
}
```

Three properties make this work and are worth stating, because each one is a
decision:

1. **`Plugin::build` runs exactly once.** Anything derived there is frozen for
   the process lifetime. That is not a style preference — it is the precise
   shape of every "the setting does nothing until you restart" report.
2. **`resource_changed` is true on the frame the resource is inserted.** So the
   apply system also *seeds* the derived value at boot: one code path, no
   separate initialisation to keep in sync.
3. **Apply systems run in `PreUpdate`.** Consumers run in `Update`, so a change
   is visible to them in the *same* frame rather than one frame late.

## The audit

`SETTINGS_AUDIT` in `live.rs` records, per `ClientConfig` group, whether it is
`Live`, `Restart` (with the reason), `Mixed` (both, per field — see below) or a
`DeadWire` (with an issue). Two tests keep it honest:

- `the_audit_covers_every_client_config_group` scrapes the field names out of
  `struct ClientConfig` and fails when a group has no verdict — so a setting
  cannot be added and quietly wired to nothing.
- `no_plugin_build_derives_a_value_from_the_config` scans every
  `fn build(&self, app: &mut App)` in the crate for a `ClientConfig` read. The
  only exceptions are the four places that decide **plugin registration**
  (`net/plugin.rs`, `scenes/mod.rs`, `dev/mod.rs`, `environment/mod.rs`); Bevy
  cannot add or remove a plugin after the `App` is built, so those are
  restart-only by construction. Each exception must still match a real file, or
  the test fails as stale.

### A group is a config section, not a lifetime (#730)

`ClientConfig` groups are `config.yaml` sections, so one of them can hold a
field that decides plugin registration next to a field a system reads every
frame. `network_settings` is exactly that: `enabled` gates `GatewayPlugin`
(`net/plugin.rs:97`) and `packet_dump` / `outbound_encryption` decide an
`init_resource` in `NetworkCorePlugin::build`, while `item_use_enabled` is read
from `Res<ClientConfig>` in the cast system (`hud/underbar/cast.rs:140`) and
`gateway_address` is read per connect attempt in `init_gateway_service`. A
single verdict is wrong about half of that group whichever way it goes, so the
row is `Liveness::Mixed` and its note names the `restart:` half and the `live:`
half by field — enforced by `every_mixed_group_names_a_restart_half_and_a_live_half`.

The other two questions #730 asked, answered by measurement rather than by
re-reading the note:

- **`dev_tools` does not have a live half.** All three reads are registration
  decisions (`main.rs:187`, `dev/mod.rs:120`, `environment/mod.rs:550-563`);
  `dev_hotkeys_enabled` *looks* like a run condition but is a plain `fn` called
  from `DevPlugin::build`. `dev_tools_is_read_only_at_registration` fails the
  build the day one of those reads becomes a `run_if`, which is when the row
  would need splitting.
- **`fonts` is deliberately not live.** Every spawned `TextFont` holds its own
  cloned `Handle<Font>`, so following a change means walking every text entity
  and re-resolving its handle — mechanical, expensive, and for a boot-time knob
  nobody toggles mid-session. That is a decision with a cost attached, not an
  impossibility, and the row now says so.

## An expensive apply is allowed to diff first

`resource_changed` fires for *any* edit to the resource. Where applying is
cheap (recomputing a colour palette) that is the whole story. Where it is not —
`map::foliage::apply_foliage_settings` has to rescatter every grass block when
the mode or density changes — the system keeps a `Local` copy of just the
fields that force the expensive path, so a chat-colour edit does not rebuild
the world's grass. The gate stays `resource_changed`; the diff is an
optimisation inside the apply system, not a second mechanism.

## What "restart" is allowed to mean

Only that the value decides something that cannot exist twice in one process —
today, plugin registration. It is *not* an excuse for "nobody wired it up". A
`Restart` row carries its reason precisely so the options UI can tell the user
which of the two it is, instead of a control that silently does nothing.
