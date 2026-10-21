use std::sync::{Mutex, OnceLock};

use eldenring::cs::{
    BlockId, BonfireWarpParam, CSCamera, CSEventFlagMan, CSFade, CSFeManHudState, CSFeManImp,
    CSMenuManImp, PlayerIns, SoloParamRepository, WorldChrMan, WorldMapLegacyConvParam,
    WorldMapPieceParam, WorldMapPointParam,
};
use fromsoftware_shared::FromStatic;
use hudhook::windows::Win32::System::LibraryLoader::GetModuleHandleW;
use hudhook::windows::core::w;

use crate::maptiles::{self, MapId};
use crate::mfg;

/// Copied out on a game task so the render thread never touches live game objects.
#[derive(Clone)]
pub struct Snapshot {
    /// Meters on the overworld grid where the world map can place the player, otherwise local
    /// to `space`.
    pub position: [f32; 2],
    /// Positions are only comparable within the same space.
    pub space: i32,
    /// The menu map showing `position`, if any.
    pub map: Option<MapId>,
    pub facing: [f32; 2],
    pub camera_right: [f32; 2],
    pub camera_forward: [f32; 2],
    /// Screen fade from 0 to 1; loading screens and warps sit behind a full one.
    pub fade: f32,
    /// False while a menu has the HUD hidden or changed.
    pub hud_visible: bool,
    /// A boss health bar is on screen.
    pub boss_fight: bool,
    /// Catacombs (m30), caves (m31) and tunnels (m32), which have no map of their own.
    pub in_cave: bool,
    pub graces: Vec<[f32; 2]>,
    pub beacons: Vec<Beacon>,
    /// Includes whatever map mods like Map for Goblins add.
    pub points: Vec<MapPoint>,
    /// Other players in the session, Seamless Co-op partners included, in the same space.
    pub coop: Vec<CoopPlayer>,
    /// Map fragments the player has, see `map_fragments`.
    pub fragments: u32,
}

