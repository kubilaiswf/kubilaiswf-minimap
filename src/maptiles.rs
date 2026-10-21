//! Streams the game's own world map textures around the player.
//!
//! The menu map is a 41x41 grid of 256px tiles (`MENU_MapTile_Mxx_L0_XX_YY_flags`) at one pixel
//! per meter, with Y growing northwards. Every grid cell has a variant per combination of map
//! fragments; the game's `.mtmsk` lists which fragments touch each cell, and the variant named
//! after the fragments the player has, within that mask, is the cell as the world map shows it.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::io;
use std::path::Path;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};

use crate::archive;
use crate::formats::{BinderEntry, Oodle, first_tpf_texture, read_binder};
use crate::reader::{Reader, invalid};

pub const TILE: usize = 256;
pub const WINDOW_TILES: usize = 3;
pub const WINDOW: usize = TILE * WINDOW_TILES;
/// Shown for cells with no texture, matching the map's unexplored parchment.
const EMPTY: [u8; 4] = [40, 34, 26, 255];

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MapId {
    Overworld,
    Underground,
    ShadowRealm,
}

impl MapId {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "M00" => Some(Self::Overworld),
            "M01" => Some(Self::Underground),
            "M10" => Some(Self::ShadowRealm),
            _ => None,
        }
    }

    /// Map number stored in the game's beacon slots.
    pub fn menu_id(self) -> u8 {
        match self {
            Self::Overworld => 0,
            Self::Underground => 1,
            Self::ShadowRealm => 10,
        }
    }

    /// Map for Goblins' API uses layer 2 for the DLC, rather than its M10 map number.
    pub fn mfg_layer(self) -> u32 {
        match self {
            Self::Overworld => 0,
            Self::Underground => 1,
            Self::ShadowRealm => 2,
        }
    }
}

/// Converts an overworld-aligned position (meters, see `game.rs`) to menu map pixels, Y up.
/// Map cell (i, j) shows tile m60/m61_{i+28}_{j+24}_00, centers aligned.
pub fn map_pixel([x, z]: [f32; 2]) -> [f32; 2] {
    const HALF: f32 = TILE as f32 / 2.0;
    [x + HALF - 28.0 * TILE as f32, z + HALF - 24.0 * TILE as f32]
}

/// The inverse of `map_pixel` for the game's own world map coordinates, which run Y down
/// over the 41-cell map (checked against the player position the game keeps for its map).
pub fn from_game_map([x, y]: [f32; 2]) -> [f32; 2] {
    const HALF: f32 = TILE as f32 / 2.0;
    let y_up = 41.0 * TILE as f32 - y;
    [
        x - HALF + 28.0 * TILE as f32,
        y_up - HALF + 24.0 * TILE as f32,
    ]
}

/// 3x3 map cells stitched into one RGBA image, rows top (north) first.
pub struct Window {
    pub map: MapId,
    /// Grid cell of the window's south-west corner.
    pub origin: [i32; 2],
    pub rgba: Vec<u8>,
}

/// A window to load: the map, its south-west cell and the map fragments to show, one bit per
/// fragment as in the `.mtmsk` masks.
pub type Request = (MapId, [i32; 2], u32);

pub struct Loader {
    requests: Sender<Request>,
    // The render loop has to be Sync, which a bare Receiver isn't.
    windows: Mutex<Receiver<Window>>,
}

impl Loader {
    /// Starts reading the map archives on a background thread. If they can't be read the
    /// loader just never produces windows and the minimap stays a plain compass.
    pub fn spawn(game_dir: &Path) -> Self {
        let (requests, request_rx) = channel();
        let (window_tx, windows) = channel();
        let game_dir = game_dir.to_owned();
        std::thread::spawn(move || {
            let _ = serve(&game_dir, &request_rx, &window_tx);
        });
        Self {
            requests,
            windows: Mutex::new(windows),
        }
    }

    pub fn request(&self, request: Request) {
        let _ = self.requests.send(request);
    }

    pub fn poll(&self) -> Option<Window> {
        self.windows.lock().unwrap().try_iter().last()
    }
}

