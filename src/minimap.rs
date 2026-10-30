use std::collections::{HashMap, VecDeque};
use std::f32::consts::{PI, TAU};
use std::path::{Path, PathBuf};

use hudhook::imgui::{Context, DrawListMut, FontSource, ImColor32, TextureId, Ui};
use hudhook::{ImguiRenderLoop, RenderContext};

use crate::config::Config;
use crate::{dungeon, dungeon_loader};
#[cfg(test)]
#[path="dungeon_render_tests.rs"]
mod dungeon_render_tests;
use crate::game::{self, MfgIcon, PointKind, Snapshot};
use crate::maptiles::{self, Loader, MapId, TILE, WINDOW};

/// Meters from the center to the rim at each zoom level.
const ZOOM_RANGES: [f32; 3] = [40.0, 80.0, 160.0];
const TRAIL_SPACING: f32 = 1.5;
const TRAIL_LENGTH: usize = 800;
const TELEPORT_DISTANCE: f32 = 150.0;
const FONT_SIZE: f32 = 13.0;
/// Radius of the map disc at 1080p.
const RADIUS: f32 = 96.0;
/// Map pixels (meters) the window must extend past the player: the widest zoom plus slack.
const WINDOW_MARGIN: f32 = ZOOM_RANGES[2] + 20.0;
/// Segments of the textured map disc.
const DISC_SEGMENTS: usize = 72;

const FRAME_PNG: &[u8] = include_bytes!("../assets/frame.png");
const ARROW_PNG: &[u8] = include_bytes!("../assets/arrow.png");
const BEACON_PNG: &[u8] = include_bytes!("../assets/marker.png");
/// Map for Goblins' icons, 48px cells, 9 per row (see `game::MFG_ICON_COUNT`).
const MFG_ICONS_PNG: &[u8] = include_bytes!("../assets/mfg_icons.png");
const MFG_ICON_COLUMNS: u16 = 9;
const MFG_ICON_ROWS: u16 = game::MFG_ICON_COUNT.div_ceil(MFG_ICON_COLUMNS);
/// Map radius as a fraction of frame.png's width: just past the widest point of its hole
/// (measured 0.387 to 0.406) so the map always reaches under the frame.
const FRAME_HOLE: f32 = 0.408;
/// Point of arrow.png it turns around: the middle of the arrowhead, not of the canvas.
const ARROW_PIVOT: [f32; 2] = [0.5, 603.0 / 1254.0];

const PALE_GOLD: [u8; 3] = [214, 196, 150];
const BRIGHT_GOLD: [u8; 3] = [255, 222, 150];
const BACKGROUND: [u8; 3] = [24, 20, 15];
const BLACK: [u8; 3] = [0, 0, 0];

pub struct Minimap {
    config: Config,
    dungeon_loader: Option<dungeon_loader::DungeonLoader>,
    dungeon_key:Option<[u8;4]>,
    dungeon:Option<dungeon_loader::Loaded>,
    dungeon_floor:dungeon::FloorTracker,
    dungeon_view:dungeon::View,
    /// False on game versions the mod can't read, see `lib.rs`.
    supported: bool,
    /// Seconds the update notice has been on screen.
    notice_time: f32,
    loader: Loader,
    frame: Option<TextureId>,
    arrow: Option<TextureId>,
    beacon: Option<TextureId>,
    mfg_icons: Option<TextureId>,
    mfg_api_icons: Option<IconAtlas>,
    map_texture: Option<TextureId>,
    /// What `map_texture` currently shows.
    map_window: Option<(MapId, [i32; 2])>,
    requested_window: Option<maptiles::Request>,
    font_display_height: f32,
    last: Option<Snapshot>,
    camera_forward: [f32; 2],
    camera_right: [f32; 2],
    trail: VecDeque<[f32; 2]>,
    trail_space: i32,
    visible: bool,
    zoom: usize,
    alpha: f32,
    /// Displayed facing as a world angle, where 0 is north (+Z).
    facing_yaw: Option<f32>,
}