#[derive(Clone, Copy)]
pub struct CoopPlayer {
    pub position: [f32; 2],
    pub facing: [f32; 2],
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PointKind {
    Landmark,
    Npc,
    Invasion,
    Boss,
    /// A grace shown before it's discovered.
    Grace,
    /// Anything else; map mods mostly add item pickups.
    Item,
}

#[derive(Clone, Copy)]
pub struct MapPoint {
    pub kind: PointKind,
    pub position: [f32; 2],
    /// Defeated bosses and the like, which the game marks as done.
    pub cleared: bool,
    pub mfg_icon: Option<MfgIcon>,
}

/// Icons of markers added by Map for Goblins' DLL edition.
#[derive(Clone, Copy)]
pub enum MfgIcon {
    /// A cell of `assets/mfg_icons.png`, for versions without the map API.
    Bundled(u16),
    /// A source icon id from the map API, see `mfg::icons`.
    Api(u32),
}

/// Icons in `assets/mfg_icons.png`, made from Map for Goblins DLL 2.1.5. The DLL appends them
/// to the world map's icon frames in its alphabetical category order and numbers its markers'
/// icons after the game's own frames, so the lowest number it uses ("ammo") is the first cell.
pub const MFG_ICON_COUNT: u16 = 70;

/// A beacon placed on the world map, numbered like the game numbers it.
#[derive(Clone, Copy)]
pub struct Beacon {
    pub number: u8,
    pub position: [f32; 2],
}

pub static LATEST: Mutex<Option<Snapshot>> = Mutex::new(None);

pub fn update() {
    *LATEST.lock().unwrap() = read();
}

fn read() -> Option<Snapshot> {
    // SAFETY: game tasks run in step with the game's own updates of these singletons.
    let world_chr_man = unsafe { WorldChrMan::instance() }.ok()?;
    let camera = unsafe { CSCamera::instance() }.ok()?;
    let player = world_chr_man.main_player.as_ref()?;
    let fade = unsafe { CSFade::instance() }.map_or(0.0, |fade| {
        fade.fade_plates
            .iter()
            .map(|plate| plate.current_color.a)
            .fold(0.0, f32::max)
    });
    let fe = unsafe { CSFeManImp::instance() }.ok();
    let hud_visible = fe.is_none_or(|fe| fe.hud_state == CSFeManHudState::Default);
    // The boss name entries outlive the bar (they stay set after dying), so ask the HUD instead.
    let boss_fight = fe.is_some_and(|fe| {
        fe.frontend_values
            .boss_list_tag_data
            .iter()
            .any(|tag| tag.is_visible)
    });

    // Havok space, which the camera and physics matrices use, shares axes and
    // scale with block space, so their direction vectors apply to block positions.
    let view = &camera.pers_cam_1.matrix;
    let model = &player.chr_ins.chr_ctrl.physics_model_matrix;
    let (position, space, map) = locate(
        player.current_block_id,
        [player.block_position.x, player.block_position.z],
    );
    Some(Snapshot {
        position,
        space,
        map,
        // Characters face down their model's -Z axis (verified in game).
        facing: [-model.2.0, -model.2.2],
        camera_right: [view.0.0, view.0.2],
        camera_forward: [view.2.0, view.2.2],
        fade,
        hud_visible,
        boss_fight,
        in_cave: (30..=32).contains(&player.current_block_id.area()),
        graces: discovered_graces(position, space),
        beacons: map.map_or_else(Vec::new, placed_beacons),
        points: map.map_or_else(Vec::new, |map| visible_points(map, position, space)),
        coop: coop_players(world_chr_man, player, position, space),
        fragments: map_fragments(map),
    })
}

/// The map fragments the player has, one bit per fragment of the map they're on, as the map
/// tile masks number them: bit b is WorldMapPieceParam row `base + b`, base 0 for the
/// overworld, 100 for the underground and 1000 for the Shadow Realm.
/// Everything counts as found until the params and flags are loaded.
fn map_fragments(map: Option<MapId>) -> u32 {
    let base = match map {
        Some(MapId::Overworld) => 0,
        Some(MapId::Underground) => 100,
        Some(MapId::ShadowRealm) => 1000,
        None => return u32::MAX,
    };
    // SAFETY: game tasks run in step with the game's own updates of these singletons.
    let (Ok(params), Ok(flags)) = (unsafe { SoloParamRepository::instance() }, unsafe {
        CSEventFlagMan::instance()
    }) else {
        return u32::MAX;
    };
    (0..32)
        .filter(|&bit| {
            params
                .get::<WorldMapPieceParam>(base + bit)
                .map(|row| row.open_event_flag_id())
                .is_some_and(|flag| flag != 0 && flags.virtual_memory_flag.get_flag(flag))
        })
        .fold(0, |bits, bit| bits | 1 << bit)
}
/// The game leaves other players' block positions unset, but their physics positions are live
/// and share Havok space with ours, so they are placed by their offset from the main player.
fn coop_players(
    world_chr_man: &WorldChrMan,
    player: &PlayerIns,
    position: [f32; 2],
    space: i32,
) -> Vec<CoopPlayer> {
    let origin = &player.chr_ins.chr_ctrl.physics_model_matrix.3;
    world_chr_man
        .player_chr_set
        .characters()
        .filter(|other| !std::ptr::eq(&**other, player))
        .filter(|other| locate(other.chr_ins.block_id, [0.0, 0.0]).1 == space)
        .map(|other| {
            let model = &other.chr_ins.chr_ctrl.physics_model_matrix;
            CoopPlayer {
                position: [
                    position[0] + model.3.0 - origin.0,
                    position[1] + model.3.2 - origin.2,
                ],
                facing: [-model.2.0, -model.2.2],
            }
        })
        .collect()
}

/// Converts a block-local position to the minimap's coordinates: (position, space, map).
fn locate(block: BlockId, local: [f32; 2]) -> ([f32; 2], i32, Option<MapId>) {
    if block.is_overworld() {
        // Overworld blocks are mAA_XX_ZZ_LL grid tiles of (256 << LL) meters with positions
        // relative to the tile center. Larger tiles are centered on their 2x2 or 4x4 group of
        // small tiles, hence the extra half-tile offsets.
        // Only the units digit is the grid scale; the tens digit is an event variant.
        let tile = (256u32 << (block.index() % 10).min(2)) as f32;
        let center = (tile - 256.0) / 2.0;
        let position = [
            local[0] + f32::from(block.block()) * tile + center,
            local[1] + f32::from(block.region()) * tile + center,
        ];
        let map = match block.area() {
            60 => Some(MapId::Overworld),
            61 => Some(MapId::ShadowRealm),
            _ => None,
        };
        return (position, i32::from(block.area()), map);
    }
    // Roundtable Hold has a world map spot but isn't anywhere on the map itself.
    let roundtable_hold = (block.area(), block.block()) == (11, 10);
    let conversion = legacy_conversion(block, local).filter(|_| !roundtable_hold);
    let Some((position, dst_area)) = conversion else {
        return (local, i32::from(block), None);
    };
    // Underground areas share the overworld's coordinates but have their own map.
    let (space, map) = match (dst_area, block.area()) {
        (60, 12) => (12, Some(MapId::Underground)),
        (60, _) => (60, Some(MapId::Overworld)),
        (61, _) => (61, Some(MapId::ShadowRealm)),
        (area, _) => (i32::from(area), None),
    };
    (position, space, map)
}

/// One row of WorldMapLegacyConvParam: a point in a dungeon and the same point either on the
/// world map grid or in another dungeon.
struct Conversion {
    src: (u8, u8, u8),
    src_pos: [f32; 2],
    dst: (u8, u8, u8),
    dst_pos: [f32; 2],
}

/// The overworld and the Shadow of the Erdtree world, the areas the world map is drawn for.
const WORLD_AREAS: [u8; 2] = [60, 61];

static CONVERSIONS: OnceLock<Vec<Conversion>> = OnceLock::new();

/// Returns the world map aligned position and its area. Dungeons list several reference
/// points, each valid around itself, so the one nearest the player is used. Rows that lead
/// to another dungeon (Stormveil to the Divine Tower and the like) are only followed when no
/// row leads to the world map, as for the Shunning-Grounds, which go through Leyndell.
fn legacy_conversion(block: BlockId, local: [f32; 2]) -> Option<([f32; 2], u8)> {
    let conversions = conversions()?;
    let (mut src, mut at) = ((block.area(), block.block(), block.region()), local);
    for _ in 0..4 {
        let nearest = |to_world: bool| {
            conversions
                .iter()
                .filter(|c| c.src == src && WORLD_AREAS.contains(&c.dst.0) == to_world)
                .min_by(|a, b| {
                    distance_squared(a.src_pos, at).total_cmp(&distance_squared(b.src_pos, at))
                })
        };
        if let Some(c) = nearest(true) {
            let position = [
                at[0] - c.src_pos[0] + f32::from(c.dst.1) * 256.0 + c.dst_pos[0],
                at[1] - c.src_pos[1] + f32::from(c.dst.2) * 256.0 + c.dst_pos[1],
            ];
            return Some((position, c.dst.0));
        }
        let c = nearest(false)?;
        at = [
            at[0] - c.src_pos[0] + c.dst_pos[0],
            at[1] - c.src_pos[1] + c.dst_pos[1],
        ];
        src = c.dst;
    }
    None
}

fn conversions() -> Option<&'static [Conversion]> {
    if let Some(conversions) = CONVERSIONS.get() {
        return Some(conversions);
    }
    // SAFETY: the param repository is only read here, on a game task.
    let params = unsafe { SoloParamRepository::instance() }.ok()?;
    let conversions: Vec<Conversion> = (0..)
        .map_while(|i| params.get_row_by_index::<WorldMapLegacyConvParam>(i))
        .map(|row| Conversion {
            src: (row.src_area_no(), row.src_grid_x_no(), row.src_grid_z_no()),
            src_pos: [row.src_pos_x(), row.src_pos_z()],
            dst: (row.dst_area_no(), row.dst_grid_x_no(), row.dst_grid_z_no()),
            dst_pos: [row.dst_pos_x(), row.dst_pos_z()],
        })
        .collect();
    // The params load after the player can already exist; try again next frame.
    if conversions.is_empty() {
        return None;
    }
    Some(CONVERSIONS.get_or_init(|| conversions))
}