fn serve(
    game_dir: &Path,
    requests: &Receiver<Request>,
    windows: &Sender<Window>,
) -> io::Result<()> {
    let oodle = Oodle::load(&game_dir.join("oo2core_6_win64.dll"))?;
    let masks =
        read_masks(&oodle.decompress_dcx(
            &archive::open(game_dir, "/menu/71_maptile.mtmskbnd.dcx")?.read_all()?,
        )?)?;
    let tiles = index_tiles(read_binder(
        &archive::open(game_dir, "/menu/71_maptile.tpfbhd")?.read_all()?,
    )?);
    let mut bdt = archive::open(game_dir, "/menu/71_maptile.tpfbdt")?;
    let mut decoded: HashMap<(MapId, [i32; 2], u32), Vec<u8>> = HashMap::new();

    while let Ok(mut request) = requests.recv() {
        // Only the newest request matters once the player has moved on.
        while let Ok(newer) = requests.try_recv() {
            request = newer;
        }
        let (map, origin, fragments) = request;
        // Nine cells are needed at a time; dropping the rest bounds memory to a few MB.
        decoded.retain(|&(m, [x, y], _), _| {
            m == map
                && (origin[0] - 1..=origin[0] + 3).contains(&x)
                && (origin[1] - 1..=origin[1] + 3).contains(&y)
        });

        let mut rgba = vec![0u8; WINDOW * WINDOW * 4];
        for dy in 0..WINDOW_TILES {
            for dx in 0..WINDOW_TILES {
                let cell = [origin[0] + dx as i32, origin[1] + dy as i32];
                let variant = masks.get(&(map, cell)).copied().unwrap_or(0) & fragments;
                let key = (map, cell, variant);
                let tile = match decoded.entry(key) {
                    Entry::Occupied(tile) => tile.into_mut(),
                    Entry::Vacant(slot) => slot.insert(match tiles.get(&key) {
                        // One unreadable tile must not stop the map for the rest of the session.
                        Some(entry) => bdt
                            .read_at(entry.offset, entry.size)
                            .and_then(|dcx| decode_tile(&oodle, &dcx))
                            .unwrap_or_else(|_| EMPTY.repeat(TILE * TILE)),
                        None => EMPTY.repeat(TILE * TILE),
                    }),
                };
                // Cells stack northwards, image rows go southwards.
                let top = (WINDOW_TILES - 1 - dy) * TILE;
                for row in 0..TILE {
                    let at = ((top + row) * WINDOW + dx * TILE) * 4;
                    rgba[at..at + TILE * 4]
                        .copy_from_slice(&tile[row * TILE * 4..(row + 1) * TILE * 4]);
                }
            }
        }
        if windows.send(Window { map, origin, rgba }).is_err() {
            return Ok(());
        }
    }
    Ok(())
}

/// Reads `<MapTileMask id="..." mask="..."/>` entries, where id is level*10000 + x*100 + y.
fn read_masks(bnd: &[u8]) -> io::Result<HashMap<(MapId, [i32; 2]), u32>> {
    let mut masks = HashMap::new();
    for entry in read_binder(bnd)? {
        let Some(map) = entry
            .name
            .rsplit('\\')
            .next()
            .and_then(|f| f.get(13..16))
            .and_then(MapId::from_name)
        else {
            continue;
        };
        let xml =
            String::from_utf8_lossy(Reader(bnd).bytes(entry.offset, entry.size)?).into_owned();
        for line in xml.lines() {
            let (Some(id), Some(mask)) = (attribute(line, "id"), attribute(line, "mask")) else {
                continue;
            };
            if id < 10000 {
                masks.insert((map, [(id / 100) as i32, (id % 100) as i32]), mask);
            }
        }
    }
    Ok(masks)
}

fn attribute(line: &str, name: &str) -> Option<u32> {
    let start = line.find(&format!(" {name}=\""))? + name.len() + 3;
    line[start..].split('"').next()?.parse().ok()
}

/// Every L0 cell variant, by map, cell and the fragments it shows.
fn index_tiles(entries: Vec<BinderEntry>) -> HashMap<(MapId, [i32; 2], u32), BinderEntry> {
    let mut tiles = HashMap::new();
    for entry in entries {
        // e.g. 71_MapTile\MENU_MapTile_M00_L0_12_31_00000100.tpf.dcx
        let Some(stem) = entry
            .name
            .rsplit('\\')
            .next()
            .and_then(|f| f.strip_suffix(".tpf.dcx"))
        else {
            continue;
        };
        let parts: Vec<&str> = stem.split('_').collect();
        let [_, _, map, "L0", x, y, flags] = parts[..] else {
            continue;
        };
        let (Some(map), Ok(x), Ok(y), Ok(flags)) = (
            MapId::from_name(map),
            x.parse(),
            y.parse(),
            u32::from_str_radix(flags, 16),
        ) else {
            continue;
        };
        tiles.insert((map, [x, y], flags), entry);
    }
    tiles
}