impl Minimap {
    pub fn with_dungeon_loader(mut self, loader:Option<dungeon_loader::DungeonLoader>)->Self {
        self.dungeon_loader=loader;self
    }
    fn dungeon_active(&self,player:&Snapshot)->bool {
        self.config.dungeon_mode && player.local_position.iter().all(|v|v.is_finite())
            && self.dungeon.as_ref().is_some_and(|d|d.block==player.block_id && d.map.is_ok())
    }
    fn select_dungeon(&mut self,player:&Snapshot,dt:f32) {
        let wanted=(self.config.dungeon_mode && player.in_cave).then_some(player.block_id);
        if wanted!=self.dungeon_key {
            self.dungeon_key=wanted;self.dungeon=None;self.dungeon_floor.reset();
            if let (Some(key),Some(loader))=(wanted,&self.dungeon_loader) {loader.request(key);}
        }
        if let Some(loaded)=self.dungeon_loader.as_ref().and_then(|l|l.poll()) {
            self.accept_dungeon(loaded);
        }
        if let Some(map)=self.dungeon.as_ref().and_then(|d|d.map.as_ref().ok()).filter(|_|player.local_position.iter().all(|v|v.is_finite())) {
            self.dungeon_floor.update(map,player.local_position,dt);
        }
    }
    fn accept_dungeon(&mut self,loaded:dungeon_loader::Loaded) {
        if Some(loaded.block)==self.dungeon_key {self.dungeon=Some(loaded);}
    }
    fn draw_dungeon(&self,draw:&DrawListMut,player:&Snapshot,center:[f32;2],radius:f32) {
        let Some(map)=self.dungeon.as_ref().and_then(|d|d.map.as_ref().ok()) else {return;};
        let p=player.local_position;let floor=self.dungeon_floor.height().unwrap_or(p[1]);
        let project=|v:dungeon::Point|self.to_screen([(v[0]-p[0])*radius/dungeon::ZOOM[self.zoom],(v[2]-p[2])*radius/dungeon::ZOOM[self.zoom]]);
        let bands: &[(f32,f32,[u8;3],f32)]=match self.dungeon_view {
            dungeon::View::Layered=>&[(-8.,-2.,[110,100,80],0.22),(2.,8.,[130,120,95],0.18),(-2.,2.,PALE_GOLD,0.8)],
            dungeon::View::Current=>&[(-2.,2.,PALE_GOLD,0.8)],
            dungeon::View::All=>&[(-10000.,10000.,PALE_GOLD,0.45)],
        };
        for &(low,high,color,alpha) in bands {
            for tri in &map.triangles {
                if !dungeon::overlaps_view(tri.points,p,dungeon::ZOOM[self.zoom],floor+low,floor+high) {continue;}
                let clipped=dungeon::clip_height(&tri.points,floor+low,floor+high);
                let screen:Vec<_>=clipped.into_iter().map(project).collect();
                let clipped=dungeon::clip_disc(&screen,radius);
                if clipped.len()<3 {continue;}
                let points=clipped.into_iter().map(|p|[center[0]+p[0],center[1]+p[1]]).collect();
                draw.add_polyline(points,self.color(color,alpha)).filled(true).build();
            }
        }
        for line in &map.boundaries {
            if let Some(line)=dungeon::clip_segment_height(*line,floor-2.,floor+2.) {
                if let Some([a,b])=dungeon::clip_segment_disc(line.map(project),radius) {
                    draw.add_line([center[0]+a[0],center[1]+a[1]],[center[0]+b[0],center[1]+b[1]],self.color(BRIGHT_GOLD,0.9)).thickness(1.).build();
                }
            }
        }
    }
    pub fn new(game_dir: &Path, config: Config, supported: bool) -> Self {
        Self {
            config,
            dungeon_loader:None,dungeon_key:None,dungeon:None,
            dungeon_floor:dungeon::FloorTracker::default(),dungeon_view:dungeon::View::Layered,
            supported,
            notice_time: 0.0,
            loader: Loader::spawn(game_dir),
            frame: None,
            arrow: None,
            beacon: None,
            mfg_icons: None,
            mfg_api_icons: None,
            map_texture: None,
            map_window: None,
            requested_window: None,
            font_display_height: 1080.0,
            last: None,
            camera_forward: [0.0, 1.0],
            camera_right: [1.0, 0.0],
            trail: VecDeque::new(),
            trail_space: 0,
            visible: true,
            zoom: 1,
            alpha: 0.0,
            facing_yaw: None,
        }
    }
}

