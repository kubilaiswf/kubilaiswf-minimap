using System;
using System.IO;
using System.IO.Compression;
using System.Linq;
using System.Collections.Generic;
using System.Text.Json;
using HKLib.hk2018;
using HKLib.Serialization.hk2018.Binary;
using SoulsFormats;

public static class NavmeshExport
{
    public sealed class Result
    {
        public int Meshes { get; set; }
        public int Vertices { get; set; }
        public int Faces { get; set; }
        public int Edges { get; set; }
        public string[] SkippedEntries { get; set; }
    }

    // Keep native mesh coordinates and all raw edge links. NVA holds instance transforms.
    public static Result Export(byte[] bytes, string path, string mapId)
    {
        var result = new Result();
        var meshes = new List<object>();
        var skipped = new List<string>();
        var serializer = new HavokBinarySerializer();
        var binder = BND4.Read(bytes);
        foreach (var entry in binder.Files)
        {
            var name = entry.Name.Replace('\\', '/').Split('/').Last();
            // 'o' meshes are another variant; do not overlay both variants as duplicate floors.
            if (!name.StartsWith("n") || !name.EndsWith(".hkx"))
            {
                skipped.Add(name);
                continue;
            }
            using var source = new MemoryStream(entry.Bytes.ToArray(), false);
            var navs = serializer.ReadAllObjects(source).OfType<hkaiNavMesh>().ToList();
            if (navs.Count != 1) throw new InvalidDataException(name + ": expected one hkaiNavMesh");
            var nav = navs[0];
            var faces = new List<int[]>();
            foreach (var face in nav.m_faces)
            {
                if (face.m_numEdges < 3 || face.m_startEdgeIndex < 0 ||
                    face.m_startEdgeIndex + face.m_numEdges > nav.m_edges.Count)
                    throw new InvalidDataException(name + ": invalid polygon edge range");
                var indices = new int[face.m_numEdges];
                for (int i = 0; i < indices.Length; i++)
                {
                    var edge = nav.m_edges[face.m_startEdgeIndex + i];
                    if (edge.m_a >= nav.m_vertices.Count || edge.m_b >= nav.m_vertices.Count)
                        throw new InvalidDataException(name + ": invalid vertex index");
                    var next = nav.m_edges[face.m_startEdgeIndex + (i + 1) % indices.Length];
                    if (edge.m_b != next.m_a)
                        throw new InvalidDataException(name + ": polygon edge chain is not closed");
                    indices[i] = (int)edge.m_a;
                }
                faces.Add(indices);
            }
            foreach (var v in nav.m_vertices)
                if (!float.IsFinite(v.X) || !float.IsFinite(v.Y) || !float.IsFinite(v.Z))
                    throw new InvalidDataException(name + ": non-finite vertex");
            meshes.Add(new {
                name,
                binder_id = entry.ID,
                vertices = nav.m_vertices.Select(v => new[] { v.X, v.Y, v.Z }).ToArray(),
                vertex_w = nav.m_vertices.Select(v => v.W).ToArray(),
                faces,
                face_edges = nav.m_faces.Select(f => new[] { f.m_startEdgeIndex, (int)f.m_numEdges,
                    f.m_startUserEdgeIndex, (int)f.m_numUserEdges }).ToArray(),
                // a, b, opposite_edge, opposite_face, flags. Preserve packed links/sentinels.
                edges = nav.m_edges.Select(e => new long[] { e.m_a, e.m_b,
                    e.m_oppositeEdge, e.m_oppositeFace, (long)e.m_flags }).ToArray(),
                face_data = nav.m_faceData,
                face_data_stride = nav.m_faceDataStriding,
                edge_data = nav.m_edgeData,
                edge_data_stride = nav.m_edgeDataStriding
            });
            result.Meshes++;
            result.Vertices += nav.m_vertices.Count;
            result.Faces += nav.m_faces.Count;
            result.Edges += nav.m_edges.Count;
        }
        result.SkippedEntries = skipped.ToArray();
        WriteGzip(path, new { schema_version = 1, map_id = mapId,
            coordinate_space = "havok_file_xyz_y_up; apply matching NVA instance transform",
            meaning = "AI navigation polygons, not semantic rooms or complete collision geometry",
            meshes, skipped_entries = skipped });
        return result;
    }

    public static void ExportNva(byte[] bytes, string path)
    {
        var nva = NVA_ER.Read(bytes);
        WriteGzip(path, nva, true);
    }

    private static void WriteGzip(string path, object value, bool fields = false)
    {
        using var file = File.Create(path);
        using var gzip = new GZipStream(file, CompressionLevel.Fastest);
        JsonSerializer.Serialize(gzip, value, value.GetType(), new JsonSerializerOptions { IncludeFields = fields });
    }
}
