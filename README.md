# Absoloot Overlay

This is an in-game overlay for ELDEN RING with the ability to track the following data:

1. Bosses Killed
2. Total Bosses being tracked
3. Number of player deaths
4. Number of Messmer's Kindling Shards in player's inventory
  - This is compatible with Matt's Randomizer (it has custom logic that splits Kindling into shards required burn the sealing tree)
5. Number of Great Runes player has acquired

---

## Victory conditions

Victory tracking is disabled by default:

```toml
[victory]
mode = "None"
```

Complete after every boss in the selected checklist:

```toml
[victory]
mode = "Checklist"
```

Complete after every supplied event flag is active:

```toml
[victory]
mode = "BossIds"
boss_ids = [10000800, 19000800]
```

Complete after one supplied event flag is active:

```toml
[victory]
mode = "OneBoss"
boss_id = 19000800
```

`BossIds` is one AND condition. Explicit IDs do not need to exist in the checklist and do not affect checklist kill or total counts. IDs must be non-negative, zero is allowed, and duplicate `BossIds` entries are normalized. Completion freezes IGT and adds `GOAL COMPLETE` above the configured overlay text. Completion remains latched until the overlay restarts.

The former `[timer].freeze_on_boss_flag` setting has been removed. Configurations that still contain it are rejected; replace it with one of the `[victory]` modes above.

## Absoloot Chaos competition telemetry

Competition telemetry is optional. A player downloads their personal
`competition_config.toml` from the Season 2 website and places it beside
`absoloot_overlay.dll`. Do not copy `competition_config.example.toml` over it: the
downloaded file contains the player's assigned seed, allowed event flags, and a
secret assignment token.

The local file does not need a teammate ID. Supabase links players through their
team and selected seed. The overlay reports configured event flags, IGT,
completed check event flags, shard inventory, and deaths. Supabase deduplicates
both checks and shard locations across the team. When a seed has an optional
unique-shard requirement, the configured win flag does not freeze a player's
IGT until the team has also reached that shard total.

When competition configuration is present, compact mode uses a fixed scorecard
inspired by the Ignite Nightreign overlay. It shows the seed name followed by
player and team columns for IGT, checks, shards, and deaths. The team shard value
is the distinct union of configured shard-location flags, not the sum of both
players' inventory quantities. The team check value is likewise the distinct
union of completed check IDs, so the same location completed by both players
counts once. Reaching an optional shard requirement reveals a green team-goal
message below the compact scorecard.

The overlay can display the server-confirmed values with these template
variables in `overlay_config.toml`:

- `{player_shards}`: unique configured shard locations found by this player
- `{team_shards}`: unique configured shard locations found by either teammate
- `{teammate_igt}`: the teammate's latest IGT, or their frozen finish IGT
- `{combined_igt}`: the sum of both frozen finish IGTs

If the competition file is missing, the normal standalone overlay continues to
work. If Supabase cannot be reached, gameplay and the overlay continue; remote
values remain at their last confirmed state.

### Credits
- [Sully-](https://github.com/Sully-): for showing me hudhook, sharing code examples, and helping me with debugging the overlay
- [hudhook](https://github.com/veeenu/hudhook): a rust crate for creating in-game UI overlays, made by Andrea
- [fromsoftware-rs](https://github.com/vswarte/fromsoftware-rs): a collection of rust crates for interacting with elden ring specifically, made by Vswarte

## Building a release

Release builds require Windows, Rust with the `x86_64-pc-windows-msvc` target, and the Microsoft Visual C++ build tools used by that target. Run the script from PowerShell 5.1 or later at the repository root.

The lockfile currently resolves the `fromsoftware-rs` Git dependencies to
`16d266146bcc7e896ac0d9a2c020e0d4c07baf52`, which includes ELDEN RING
1.17.1 support. Update and validate the lockfile before claiming support for a
newer game version.

Build and package the overlay into the default `output/` folder:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-release.ps1
```

Choose a different output folder with `-OutputPath`:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-release.ps1 -OutputPath .\artifacts\absoloot_overlay
```

Add `-Zip` to create a distributable archive as well:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-release.ps1 -Zip
```

The script runs `cargo build --locked --release --target x86_64-pc-windows-msvc`. It copies every file and directory under `dist/` into the output folder, then adds the compiled `absoloot_overlay.dll`. The required packaged layout includes `overlay_config.toml`, the localized `data/` tree, and `absoloot_overlay.dll`.

With `-Zip`, the script creates `absoloot_overlay-<version>-windows-x86_64.zip` beside the output folder. The version comes from the workspace package metadata. The ZIP contains the packaged files at its root, without an extra output-folder wrapper.

Packaging uses a temporary staging folder and validates required files before replacing the output. An absent or empty output folder is accepted. A nonempty output folder is replaced only when its adjacent `<output>.absoloot_overlay-release-owner` marker exactly identifies it as output previously created by this script. The marker is not included in the package. The script refuses output folders that are reparse points, including junctions and symbolic links, and refuses to replace nonempty unowned folders. It also refuses to publish a ZIP over a directory, reparse point, or other non-ordinary archive path, and validates a temporary ZIP before publishing it.

## Licensing

SPDX-License-Identifier: GPL-3.0-only