impl ImguiRenderLoop for Minimap {
    fn initialize<'a>(&'a mut self, ctx: &mut Context, render_context: &'a mut dyn RenderContext) {
        ctx.set_ini_filename(None);
        self.font_display_height = ctx.io().display_size[1];
        // The first font added becomes the default one.
        let path = PathBuf::from(std::env::var_os("WINDIR").unwrap_or_default())
            .join(r"Fonts\georgiab.ttf");
        if let Ok(data) = std::fs::read(path) {
            ctx.fonts().add_font(&[FontSource::TtfData {
                data: &data,
                size_pixels: FONT_SIZE * self.font_display_height / 1080.0,
                config: None,
            }]);
        }
        self.frame = load_png(render_context, FRAME_PNG);
        self.arrow = load_png(render_context, ARROW_PNG);
        self.beacon = load_png(render_context, BEACON_PNG);
        self.mfg_icons = load_png(render_context, MFG_ICONS_PNG);
        self.mfg_api_icons = IconAtlas::load(render_context);
    }

    fn before_render<'a>(
        &'a mut self,
        ctx: &mut Context,
        render_context: &'a mut dyn RenderContext,
    ) {
        // The font is baked once, so follow later resolution changes by scaling it.
        let height = ctx.io().display_size[1];
        ctx.io_mut().font_global_scale = height / self.font_display_height * self.config.size;

        // hudhook uploads on the game's queue and waits for the GPU. On loading screens that
        // queue can be waiting on the game, which deadlocks it.
        if !self.last.as_ref().is_some_and(in_control) {
            return;
        }
        let Some(window) = self.loader.poll() else {
            return;
        };
        let size = WINDOW as u32;
        let uploaded = match self.map_texture {
            Some(texture) => render_context
                .replace_texture(texture, &window.rgba, size, size)
                .is_ok(),
            None => match render_context.load_texture(&window.rgba, size, size) {
                Ok(texture) => {
                    self.map_texture = Some(texture);
                    true
                }
                Err(_) => false,
            },
        };
        if uploaded {
            self.map_window = Some((window.map, window.origin));
        } else {
            // Ask again next frame instead of waiting until the player nears the window's edge.
            self.requested_window = None;
        }
    }

    fn render(&mut self, ui: &mut Ui) {
        let pressed = |key: Option<_>| key.is_some_and(|key| ui.is_key_pressed_no_repeat(key));
        if pressed(self.config.dungeon_floor_key) { self.dungeon_view=self.dungeon_view.next(); }
        if pressed(self.config.toggle_key) {
            self.visible = !self.visible;
        }
        if pressed(self.config.zoom_key) {
            self.zoom = (self.zoom + 1) % ZOOM_RANGES.len();
        }
        if !self.supported {
            // Shown for a while after launch, then gone for good.
            const NOTICE_SECONDS: f32 = 20.0;
            self.notice_time += ui.io().delta_time;
            if self.visible && self.notice_time < NOTICE_SECONDS {
                self.alpha = (NOTICE_SECONDS - self.notice_time).min(1.0);
                self.draw_unsupported(ui);
            }
            return;
        }

        let snapshot = game::LATEST.lock().unwrap().clone();
        if let Some(snapshot) = &snapshot {
            self.select_dungeon(snapshot,ui.io().delta_time);
            self.update(snapshot);
        }
        // Characters turn instantly, so the arrow eases toward the facing in world space,
        // which keeps it from lagging behind camera turns.
        if let Some([x, z]) = snapshot.as_ref().and_then(|s| normalize(s.facing)) {
            let target = x.atan2(z);
            let current = self.facing_yaw.get_or_insert(target);
            *current += wrap_angle(target - *current) * (ui.io().delta_time * 14.0).min(1.0);
        }
        // Menus like the pause menu and world map draw where the minimap sits.
        let target = match &snapshot {
            Some(snapshot)
                if self.visible
                    && in_control(snapshot)
                    && !(self.config.hide_in_boss_fights && snapshot.boss_fight)
                    && !(self.config.hide_in_caves && snapshot.in_cave && !self.dungeon_active(snapshot)) =>
            {
                1.0
            }
            _ => 0.0,
        };
        self.alpha += (target - self.alpha) * (ui.io().delta_time * 12.0).min(1.0);
        // The last known state keeps drawing while the map fades out.
        if self.alpha < 0.01 {
            return;
        }
        if let Some(last) = &self.last {
            self.draw(ui, last);
        }
    }
}

impl Minimap {
    fn update(&mut self, snapshot: &Snapshot) {
        self.last = Some(snapshot.clone());
        // Without rotation the basis stays at its north-up default.
        if self.config.rotate {
            if let Some(forward) = normalize(snapshot.camera_forward) {
                self.camera_forward = forward;
            }
            if let Some(right) = normalize(snapshot.camera_right) {
                self.camera_right = right;
            }
        }

        // hudhook leaks the staging buffer of every upload, so only re-center the window near
        // its edge instead of on every cell crossing.
        if let Some(map) = snapshot.map {
            let pixel = maptiles::map_pixel(snapshot.position);
            let fragments = if self.config.full_map {
                u32::MAX
            } else {
                snapshot.fragments
            };
            // A newly found fragment redraws the window in place.
            let reload = match self.requested_window {
                Some((requested_map, origin, shown)) if requested_map == map => {
                    shown != fragments
                        || (0..2).any(|axis| {
                            let offset = pixel[axis] - (origin[axis] * TILE as i32) as f32;
                            !(WINDOW_MARGIN..WINDOW as f32 - WINDOW_MARGIN).contains(&offset)
                        })
                }
                _ => true,
            };
            if reload {
                let cell = pixel.map(|p| (p / TILE as f32).floor() as i32);
                let wanted = (map, [cell[0] - 1, cell[1] - 1], fragments);
                self.loader.request(wanted);
                self.requested_window = Some(wanted);
            }
        }

        if snapshot.space != self.trail_space {
            self.trail.clear();
            self.trail_space = snapshot.space;
        }
        if let Some(&last) = self.trail.back() {
            let distance = length([
                snapshot.position[0] - last[0],
                snapshot.position[1] - last[1],
            ]);
            if distance > TELEPORT_DISTANCE {
                self.trail.clear();
            } else if distance < TRAIL_SPACING {
                return;
            }
        }
        self.trail.push_back(snapshot.position);
        if self.trail.len() > TRAIL_LENGTH {
            self.trail.pop_front();
        }
    }