fn distance_squared(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
}

/// Graces further than this aren't drawn even at the widest zoom.
const GRACE_RANGE: f32 = 400.0;

struct Grace {
    event_flag: u32,
    position: [f32; 2],
    space: i32,
}

static GRACES: OnceLock<Vec<Grace>> = OnceLock::new();

fn discovered_graces(position: [f32; 2], space: i32) -> Vec<[f32; 2]> {
    let (Some(graces), Ok(flags)) = (graces(), unsafe { CSEventFlagMan::instance() }) else {
        return Vec::new();
    };
    graces
        .iter()
        .filter(|g| {
            g.space == space && distance_squared(g.position, position) < GRACE_RANGE * GRACE_RANGE
        })
        .filter(|g| flags.virtual_memory_flag.get_flag(g.event_flag))
        .map(|g| g.position)
        .collect()
}

fn graces() -> Option<&'static [Grace]> {
    if let Some(graces) = GRACES.get() {
        return Some(graces);
    }
    conversions()?;
    // SAFETY: the param repository is only read here, on a game task.
    let params = unsafe { SoloParamRepository::instance() }.ok()?;
    let graces: Vec<Grace> = (0..)
        .map_while(|i| params.get_row_by_index::<BonfireWarpParam>(i))
        .filter(|row| row.eventflag_id() != 0)
        .map(|row| {
            let block = BlockId::from_parts(row.area_no(), row.grid_x_no(), row.grid_z_no(), 0);
            let (position, space, _) = locate(block, [row.pos_x(), row.pos_z()]);
            Grace {
                event_flag: row.eventflag_id(),
                position,
                space,
            }
        })
        .collect();
    Some(GRACES.get_or_init(|| graces))
}

