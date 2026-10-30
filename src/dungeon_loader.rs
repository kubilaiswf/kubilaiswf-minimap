//! Load one locally generated dungeon at a time, away from the render thread.
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};

use crate::dungeon::DungeonMap;

#[derive(Clone)]
struct Entry { block:[u8;4], name:String, path:PathBuf }

pub struct Loaded {
    pub block:[u8;4],
    pub name:String,
    pub map:io::Result<DungeonMap>,
}

pub struct DungeonLoader {
    entries:HashMap<[u8;4],Entry>,
    requests:Sender<Entry>,
    results:Mutex<Receiver<Loaded>>,
}

impl DungeonLoader {
    pub fn spawn(directory:&Path, log:&Path) -> io::Result<Self> {
        let entries=read_catalog(directory,&std::fs::read_to_string(directory.join("maps.tsv"))?)?;
        let (requests,request_rx)=channel::<Entry>();
        let (result_tx,results)=channel();
        let log=log.to_owned();
        std::thread::spawn(move || {
            while let Ok(mut entry)=request_rx.recv() {
                // If a loading screen produced several block changes, only read the newest.
                for newer in request_rx.try_iter() {entry=newer;}
                let map=DungeonMap::load_for_block(&entry.path,entry.block);
                if let Err(error)=&map {
                    let _=std::fs::write(&log,format!("Dungeon {:?} unavailable: {error}\n",entry.block));
                }
                if result_tx.send(Loaded {block:entry.block,name:entry.name,map}).is_err() {break;}
            }
        });
        Ok(Self {entries,requests,results:Mutex::new(results)})
    }

    pub fn request(&self, block:[u8;4]) {
        if let Some(entry)=self.entries.get(&block) {let _=self.requests.send(entry.clone());}
    }

    pub fn poll(&self) -> Option<Loaded> {
        self.results.lock().unwrap().try_iter().last()
    }
}

fn read_catalog(directory:&Path, text:&str) -> io::Result<HashMap<[u8;4],Entry>> {
    let invalid=||io::Error::new(io::ErrorKind::InvalidData,"invalid dungeon catalog");
    let mut lines=text.lines();
    if lines.next()!=Some("map_id\tname") {return Err(invalid());}
    let mut entries=HashMap::new();
    for line in lines {
        let (id,name)=line.split_once('\t').ok_or_else(invalid)?;
        let block=parse_map_id(id).ok_or_else(invalid)?;
        if name.is_empty() || name.len()>160 || name.chars().any(char::is_control) {return Err(invalid());}
        let entry=Entry {block,name:name.to_owned(),path:directory.join(format!("{id}.nav"))};
        if entries.insert(block,entry).is_some() || entries.len()>512 {return Err(invalid());}
    }
    if entries.is_empty() {return Err(invalid());}
    Ok(entries)
}

fn parse_map_id(id:&str) -> Option<[u8;4]> {
    let bytes=id.as_bytes();
    if bytes.len()!=12 || bytes[0]!=b'm' || [3,6,9].iter().any(|&i|bytes[i]!=b'_') {return None;}
    let mut block=[0;4];
    for (axis,offset) in [1,4,7,10].into_iter().enumerate() {
        if !bytes[offset..offset+2].iter().all(u8::is_ascii_digit) {return None;}
        block[axis]=(bytes[offset]-b'0')*10+bytes[offset+1]-b'0';
    }
    matches!(block[0],30..=32|40..=43).then_some(block)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_matches_exact_blocks_and_rejects_paths_and_duplicates() {
        let text="map_id\tname\nm30_01_00_00\tImpaler's Catacombs\nm43_01_00_00\tDragon's Pit\n";
        let entries=read_catalog(Path::new("dungeons"),text).unwrap();
        assert_eq!(entries.len(),2);
        assert_eq!(entries[&[43,1,0,0]].name,"Dragon's Pit");
        assert!(!entries.contains_key(&[43,1,0,1]));
        for id in ["../bad.nav","m60_01_00_00","m30_01_00_0x","m30_01_0/_00"] {
            assert!(read_catalog(Path::new("dungeons"),&format!("map_id\tname\n{id}\tbad\n")).is_err());
        }
        assert!(read_catalog(Path::new("dungeons"),&(text.to_owned()+"m30_01_00_00\tduplicate\n")).is_err());
    }

    #[test]
    #[ignore="Requires DUNGEON_NAV_TEST_DIR generated from the local game"]
    fn real_maps_decode_and_background_loader_switches_exact_blocks() {
        use std::time::Duration;
        let path=std::env::var_os("DUNGEON_NAV_TEST_DIR").expect("Set local dungeon directory");
        let directory=Path::new(&path);
        let loader=DungeonLoader::spawn(directory,&directory.join("loader-test.log")).unwrap();
        assert_eq!(loader.entries.len(),60);
        let mut total=0;
        for entry in loader.entries.values() {
            let map=DungeonMap::load_for_block(&entry.path,entry.block).unwrap();
            total+=map.triangles.len();
            for triangle in map.triangles.iter().step_by((map.triangles.len()/32).max(1)) {
                let p:[f32;3]=std::array::from_fn(|axis|triangle.points.iter().map(|p|p[axis]).sum::<f32>()/3.);
                assert!((map.floor_height([p[0],p[1]+0.05,p[2]])-p[1]).abs()<0.15,"{:?} {p:?}",entry.block);
            }
        }
        assert!(total>100_000);
        for block in [[30,1,0,0],[31,3,0,0],[32,1,0,0],[40,0,0,0],[43,1,0,0],[30,0,0,0]] {
            loader.request(block);
            let loaded=loader.results.lock().unwrap().recv_timeout(Duration::from_secs(10)).unwrap();
            assert_eq!(loaded.block,block);
            assert_eq!(loaded.map.unwrap().block,block);
        }
        let tombsward=&loader.entries[&[30,0,0,0]];
        assert!(DungeonMap::load_for_block(&tombsward.path,[30,1,0,0]).is_err());
        loader.request([60,1,0,0]);
        assert!(loader.results.lock().unwrap().try_recv().is_err());
    }
}