    /// Turns a horizontal world direction into a screen direction. The map turns with
    /// the camera: whatever the camera faces is up.
    fn to_screen(&self, [dx, dz]: [f32; 2]) -> [f32; 2] {
        let [rx, rz] = self.camera_right;
        let [fx, fz] = self.camera_forward;
        [dx * rx + dz * rz, -(dx * fx + dz * fz)]
    }

    /// The inverse of `to_screen`, since camera right and forward are orthonormal on the ground.
    fn to_world(&self, [sx, sy]: [f32; 2]) -> [f32; 2] {
        let [rx, rz] = self.camera_right;
        let [fx, fz] = self.camera_forward;
        [sx * rx - sy * fx, sx * rz - sy * fz]
    }

    /// Screen direction of a compass bearing, where 0 is north (+Z) and 90 is east (+X).
    fn bearing(&self, degrees: f32) -> [f32; 2] {
        let (sin, cos) = degrees.to_radians().sin_cos();
        self.to_screen([sin, cos])
    }

    fn color(&self, [r, g, b]: [u8; 3], opacity: f32) -> ImColor32 {
        ImColor32::from_rgba(
            r,
            g,
            b,
            ((opacity * self.alpha).clamp(0.0, 1.0) * 255.0) as u8,
        )
    }

    /// Disc center, map radius, frame size and scale on the current screen.
    fn layout(&self, ui: &Ui) -> ([f32; 2], f32, f32, f32) {
        let [width, height] = ui.io().display_size;
        let screen_scale = height / 1080.0;
        let s = screen_scale * self.config.size;
        let radius = RADIUS * s;
        let frame_size = radius / FRAME_HOLE;
        let margin = self.config.margin * screen_scale;
        let center = [width - margin - frame_size / 2.0, margin + frame_size / 2.0];
        (center, radius, frame_size, s)
    }

    /// An empty frame, so players can tell the mod loaded but needs an update.
    fn draw_unsupported(&self, ui: &Ui) {
        let (center, radius, frame_size, s) = self.layout(ui);
        let draw_list = ui.get_foreground_draw_list();
        draw_list
            .add_circle(center, radius, self.color(BACKGROUND, 0.9))
            .filled(true)
            .num_segments(96)
            .build();
        let blink = 0.55 + 0.45 * (self.notice_time * 4.0).cos();
        for (line, dy) in [("needs an", -8.0), ("update", 8.0)] {
            let at = [center[0], center[1] + dy * s];
            self.draw_text_centered(ui, &draw_list, at, self.color(PALE_GOLD, blink), line);
        }
        self.draw_frame(&draw_list, center, frame_size);

    }

    fn draw_frame(&self, draw_list: &DrawListMut, center: [f32; 2], frame_size: f32) {
        if let Some(frame) = self.frame {
            let half = frame_size / 2.0;
            draw_list
                .add_image(
                    frame,
                    [center[0] - half, center[1] - half],
                    [center[0] + half, center[1] + half],
                )
                .col(self.color([255, 255, 255], 1.0))
                .build();
        }
    }