// Beacon storage in game version 1.17.1, from soarqin's EROverlay notes:
// CSMenuMan -> owner -> map view -> WorldMapMarkerDataList (+0x300) -> CSMenuMarkersSaveData (+0x38).
const MENU_MAP_OWNER: usize = 0x80;
const OWNER_MAP_VIEW: usize = 0x250;
const VIEW_MARKER_SAVE: usize = 0x300 + 0x38;
const SAVE_SLOTS: usize = 0x08;
const SAVE_CAPACITY: usize = 0x10;
/// The save has 10 slots; the game only allows 5 beacons at a time.
const MAX_SLOTS: usize = 10;

#[repr(C)]
#[derive(Clone, Copy)]
struct MarkerSlot {
    /// Negative for an empty slot.
    id: i32,
    /// Game world map coordinates, see `maptiles::from_game_map`.
    x: f32,
    y: f32,
    /// 0 for the overworld, 1 for the underground, 10 for the Shadow Realm.
    map: u8,
    icon: u8,
    _pad: [u8; 2],
}

fn placed_beacons(map: MapId) -> Vec<Beacon> {
    let wanted_map = map.menu_id();
    // SAFETY: read on a game task, so the menu objects aren't being torn down meanwhile;
    // every pointer in the chain is checked before it's followed.
    unsafe {
        let Ok(menu) = CSMenuManImp::instance_ptr() else {
            return Vec::new();
        };
        let Some(save) = follow(
            menu as usize,
            &[MENU_MAP_OWNER, OWNER_MAP_VIEW, VIEW_MARKER_SAVE],
        ) else {
            return Vec::new();
        };
        let Some(slots) = follow(save, &[SAVE_SLOTS]) else {
            return Vec::new();
        };
        let capacity = ((save + SAVE_CAPACITY) as *const u32).read_unaligned() as usize;
        (0..capacity.min(MAX_SLOTS))
            .filter_map(|i| {
                let slot =
                    ((slots + i * size_of::<MarkerSlot>()) as *const MarkerSlot).read_unaligned();
                (slot.id >= 0 && slot.map == wanted_map && slot.x.is_finite() && slot.y.is_finite())
                    .then(|| Beacon {
                        // The game shows the slot number, not the id.
                        number: i as u8 + 1,
                        position: maptiles::from_game_map([slot.x, slot.y]),
                    })
            })
            .collect()
    }
}

