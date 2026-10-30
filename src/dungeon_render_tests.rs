//! CPU rendering and exact-map state regressions; no game or GPU required.
use super::*;
fn config()->Config {
    Config { dungeon_mode:true,dungeon_floor_key:None,trail:true,rotate:true,compass:true,full_map:true,
        coop_players:true,hide_in_boss_fights:false,hide_in_caves:true,size:1.,margin:24.,toggle_key:None,zoom_key:None }
}
fn snapshot()->Snapshot {
    Snapshot { block_id:[30,0,0,0],local_position:[0.,0.,0.],position:[0.,0.],space:30,map:None,
        facing:[0.,1.],camera_right:[1.,0.],camera_forward:[0.,1.],fade:0.,hud_visible:true,
        boss_fight:false,in_cave:true,graces:vec![],beacons:vec![],points:vec![],coop:vec![],fragments:u32::MAX }
}
fn loaded(block:[u8;4])->dungeon_loader::Loaded {
    dungeon_loader::Loaded {block,name:"Test cave".into(),map:Ok(dungeon::DungeonMap {block,
        triangles:vec![dungeon::Triangle {points:[[-100.,0.,-100.],[100.,0.,-100.],[0.,0.,100.]],section:0}],boundaries:vec![]})}
}
#[test]
fn stale_results_missing_maps_and_nonfinite_positions_never_keep_old_dungeon() {
    let mut m=Minimap::new(Path::new("missing-game"),config(),true);let mut p=snapshot();
    m.select_dungeon(&p,0.1);m.accept_dungeon(loaded(p.block_id));assert!(m.dungeon_active(&p));
    m.config.dungeon_mode=false;assert!(!m.dungeon_active(&p));m.config.dungeon_mode=true;
    p.block_id=[31,0,0,0];m.select_dungeon(&p,0.1);assert!(!m.dungeon_active(&p));
    m.accept_dungeon(loaded([30,0,0,0]));assert!(m.dungeon.is_none());
    m.accept_dungeon(loaded(p.block_id));assert!(m.dungeon_active(&p));
    p.local_position[1]=f32::NAN;assert!(!m.dungeon_active(&p));
    p.local_position[1]=0.;p.in_cave=false;p.block_id=[60,34,42,0];m.select_dungeon(&p,0.1);
    assert!(!m.dungeon_active(&p));assert!(m.dungeon.is_none());
}
#[test]
fn clipped_dungeon_render_stays_in_disc_for_every_floor_view_and_zoom() {
    let mut ctx=Context::create();ctx.set_ini_filename(None);ctx.io_mut().display_size=[800.,600.];ctx.fonts().build_rgba32_texture();
    let mut m=Minimap::new(Path::new("missing-game"),config(),true);m.alpha=1.;let p=snapshot();m.dungeon=Some(loaded(p.block_id));
    assert!(dungeon::overlaps_view([[-100.,0.,-100.],[100.,0.,-100.],[0.,0.,100.]],[0.,0.,0.],10.,-2.,2.));
    assert!(!dungeon::overlaps_view([[100.,0.,100.],[101.,0.,100.],[100.,0.,101.]],[0.,0.,0.],10.,-2.,2.));
    for view in [dungeon::View::Layered,dungeon::View::Current,dungeon::View::All] {
        m.dungeon_view=view;
        for zoom in 0..3 {
            m.zoom=zoom;let ui=ctx.frame();m.draw_dungeon(&ui.get_foreground_draw_list(),&p,[400.,300.],96.);
            let data=ctx.render();assert!(data.total_vtx_count>0);
            for list in data.draw_lists() {for vertex in list.vtx_buffer() {
                let [x,y]=vertex.pos;assert!(x.is_finite() && y.is_finite());
                assert!((x-400.).hypot(y-300.)<=98.);
            }}
        }
    }
}