    fn draw(&self, ui: &Ui, player: &Snapshot) {
        let (center, radius, frame_size, s) = self.layout(ui);
        let draw_list = ui.get_foreground_draw_list();

        draw_list
            .add_circle(center, frame_size * 0.47, self.color(BLACK, 0.35))
            .filled(true)
            .num_segments(96)
            .build();
        let local=self.dungeon_active(player);
        if local || !self.draw_map(&draw_list, player, center, radius) {
            draw_list
                .add_circle(center, radius, self.color(BACKGROUND, 0.9))
                .filled(true)
                .num_segments(96)
                .build();
        }
        if local { self.draw_dungeon(&draw_list,player,center,radius); }
        // Darken toward the rim so the frame sits on the map instead of cutting it.
        for i in 0..4 {
            draw_list
                .add_circle(
                    center,
                    radius - (6.0 + i as f32 * 3.0) * s,
                    self.color(BLACK, 0.22 - i as f32 * 0.05),
                )
                .num_segments(96)
                .thickness(4.0 * s)
                .build();
        }

        if !local {
        if self.config.trail {
            self.draw_trail(&draw_list, player, center, radius, s);
        }
        self.draw_points(&draw_list, player, center, radius, s);
        self.draw_graces(&draw_list, player, center, radius, s);
        self.draw_beacons(ui, &draw_list, player, center, radius, s);
        if self.config.coop_players {
            self.draw_coop(&draw_list, player, center, radius, s);
        }

        }
        let letters = if self.config.compass {
            ["N", "E", "S", "W"].as_slice()
        } else {
            &[]
        };
        for (i, letter) in letters.iter().enumerate() {
            let direction = self.bearing(i as f32 * 90.0);
            let position = [
                center[0] + direction[0] * (radius - 14.0 * s),
                center[1] + direction[1] * (radius - 14.0 * s),
            ];
            draw_list
                .add_circle(position, 9.0 * s, self.color(BACKGROUND, 0.75))
                .filled(true)
                .build();
            let color = if i == 0 {
                self.color(BRIGHT_GOLD, 1.0)
            } else {
                self.color(PALE_GOLD, 0.9)
            };
            self.draw_text_centered(ui, &draw_list, position, color, letter);
        }

        self.draw_arrow(&draw_list, center, s);
        self.draw_frame(&draw_list, center, frame_size);
        if local {
            if let Some(dungeon)=&self.dungeon {
                let mode=match self.dungeon_view {dungeon::View::Layered=>"Layered",dungeon::View::Current=>"Current floor",dungeon::View::All=>"All floors"};
                self.draw_text_centered(ui,&draw_list,[center[0],center[1]+frame_size*0.48+10.*s],self.color(PALE_GOLD,0.9),&format!("{} | {mode}",dungeon.name));
            }
        }
    }

    /// A fan of wedges clips the texture to a circle and gives each rim vertex its own
    /// rotated UV. Returns false when there's no map for this spot.
    fn draw_map(
        &self,
        draw_list: &DrawListMut,
        player: &Snapshot,
        center: [f32; 2],
        radius: f32,
    ) -> bool {
        let (Some(texture), Some((map, origin)), Some(player_map)) =
            (self.map_texture, self.map_window, player.map)
        else {
            return false;
        };
        if map != player_map {
            return false;
        }
        let meters_per_pixel = ZOOM_RANGES[self.zoom] / radius;
        let player_pixel = maptiles::map_pixel(player.position);
        let uv = |screen_offset: [f32; 2]| {
            let world = self.to_world(screen_offset.map(|d| d * meters_per_pixel));
            let x = player_pixel[0] + world[0] - (origin[0] * TILE as i32) as f32;
            let y = player_pixel[1] + world[1] - (origin[1] * TILE as i32) as f32;
            [x / WINDOW as f32, 1.0 - y / WINDOW as f32]
        };
        let center_uv = uv([0.0, 0.0]);
        // A window from somewhere else would draw the wrong place until the new one arrives.
        if !center_uv.iter().all(|c| (0.0..=1.0).contains(c)) {
            return false;
        }
        let tint = self.color([236, 226, 210], 1.0);
        for i in 0..DISC_SEGMENTS {
            let rim = |k: usize| {
                let (sin, cos) = (k as f32 / DISC_SEGMENTS as f32 * TAU).sin_cos();
                [cos * radius, sin * radius]
            };
            let (a, b) = (rim(i), rim(i + 1));
            let (pa, pb) = (
                [center[0] + a[0], center[1] + a[1]],
                [center[0] + b[0], center[1] + b[1]],
            );
            draw_list
                .add_image_quad(texture, center, pa, pb, pb)
                .uv(center_uv, uv(a), uv(b), uv(b))
                .col(tint)
                .build();
        }
        true
    }

    fn draw_trail(
        &self,
        draw_list: &DrawListMut,
        player: &Snapshot,
        center: [f32; 2],
        radius: f32,
        s: f32,
    ) {
        let pixels_per_meter = radius / ZOOM_RANGES[self.zoom];
        let limit = radius - 6.0 * s;
        let project = |point: [f32; 2]| {
            let d = self.to_screen([point[0] - player.position[0], point[1] - player.position[1]]);
            [
                center[0] + d[0] * pixels_per_meter,
                center[1] + d[1] * pixels_per_meter,
            ]
        };
        let inside = |p: [f32; 2]| length([p[0] - center[0], p[1] - center[1]]) < limit;

        let points: Vec<[f32; 2]> = self
            .trail
            .iter()
            .map(|&p| project(p))
            .chain([center])
            .collect();
        for (i, segment) in points.windows(2).enumerate() {
            if !inside(segment[0]) || !inside(segment[1]) {
                continue;
            }
            let age = (i + 1) as f32 / (points.len() - 1) as f32;
            draw_list
                .add_line(
                    segment[0],
                    segment[1],
                    self.color([255, 236, 190], 0.10 + 0.55 * age),
                )
                .thickness(2.0 * s)
                .build();
        }
    }