/// Reads a pointer at each offset in turn, stopping at null.
unsafe fn follow(mut address: usize, offsets: &[usize]) -> Option<usize> {
    for offset in offsets {
        // SAFETY: the caller guarantees `address` points at a live object with a pointer here.
        address = unsafe { ((address + offset) as *const usize).read_unaligned() };
        if address == 0 {
            return None;
        }
    }
    Some(address)
}

/// Points further than this aren't drawn even at the widest zoom.
const POINT_RANGE: f32 = 300.0;

/// One WorldMapPointParam row, reduced to what deciding its visibility needs.
struct PointRow {
    position: [f32; 2],
    space: i32,
    on_overworld: bool,
    on_underground: bool,
    on_shadow_realm: bool,
    icon: u16,
    alt_icon: u16,
    open_flag: u32,
    cleared_flag: u32,
    /// Rows allowed without text skip the text condition.
    needs_text: bool,
    texts: [PointText; 8],
    /// No map selected, which only Map for Goblins' DLL edition does.
    unmasked: bool,
    mfg_icon: Option<u16>,
}

#[derive(Clone, Copy)]
struct PointText {
    id: i32,
    enable: [u32; 2],
    disable: [u32; 2],
    kind: u8,
}

static POINTS: OnceLock<Vec<PointRow>> = OnceLock::new();

/// Same rule as the game (per soarqin's EROverlay notes): the open flag must be set and an
/// ordinary point needs an active text line. Map mods hide looted items by disabling the text.
fn visible_points(map: MapId, position: [f32; 2], space: i32) -> Vec<MapPoint> {
    let (Some(rows), Ok(flags)) = (points(), unsafe { CSEventFlagMan::instance() }) else {
        return Vec::new();
    };
    let flags = &flags.virtual_memory_flag;
    let on = |flag: u32| flag != 0 && flags.get_flag(flag);
    rows.iter()
        .filter(|row| match map {
            MapId::Overworld => row.on_overworld,
            MapId::Underground => row.on_underground,
            MapId::ShadowRealm => row.on_shadow_realm,
        })
        .filter(|row| {
            row.space == space
                && distance_squared(row.position, position) < POINT_RANGE * POINT_RANGE
        })
        .filter(|row| row.open_flag == 0 || on(row.open_flag))
        // The map API reports these itself, with Map for Goblins' own visibility.
        .filter(|row| !(row.unmasked && mfg::available()))
        .filter_map(|row| {
            let active = |text: &&PointText| {
                text.id > 0
                    && text.enable.iter().all(|&flag| flag == 0 || on(flag))
                    && !text.disable.iter().any(|&flag| on(flag))
            };
            let mut active_texts = row.texts.iter().filter(active).peekable();
            if row.needs_text && active_texts.peek().is_none() {
                return None;
            }
            let icon = if active_texts.any(|text| text.kind == 1) && row.alt_icon != 0 {
                row.alt_icon
            } else {
                row.icon
            };
            Some(MapPoint {
                kind: point_kind(icon)?,
                position: row.position,
                cleared: row.cleared_flag != u32::MAX && on(row.cleared_flag),
                mfg_icon: row.mfg_icon.map(MfgIcon::Bundled),
            })
        })
        .chain(mfg_api_points(map, position, space))
        .collect()
}

