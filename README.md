<h1 align="center">kubilaiswf's Minimap</h1>

<p align="center">A minimap for Elden Ring that looks like it belongs in the game.</p>

<p align="center">
  <a href="https://github.com/kubilaiswf/kubilaiswf-minimap/releases/latest"><img src="https://img.shields.io/github/v/release/kubilaiswf/kubilaiswf-minimap?label=release" alt="Latest release"></a>
  <a href="https://github.com/kubilaiswf/kubilaiswf-minimap/releases"><img src="https://img.shields.io/github/downloads/kubilaiswf/kubilaiswf-minimap/total" alt="Downloads"></a>
  <a href="https://www.nexusmods.com/eldenring/mods/11319"><img src="https://img.shields.io/badge/Nexus%20Mods-kubilaiswf's%20Minimap-da8e35?logo=nexusmods&logoColor=white" alt="Nexus Mods"></a>
  <a href="https://github.com/kubilaiswf/kubilaiswf-minimap/actions/workflows/release.yml"><img src="https://img.shields.io/github/actions/workflow/status/kubilaiswf/kubilaiswf-minimap/release.yml?branch=main" alt="Build"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="License"></a>
</p>

<p align="center"><img src="assets/screenshots/caelid.jpg" alt="The minimap in the top right corner, in Caelid" width="880"></p>

<p align="center">
  <b><a href="https://www.nexusmods.com/eldenring/mods/11319">Nexus Mods</a></b> ·
  <b><a href="https://github.com/kubilaiswf/kubilaiswf-minimap/releases/latest">Download</a></b> ·
  <a href="https://github.com/kubilaiswf/kubilaiswf-minimap/issues">Bugs and ideas</a>
</p>

It draws the game's own world map in the top right corner, turns with your camera and shows graces, beacons and map markers around you. It stays out of the way in menus, loading screens and places without a map.

<p align="center"><img src="assets/screenshots/closeups.png" alt="Close-ups: Map for Goblins DLL icons in Limgrave, beacons and the trail in Caelid" width="760"></p>

## Features