    /// Dimmed once the game marks them done.
    fn draw_points(
        &self,
        draw_list: &DrawListMut,
        player: &Snapshot,
        center: [f32; 2],
        radius: f32,
        s: f32,
    ) {
        let pixels_per_meter = radius / ZOOM_RANGES[self.zoom];
        for point in &player.points {
            let d = self.to_screen([
                point.position[0] - player.position[0],
                point.position[1] - player.position[1],
            ]);
            if length(d) * pixels_per_meter > radius - 8.0 * s {
                continue;
            }
            let at = [
                center[0] + d[0] * pixels_per_meter,
                center[1] + d[1] * pixels_per_meter,
            ];
            let opacity = if point.cleared { 0.45 } else { 1.0 };
            let icon = match point.mfg_icon {
                Some(MfgIcon::Bundled(cell)) => self.mfg_icons.map(|texture| {
                    let (column, row) = (cell % MFG_ICON_COLUMNS, cell / MFG_ICON_COLUMNS);
                    let uv = |column: u16, row: u16| {
                        [
                            f32::from(column) / f32::from(MFG_ICON_COLUMNS),
                            f32::from(row) / f32::from(MFG_ICON_ROWS),
                        ]
                    };
                    (
                        texture,
                        uv(column, row),
                        uv(column + 1, row + 1),
                        [7.0, 7.0],
                    )
                }),
                Some(MfgIcon::Api(id)) => self.mfg_api_icons.as_ref().and_then(|atlas| {
                    let cell = atlas.cells.get(&id)?;
                    Some((atlas.texture, cell.uv_min, cell.uv_max, cell.half))
                }),
                None => None,
            };
            if let Some((texture, uv_min, uv_max, half)) = icon {
                let half = [half[0] * s, half[1] * s];
                draw_list
                    .add_image(
                        texture,
                        [at[0] - half[0], at[1] - half[1]],
                        [at[0] + half[0], at[1] + half[1]],
                    )
                    .uv_min(uv_min)
                    .uv_max(uv_max)
                    .col(self.color([255, 255, 255], opacity))
                    .build();
                continue;
            }
            let outline = self.color(BLACK, 0.8 * opacity);
            let dot = |r: f32, rgb: [u8; 3]| {
                draw_list
                    .add_circle(at, (r + 1.2) * s, outline)
                    .filled(true)
                    .build();
                draw_list
                    .add_circle(at, r * s, self.color(rgb, opacity))
                    .filled(true)
                    .build();
            };
            match point.kind {
                PointKind::Item => dot(2.4, [236, 190, 96]),
                PointKind::Npc => dot(3.0, [150, 196, 240]),
                PointKind::Invasion => dot(3.0, [214, 64, 52]),
                PointKind::Landmark => {
                    let r = 4.5 * s;
                    let diamond = vec![
                        [at[0], at[1] - r],
                        [at[0] + r, at[1]],
                        [at[0], at[1] + r],
                        [at[0] - r, at[1]],
                    ];
                    draw_list
                        .add_polyline(diamond.clone(), self.color([214, 190, 140], 0.85 * opacity))
                        .filled(true)
                        .build();
                    draw_list
                        .add_polyline([diamond.as_slice(), &diamond[..1]].concat(), outline)
                        .thickness(1.2 * s)
                        .build();
                }
                PointKind::Boss => {
                    let rgb = if point.cleared {
                        [150, 150, 150]
                    } else {
                        [200, 40, 36]
                    };
                    dot(4.0, [30, 10, 8]);
                    draw_list
                        .add_circle(at, 4.0 * s, self.color(rgb, opacity))
                        .thickness(1.8 * s)
                        .build();
                    draw_list
                        .add_circle(at, 1.6 * s, self.color(rgb, opacity))
                        .filled(true)
                        .build();
                }
                PointKind::Grace => {
                    draw_list
                        .add_circle(at, 4.5 * s, self.color([255, 222, 150], 0.8 * opacity))
                        .thickness(1.4 * s)
                        .build();
                }
            }
        }
    }