/// Map for Goblins' markers as its map API reports them. Thousands of them go through
/// `locate`, so the list is refreshed a few times a second rather than every frame.
fn mfg_api_points(map: MapId, position: [f32; 2], space: i32) -> Vec<MapPoint> {
    struct Cache {
        calls: u32,
        map: Option<MapId>,
        /// Position, space and source icon id.
        points: Vec<([f32; 2], i32, u32)>,
    }
    static CACHE: Mutex<Cache> = Mutex::new(Cache {
        calls: 0,
        map: None,
        points: Vec::new(),
    });
    if !mfg::available() {
        return Vec::new();
    }
    let mut cache = CACHE.lock().unwrap();
    if cache.map != Some(map) || cache.calls.is_multiple_of(10) {
        let layer = map.mfg_layer();
        cache.points.clear();
        mfg::visit_points(layer, |point| {
            let [area, x, z, _] = point.raw_map_id.to_be_bytes();
            let (at, space, _) = locate(BlockId::from_parts(area, x, z, 0), [point.x, point.z]);
            cache.points.push((at, space, point.icon_id));
        });
        cache.map = Some(map);
    }
    cache.calls = cache.calls.wrapping_add(1);
    cache
        .points
        .iter()
        .filter(|(at, at_space, _)| {
            *at_space == space && distance_squared(*at, position) < POINT_RANGE * POINT_RANGE
        })
        .map(|&(at, _, icon)| MapPoint {
            kind: PointKind::Item,
            position: at,
            cleared: false,
            mfg_icon: Some(MfgIcon::Api(icon)),
        })
        .collect()
}
/// Groups the game's map icon ids (names from Smithbox's WORLD_MAP_ICON_ID enum).
fn point_kind(icon: u16) -> Option<PointKind> {
    match icon {
        0 => None,
        1 | 2 => Some(PointKind::Grace),
        // Light beams guiding toward graces and the like, not places.
        83..=85 => None,
        80 | 86 => Some(PointKind::Npc),
        87 | 88 => Some(PointKind::Invasion),
        3..=99 => Some(PointKind::Landmark),
        499..=502 => Some(PointKind::Boss),
        _ => Some(PointKind::Item),
    }
}