- **The game's own world map**, read from your install while the game runs. Nothing from the game is shipped.
- **Fully revealed**, in the overworld, inside legacy dungeons, in the underground and in Shadow of the Erdtree. Or, with `full_map = false`, only what your map fragments reveal, like the world map. DLC maps use the tiles from your own install and need no extra map files.
- **Turns with the camera** or stays north up, with a smooth arrow and three zoom levels (40, 80, 160 m).
- **Graces and beacons.** Numbered like in the game, beacons out of range point the way from the rim.
- **[Map for Goblins](https://www.nexusmods.com/eldenring/mods/3091) and [Map for Goblins DLL](https://www.nexusmods.com/eldenring/mods/10062).** Their markers show up and disappear once looted. With the DLL edition you get its icons, and hiding a category in its menu hides it here too. From its version 2.1.6 the minimap reads both through its map API.
- **Co-op partners**, Seamless Co-op included, as blue arrows. Out of range they point the way from the rim.
- **A trail** of where you've been.
- **Out of the way.** Hides in menus, on the world map, on loading screens, in Roundtable Hold and in caves and catacombs. Optionally during boss fights.

## Install

You need [me3](https://github.com/garyttierney/me3) and Elden Ring **1.17.2**. After a game update the mod can't read the game until it's updated too, so instead of risking a crash it shows an empty frame saying it needs an update for 20 seconds, then stays out of the way.

1. Download the zip from [Nexus](https://www.nexusmods.com/eldenring/mods/11319) or [Releases](https://github.com/kubilaiswf/kubilaiswf-minimap/releases/latest) and extract the `kubilaiswf-minimap` folder anywhere.
2. Double click `kubilaiswf-minimap.me3`.

> **Offline only.** Like every me3 mod, don't take it online.

<details>
<summary><b>Using it together with other me3 mods</b></summary>

Add the DLL to the other mod's `.me3` profile, for example Map for Goblins DLL next to the minimap:

```toml
profileVersion = "v1"

[[supports]]
game = "eldenring"

[[natives]]
path = 'MapForGoblins.dll'

[[natives]]
path = 'C:\path\to\kubilaiswf-minimap\kubilaiswf-minimap.dll'
```

If the profile has an empty `natives = []` line, delete it, me3 can't read a profile that has both. If that line lists other DLLs, keep it and add this one inside the brackets instead:

```toml
natives = [ { path = 'C:\some\other\mod.dll' }, { path = 'C:\path\to\kubilaiswf-minimap\kubilaiswf-minimap.dll' } ]
```

</details>

## Controls and settings

| Key | |
| --- | --- |
| **F7** | Show / hide |
| **F8** | Zoom: 40 / 80 / 160 m |

Everything else is in `minimap.ini` next to the DLL, created on first launch. Restart the game after changing it.

| Setting | Default | |
| --- | --- | --- |
| `trail` | `true` | Draw the path you walked |
| `rotate` | `true` | Turn the map with the camera. `false` keeps north up |
| `compass` | `true` | N / E / S / W letters |
| `full_map` | `true` | `false` shows only what your map fragments reveal |
| `coop_players` | `true` | Show your co-op partners, Seamless Co-op included |
| `hide_in_boss_fights` | `false` | Hide while a boss health bar is on screen |
| `hide_in_caves` | `true` | Hide in catacombs, caves and tunnels |
| `size` | `1.0` | 0.5 to 2.5 |
| `margin` | `24` | Distance from the top right corner, in pixels at 1080p |
| `toggle_key` | `F7` | F1-F12, A-Z, 0-9, Insert, Delete, Home, End, PageUp, PageDown, Keypad0-Keypad9, or `none` |
| `zoom_key` | `F8` | Same as above |

Updating from before 0.4.0? The keys used to be F9 and F10, which clash with Map for Goblins DLL. Delete your `minimap.ini` once to get the new defaults and settings.

<details>
<summary><b>Troubleshooting</b></summary>

**The minimap doesn't show up.** Check that `minimap.ini` was created next to the DLL. If it wasn't, the DLL never loaded: check the path in your `.me3` profile. If it was, check the game version, the mod only runs on 1.17.2.

**A key does nothing, or does two things.** Another mod uses the same key. Change `toggle_key` or `zoom_key` in `minimap.ini`.

**The map is blank in some places.** Shadow of the Erdtree areas aren't supported yet, and overhauls that move areas around can put you somewhere the world map doesn't cover.

</details>

## Known limitations

- No Shadow of the Erdtree map yet, DLC areas get a plain compass.
- Markers of the original Map for Goblins are simple shapes, not the game's icons.
- Tested with me3 and Elden Mod Loader. For Elden Mod Loader, put `kubilaiswf-minimap.dll` straight into its `mods` folder.

<details>
<summary><b>How it works</b></summary>

The DLL hooks the game's DirectX 12 renderer through [hudhook](https://github.com/veeenu/hudhook) and reads the game's state on a game task, using the structures from [fromsoftware-rs](https://github.com/vswarte/fromsoftware-rs).

The map comes from the menu's world map tiles. A background thread decrypts the archive headers, reads the tiles around you from the archives, decompresses them with the game's own Oodle DLL and decodes the textures. Positions inside legacy dungeons go through the game's `WorldMapLegacyConvParam`, like the world map does. Markers are the game's `WorldMapPointParam` rows, shown by the same rules the world map uses, which is also how map mods' markers appear.

</details>

<details>
<summary><b>Building</b></summary>

```
cargo build --release
me3 launch --profile minimap.me3
```

`minimap.me3` loads `target/release/kubilaiswf_minimap.dll`.

Releases are built by GitHub Actions: pushing a `v*` tag builds the zip and publishes it with the tag's message as release notes.

</details>

## Credits

- [fromsoftware-rs](https://github.com/vswarte/fromsoftware-rs) for the game structures (MIT / Apache 2.0, NON-AI variants)
- [hudhook](https://github.com/veeenu/hudhook) for the overlay
- [soarqin/EROverlay](https://github.com/soarqin/EROverlay) for documenting the beacon and map point logic
- [Map For Goblins - DLL](https://github.com/VirusAlex/ERR-MapForGoblins-DLL) by VirusAlex for its marker icons (MIT, see `package/THIRD-PARTY-NOTICES.txt`), and special thanks for adding a map API so other mods can show its markers
- [SoulsFormats](https://github.com/soulsmods/SoulsFormatsNEXT), [UXM](https://github.com/Nordgaren/UXM-Selective-Unpack) and [Smithbox](https://github.com/vawser/Smithbox) for file format and map research

## License

MIT, see [LICENSE](LICENSE). Use it however you like, just keep the credit.