    /// Beacons out of range sit on the rim, pointing the way.
    fn draw_beacons(
        &self,
        ui: &Ui,
        draw_list: &DrawListMut,
        player: &Snapshot,
        center: [f32; 2],
        radius: f32,
        s: f32,
    ) {
        let Some(texture) = self.beacon else {
            return;
        };
        let pixels_per_meter = radius / ZOOM_RANGES[self.zoom];
        let rim = radius - 18.0 * s;
        for beacon in &player.beacons {
            let d = self.to_screen([
                beacon.position[0] - player.position[0],
                beacon.position[1] - player.position[1],
            ]);
            let mut offset = [d[0] * pixels_per_meter, d[1] * pixels_per_meter];
            let distance = length(offset);
            let inside = distance <= rim;
            if !inside {
                offset = [offset[0] / distance * rim, offset[1] / distance * rim];
            }
            let at = [center[0] + offset[0], center[1] + offset[1]];
            let half = if inside { 13.0 } else { 10.0 } * s;
            let opacity = if inside { 1.0 } else { 0.8 };
            draw_list
                .add_image(
                    texture,
                    [at[0] - half, at[1] - half],
                    [at[0] + half, at[1] + half],
                )
                .col(self.color([255, 255, 255], opacity))
                .build();
            // Too small for a digit inside, so it goes beside it like on the game's compass.
            let number = beacon.number.to_string();
            let label = [at[0] + half * 0.85, at[1] + half * 0.35];
            self.draw_text_centered(
                ui,
                draw_list,
                label,
                self.color([255, 244, 220], opacity),
                &number,
            );
        }
    }

    /// Pale blue arrows facing where the partner faces, held on the rim when out of range.
    fn draw_coop(
        &self,
        draw_list: &DrawListMut,
        player: &Snapshot,
        center: [f32; 2],
        radius: f32,
        s: f32,
    ) {
        let pixels_per_meter = radius / ZOOM_RANGES[self.zoom];
        let rim = radius - 12.0 * s;
        for partner in &player.coop {
            let d = self.to_screen([
                partner.position[0] - player.position[0],
                partner.position[1] - player.position[1],
            ]);
            let mut offset = [d[0] * pixels_per_meter, d[1] * pixels_per_meter];
            let distance = length(offset);
            let inside = distance <= rim;
            if !inside {
                offset = [offset[0] / distance * rim, offset[1] / distance * rim];
            }
            let at = [center[0] + offset[0], center[1] + offset[1]];
            let Some(forward) = normalize(self.to_screen(partner.facing)) else {
                continue;
            };
            let right = [-forward[1], forward[0]];
            let size = if inside { 7.0 } else { 5.5 } * s;
            let point = |f: f32, r: f32| {
                [
                    at[0] + (forward[0] * f + right[0] * r) * size,
                    at[1] + (forward[1] * f + right[1] * r) * size,
                ]
            };
            let (tip, left, back, right) = (
                point(1.0, 0.0),
                point(-0.7, -0.75),
                point(-0.35, 0.0),
                point(-0.7, 0.75),
            );
            let opacity = if inside { 1.0 } else { 0.8 };
            let fill = self.color([130, 200, 255], opacity);
            draw_list
                .add_triangle(tip, left, back, fill)
                .filled(true)
                .build();
            draw_list
                .add_triangle(tip, back, right, fill)
                .filled(true)
                .build();
            draw_list
                .add_polyline(
                    vec![tip, left, back, right, tip],
                    self.color(BLACK, 0.85 * opacity),
                )
                .thickness(1.2 * s)
                .build();
        }
    }

    /// A glowing golden point, like the grace icons on the world map.
    fn draw_graces(
        &self,
        draw_list: &DrawListMut,
        player: &Snapshot,
        center: [f32; 2],
        radius: f32,
        s: f32,
    ) {
        let pixels_per_meter = radius / ZOOM_RANGES[self.zoom];
        for grace in &player.graces {
            let d = self.to_screen([grace[0] - player.position[0], grace[1] - player.position[1]]);
            if length(d) * pixels_per_meter > radius - 10.0 * s {
                continue;
            }
            let at = [
                center[0] + d[0] * pixels_per_meter,
                center[1] + d[1] * pixels_per_meter,
            ];
            let circle = |r: f32, color: ImColor32| {
                draw_list.add_circle(at, r * s, color).filled(true).build();
            };
            circle(10.0, self.color([255, 210, 110], 0.12));
            circle(6.5, self.color([255, 214, 120], 0.25));
            draw_list
                .add_circle(at, 4.5 * s, self.color([255, 232, 170], 0.95))
                .thickness(1.6 * s)
                .build();
            circle(2.2, self.color([255, 248, 222], 1.0));
        }
    }