fn points() -> Option<&'static [PointRow]> {
    if let Some(points) = POINTS.get() {
        return Some(points);
    }
    conversions()?;
    // SAFETY: the param repository is only read here, on a game task.
    let params = unsafe { SoloParamRepository::instance() }.ok()?;
    // SAFETY: only looks the module up by name.
    let mfg_dll = unsafe { GetModuleHandleW(w!("MapForGoblins.dll")) }.is_ok();
    let mut points: Vec<PointRow> = (0..)
        .map_while(|i| params.get_row_by_index::<WorldMapPointParam>(i))
        // Area labels name whole regions rather than places.
        .filter(|row| !row.is_area_icon())
        .map(|row| {
            let block = BlockId::from_parts(row.area_no(), row.grid_x_no(), row.grid_z_no(), 0);
            let (position, space, located) = locate(block, [row.pos_x(), row.pos_z()]);
            // Map for Goblins' DLL edition clears the map selection of its rows and draws them
            // itself, on the map their location belongs to.
            let unmasked =
                mfg_dll && !row.disp_mask00() && !row.disp_mask01() && !row.disp_mask02();
            // The second set of text flags is signed; non-positive means unused.
            let flag2 = |flag: i32| u32::try_from(flag).unwrap_or(0);
            let texts = [
                (
                    row.text_id1(),
                    row.text_enable_flag_id1(),
                    row.text_disable_flag_id1(),
                    row.text_enable_flag2_id1(),
                    row.text_disable_flag2_id1(),
                    row.text_type1(),
                ),
                (
                    row.text_id2(),
                    row.text_enable_flag_id2(),
                    row.text_disable_flag_id2(),
                    row.text_enable_flag2_id2(),
                    row.text_disable_flag2_id2(),
                    row.text_type2(),
                ),
                (
                    row.text_id3(),
                    row.text_enable_flag_id3(),
                    row.text_disable_flag_id3(),
                    row.text_enable_flag2_id3(),
                    row.text_disable_flag2_id3(),
                    row.text_type3(),
                ),
                (
                    row.text_id4(),
                    row.text_enable_flag_id4(),
                    row.text_disable_flag_id4(),
                    row.text_enable_flag2_id4(),
                    row.text_disable_flag2_id4(),
                    row.text_type4(),
                ),
                (
                    row.text_id5(),
                    row.text_enable_flag_id5(),
                    row.text_disable_flag_id5(),
                    row.text_enable_flag2_id5(),
                    row.text_disable_flag2_id5(),
                    row.text_type5(),
                ),
                (
                    row.text_id6(),
                    row.text_enable_flag_id6(),
                    row.text_disable_flag_id6(),
                    row.text_enable_flag2_id6(),
                    row.text_disable_flag2_id6(),
                    row.text_type6(),
                ),
                (
                    row.text_id7(),
                    row.text_enable_flag_id7(),
                    row.text_disable_flag_id7(),
                    row.text_enable_flag2_id7(),
                    row.text_disable_flag2_id7(),
                    row.text_type7(),
                ),
                (
                    row.text_id8(),
                    row.text_enable_flag_id8(),
                    row.text_disable_flag_id8(),
                    row.text_enable_flag2_id8(),
                    row.text_disable_flag2_id8(),
                    row.text_type8(),
                ),
            ]
            .map(|(id, enable, disable, enable2, disable2, kind)| PointText {
                id,
                enable: [enable, flag2(enable2)],
                disable: [disable, flag2(disable2)],
                kind,
            });
            PointRow {
                position,
                space,
                on_overworld: row.disp_mask00() || unmasked && located == Some(MapId::Overworld),
                on_underground: row.disp_mask01()
                    || unmasked && located == Some(MapId::Underground),
                on_shadow_realm: row.disp_mask02()
                    || unmasked && located == Some(MapId::ShadowRealm),
                icon: row.icon_id(),
                alt_icon: row.alt_icon_id(),
                open_flag: row.event_flag_id(),
                cleared_flag: row.cleared_event_flag_id(),
                needs_text: !row.is_enable_no_text(),
                texts,
                unmasked,
                mfg_icon: None,
            }
        })
        .collect();
    let is_mfg = |row: &PointRow| row.unmasked && row.icon >= 100;
    if let Some(first) = points
        .iter()
        .filter(|row| is_mfg(row))
        .map(|row| row.icon)
        .min()
    {
        for row in points.iter_mut().filter(|row| is_mfg(row)) {
            row.mfg_icon = Some(row.icon - first).filter(|&cell| cell < MFG_ICON_COUNT);
        }
    }
    Some(POINTS.get_or_init(|| points))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dlc_and_base_world_keep_the_same_grid_scale_across_event_variants() {
        for (area, map) in [(60, MapId::Overworld), (61, MapId::ShadowRealm)] {
            for lod in 0..=2 {
                let tile = (256u32 << lod) as f32;
                let expected = [
                    12.0 + 18.0 * tile + (tile - 256.0) / 2.0,
                    -9.0 + 16.0 * tile + (tile - 256.0) / 2.0,
                ];
                for variant in [0, 10] {
                    assert_eq!(
                        locate(
                            BlockId::from_parts(area, 18, 16, variant + lod),
                            [12.0, -9.0]
                        ),
                        (expected, i32::from(area), Some(map))
                    );
                }
            }
        }
    }
}
