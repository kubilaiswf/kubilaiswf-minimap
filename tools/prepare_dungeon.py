"""Generate private dungeon sidecars from local navigation exports.
Usage: python tools/prepare_dungeon.py NAVIGATION_ROOT DLL_DIRECTORY
The generated sidecar contains game-derived geometry. Do not add it to Git.
"""
import argparse
import csv
import gzip
import hashlib
import json
import math
import re
from pathlib import Path
import struct


SMALL_AREAS = {30, 31, 32, 40, 41, 42, 43}


def map_block(map_id):
    if not re.fullmatch(r'm\d{2}_\d{2}_\d{2}_\d{2}', map_id):
        raise ValueError('Invalid map identifier')
    block = [int(part) for part in map_id[1:].split('_')]
    if not 10 <= block[0] < 60:
        raise ValueError('Only non-overworld dungeon blocks are supported')
    return bytes(block)


def build_data(root, map_id):
    block = map_block(map_id)
    directory = root / map_id
    with gzip.open(directory / 'navmesh.json.gz', 'rt', encoding='utf-8') as stream:
        navigation = json.load(stream)
    with gzip.open(directory / 'placements.json.gz', 'rt', encoding='utf-8') as stream:
        placements = json.load(stream)
    models = {int(m['name'].rsplit('_', 1)[-1].split('.')[0]): m for m in navigation['meshes']}
    instances = {n['NameID']: n for n in placements['Navmeshes'] if (n['NameID'] & 0xffffffff) < 2_000_000_000}
    linked_edges = {(c['MainNameID'], link['EdgeIndex']) for c in placements['Connectors']
                    for link in c['NavMeshConnections']}
    triangles, boundaries = [], []
    for section, instance in instances.items():
        if any(abs(instance['Position'][k]) > 1e-5 or abs(instance['Rotation'][k]) > 1e-5
               or abs(instance['Scale'][k]-1) > 1e-5 for k in 'XYZ'):
            raise ValueError('This pilot requires identity NVA instance transforms')
        mesh = models[instance['ModelID']]
        if len(mesh['faces']) != instance['FaceCount']:
            raise ValueError('NVA and navmesh polygon counts differ')
        for face in mesh['faces']:
            points = [mesh['vertices'][i] for i in face]
            for i in range(1, len(points)-1):
                triangle = [points[0], points[i], points[i+1]]
                a,b,c = triangle
                area = (b[0]-a[0])*(c[2]-a[2])-(b[2]-a[2])*(c[0]-a[0])
                if abs(area) > 1e-7:
                    triangles.append((triangle, instance['ModelID']))
        for i, edge in enumerate(mesh['edges']):
            if edge[3] == 0xffffffff and (section, i) not in linked_edges:
                boundaries.append([mesh['vertices'][edge[0]], mesh['vertices'][edge[1]]])
    if not 0 < len(triangles) <= 200_000 or len(boundaries) > 200_000:
        raise ValueError(f'{map_id}: empty or oversized dungeon data')
    data = bytearray(b'ERDNG001' + block + struct.pack('<II', len(triangles), len(boundaries)))
    for points, section in triangles:
        values = [v for p in points for v in p]
        if not all(math.isfinite(x) for x in values): raise ValueError('Invalid coordinate')
        data.extend(struct.pack('<9fI', *values, section))
    for points in boundaries:
        values = [v for p in points for v in p]
        if not all(math.isfinite(x) for x in values): raise ValueError('Invalid boundary coordinate')
        data.extend(struct.pack('<6f', *values))
    info = dict(map_id=map_id, triangles=len(triangles), boundary_segments=len(boundaries),
                bytes=len(data), sha256=hashlib.sha256(data).hexdigest(),
                source_navmesh_sha256=hashlib.sha256((directory/'navmesh.json.gz').read_bytes()).hexdigest(),
                source_placements_sha256=hashlib.sha256((directory/'placements.json.gz').read_bytes()).hexdigest(),
                limitations=['AI walkable surfaces, not physical walls or verified room boundaries.',
                             'Vanilla layout only; live alignment requires gameplay verification.'])
    return data, info


def prepare_small_dungeons(root, output):
    with (root/'maps.csv').open(encoding='utf-8-sig', newline='') as stream:
        records = [row for row in csv.DictReader(stream) if map_block(row['map_id'])[0] in SMALL_AREAS]
    ids = [row['map_id'] for row in records]
    if not records or len(ids) != len(set(ids)):
        raise ValueError('Missing or duplicate dungeon map records')
    destination = output/'dungeons'
    destination.mkdir(parents=True, exist_ok=True)
    maps = []
    for record in sorted(records, key=lambda row: row['map_id']):
        map_id = record['map_id']
        data, info = build_data(root, map_id)
        info['name'] = record['name'] or map_id
        if any(c in info['name'] for c in '\t\r\n'):
            raise ValueError('Invalid map display name')
        (destination/f'{map_id}.nav').write_bytes(data)
        maps.append(info)
    (destination/'maps.tsv').write_text('map_id\tname\n'+''.join(
        f"{row['map_id']}\t{row['name']}\n" for row in maps), encoding='utf-8')
    receipt = dict(maps=maps, count=len(maps),
                   base_game=sum(map_block(row['map_id'])[0] < 40 for row in maps),
                   dlc=sum(map_block(row['map_id'])[0] >= 40 for row in maps))
    (destination/'data-manifest.json').write_text(json.dumps(receipt,indent=2)+'\n',encoding='utf-8')
    print(f"Prepared {len(maps)} small dungeon maps ({receipt['base_game']} base game, {receipt['dlc']} DLC).")


if __name__ == '__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('root', type=Path)
    parser.add_argument('output', type=Path)
    args=parser.parse_args()
    prepare_small_dungeons(args.root,args.output)