    fn draw_arrow(&self, draw_list: &DrawListMut, center: [f32; 2], s: f32) {
        let Some(arrow) = self.arrow else {
            return;
        };
        let (sin, cos) = self.facing_yaw.unwrap_or(0.0).sin_cos();
        let forward = self.to_screen([sin, cos]);
        let right = [-forward[1], forward[0]];
        let size = 30.0 * s;
        // Image corners, placed around the pivot with the image's up along `forward`.
        let corner = |u: f32, v: f32| {
            let (x, y) = ((u - ARROW_PIVOT[0]) * size, (v - ARROW_PIVOT[1]) * size);
            [
                center[0] + right[0] * x - forward[0] * y,
                center[1] + right[1] * x - forward[1] * y,
            ]
        };
        draw_list
            .add_image_quad(
                arrow,
                corner(0.0, 0.0),
                corner(1.0, 0.0),
                corner(1.0, 1.0),
                corner(0.0, 1.0),
            )
            .col(self.color([255, 255, 255], 1.0))
            .build();
    }

    fn draw_text_centered(
        &self,
        ui: &Ui,
        draw_list: &DrawListMut,
        center: [f32; 2],
        color: ImColor32,
        text: &str,
    ) {
        let [w, h] = ui.calc_text_size(text);
        let x = (center[0] - w * 0.5).round();
        let y = (center[1] - h * 0.5).round();
        draw_list.add_text([x + 1.0, y + 1.0], self.color(BLACK, 0.8), text);
        draw_list.add_text([x, y], color, text);
    }
}

/// Normal gameplay: no menu changing the HUD and no fade covering the screen.
fn in_control(snapshot: &Snapshot) -> bool {
    snapshot.hud_visible && snapshot.fade < 0.05
}

/// Map for Goblins' icons from its map API, packed into one texture.
struct IconAtlas {
    texture: TextureId,
    cells: HashMap<u32, IconCell>,
}

struct IconCell {
    uv_min: [f32; 2],
    uv_max: [f32; 2],
    /// Half the drawn size at 1080p. Icons are cropped tight, so smaller art stays smaller.
    half: [f32; 2],
}

impl IconAtlas {
    /// Map for Goblins draws its icons from 96px art.
    const CELL: usize = 96;
    const COLUMNS: usize = 16;

    fn load(render_context: &mut dyn RenderContext) -> Option<Self> {
        let icons: Vec<_> = crate::mfg::icons()
            .into_iter()
            .filter(|icon| icon.width <= Self::CELL && icon.height <= Self::CELL)
            .collect();
        if icons.is_empty() {
            return None;
        }
        let width = Self::CELL * Self::COLUMNS;
        let height = Self::CELL * icons.len().div_ceil(Self::COLUMNS);
        let mut rgba = vec![0u8; width * height * 4];
        let mut cells = HashMap::new();
        for (i, icon) in icons.iter().enumerate() {
            let (x, y) = (
                i % Self::COLUMNS * Self::CELL,
                i / Self::COLUMNS * Self::CELL,
            );
            for row in 0..icon.height {
                let at = ((y + row) * width + x) * 4;
                rgba[at..at + icon.width * 4]
                    .copy_from_slice(&icon.rgba[row * icon.width * 4..(row + 1) * icon.width * 4]);
            }
            let uv = |px: usize, py: usize| [px as f32 / width as f32, py as f32 / height as f32];
            cells.insert(
                icon.source_icon_id,
                IconCell {
                    uv_min: uv(x, y),
                    uv_max: uv(x + icon.width, y + icon.height),
                    half: [
                        8.0 * icon.width as f32 / Self::CELL as f32,
                        8.0 * icon.height as f32 / Self::CELL as f32,
                    ],
                },
            );
        }
        let texture = render_context
            .load_texture(&rgba, width as u32, height as u32)
            .ok()?;
        Some(Self { texture, cells })
    }
}

fn load_png(render_context: &mut dyn RenderContext, bytes: &[u8]) -> Option<TextureId> {
    let mut decoder = png::Decoder::new(bytes);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut reader = decoder.read_info().ok()?;
    let mut rgba = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut rgba).ok()?;
    render_context
        .load_texture(&rgba[..info.buffer_size()], info.width, info.height)
        .ok()
}

fn length([x, y]: [f32; 2]) -> f32 {
    (x * x + y * y).sqrt()
}

/// Wraps an angle difference into [-PI, PI) so easing takes the short way around.
fn wrap_angle(angle: f32) -> f32 {
    (angle + PI).rem_euclid(TAU) - PI
}

fn normalize(v: [f32; 2]) -> Option<[f32; 2]> {
    let l = length(v);
    (l > 1e-3).then(|| [v[0] / l, v[1] / l])
}