fn decode_tile(oodle: &Oodle, dcx: &[u8]) -> io::Result<Vec<u8>> {
    let tpf = oodle.decompress_dcx(dcx)?;
    let dds = Reader(first_tpf_texture(&tpf)?);
    let (height, width) = (dds.u32(0xC)? as usize, dds.u32(0x10)? as usize);
    if (width, height) != (TILE, TILE) {
        return Err(invalid("unexpected map tile size"));
    }
    let mut pixels = vec![0u32; TILE * TILE];
    let result = match (dds.bytes(0x54, 4)?, dds.u32(0x80)?) {
        // DXGI_FORMAT_BC7_UNORM(_SRGB)
        (b"DX10", 98 | 99) => texture2ddecoder::decode_bc7(&dds.0[0x94..], TILE, TILE, &mut pixels),
        (b"DXT1", _) => texture2ddecoder::decode_bc1(&dds.0[0x80..], TILE, TILE, &mut pixels),
        _ => return Err(invalid("unsupported map tile format")),
    };
    result.map_err(invalid)?;
    // The decoder packs BGRA; textures are uploaded as RGBA.
    Ok(pixels
        .iter()
        .flat_map(|p| {
            let [b, g, r, a] = p.to_le_bytes();
            [r, g, b, a]
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn dlc_atlas_is_separate_and_uses_the_same_meter_scale() {
        assert_eq!(MapId::from_name("M10"), Some(MapId::ShadowRealm));
        assert_eq!(MapId::ShadowRealm.menu_id(), 10);
        assert_eq!(MapId::ShadowRealm.mfg_layer(), 2);
        assert_eq!(
            (MapId::Overworld.menu_id(), MapId::Overworld.mfg_layer()),
            (0, 0)
        );
        assert_eq!(
            (MapId::Underground.menu_id(), MapId::Underground.mfg_layer()),
            (1, 1)
        );
        // Gravesite Plain's world-map reference, with the same 1 px/m scale as M00.
        let world = [46.0 * 256.0 + 61.56, 40.0 * 256.0 + 34.086];
        let pixel = map_pixel(world);
        assert!((pixel[0] - 4797.56).abs() < 0.002);
        assert!((pixel[1] - 4258.086).abs() < 0.002);
        let recovered = from_game_map([pixel[0], 41.0 * TILE as f32 - pixel[1]]);
        assert!((0..2).all(|axis| (recovered[axis] - world[axis]).abs() < 0.002));
        let entries = ["M00", "M01", "M10"]
            .into_iter()
            .map(|map| BinderEntry {
                name: format!("71_MapTile\\MENU_MapTile_{map}_L0_18_16_00000001.tpf.dcx"),
                offset: 0,
                size: 0,
            })
            .collect();
        let tiles = index_tiles(entries);
        assert_eq!(tiles.len(), 3);
        for map in [MapId::Overworld, MapId::Underground, MapId::ShadowRealm] {
            assert!(tiles.contains_key(&(map, [18, 16], 1)));
        }
    }

    #[test]
    #[ignore = "Requires MINIMAP_WORLD_TEST_ROOT pointing to a local game install"]
    fn real_shadow_realm_tiles_preserve_fragment_variants_and_world_switches() {
        let root = std::env::var_os("MINIMAP_WORLD_TEST_ROOT").expect("Set local game root");
        let root = Path::new(&root);
        let oodle = Oodle::load(&root.join("oo2core_6_win64.dll")).unwrap();
        let masks = read_masks(
            &oodle
                .decompress_dcx(
                    &archive::open(root, "/menu/71_maptile.mtmskbnd.dcx")
                        .unwrap()
                        .read_all()
                        .unwrap(),
                )
                .unwrap(),
        )
        .unwrap();
        let tiles = index_tiles(
            read_binder(
                &archive::open(root, "/menu/71_maptile.tpfbhd")
                    .unwrap()
                    .read_all()
                    .unwrap(),
            )
            .unwrap(),
        );
        let mut bdt = archive::open(root, "/menu/71_maptile.tpfbdt").unwrap();
        let mut decoded = 0;
        for (&(map, cell, variant), entry) in &tiles {
            if map == MapId::ShadowRealm && variant == masks.get(&(map, cell)).copied().unwrap_or(0)
            {
                assert_eq!(
                    decode_tile(&oodle, &bdt.read_at(entry.offset, entry.size).unwrap())
                        .unwrap()
                        .len(),
                    TILE * TILE * 4
                );
                decoded += 1;
            }
        }
        assert!(decoded > 500, "DLC tiles missing: {decoded}");
        let loader = Loader::spawn(root);
        let origin = [17, 15];
        for (map, fragments) in [
            (MapId::ShadowRealm, 0),
            (MapId::ShadowRealm, u32::MAX),
            (MapId::Overworld, u32::MAX),
            (MapId::ShadowRealm, u32::MAX),
        ] {
            loader.request((map, origin, fragments));
            let window = loader
                .windows
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(30))
                .unwrap();
            assert_eq!((window.map, window.origin), (map, origin));
            for dy in 0..WINDOW_TILES {
                for dx in 0..WINDOW_TILES {
                    let cell = [origin[0] + dx as i32, origin[1] + dy as i32];
                    let variant = masks[&(map, cell)] & fragments;
                    let entry = &tiles[&(map, cell, variant)];
                    let expected =
                        decode_tile(&oodle, &bdt.read_at(entry.offset, entry.size).unwrap())
                            .unwrap();
                    for row in 0..TILE {
                        let at = (((WINDOW_TILES - 1 - dy) * TILE + row) * WINDOW + dx * TILE) * 4;
                        assert_eq!(
                            &window.rgba[at..at + TILE * 4],
                            &expected[row * TILE * 4..(row + 1) * TILE * 4]
                        );
                    }
                }
            }
        }
        println!(
            "Decoded {decoded} M10 cells; fragment variants and DLC/base/DLC windows match game tiles."
        );
    }
}
