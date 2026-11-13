param(
    [Parameter(Mandatory)][string]$GameRoot,
    [Parameter(Mandatory)][string]$SmithboxRoot,
    [string]$OutputRoot = "$PSScriptRoot\..\outputs\navigation",
    [string[]]$MapIDs = @(),
    [int]$TimeoutSeconds = 900
)

$ErrorActionPreference = 'Stop'
$OutputRoot = [IO.Path]::GetFullPath($OutputRoot)
$clock = [Diagnostics.Stopwatch]::StartNew()
function Assert-Time { if ($clock.Elapsed.TotalSeconds -gt $TimeoutSeconds) { throw 'Scan time limit reached' } }
New-Item -ItemType Directory -Force -Path $OutputRoot | Out-Null
$originalBase = [AppContext]::BaseDirectory
$archives = [ordered]@{}
try {
    # HKLib expects its type registry under the host base directory. Process-local only.
    [AppContext]::SetData('APP_CONTEXT_BASE_DIRECTORY', $SmithboxRoot)
    foreach ($dll in 'Andre.Core','Andre.SoulsFormats','Andre.IO','Andre.Formats',
        'Havok.HKLib','Havok.HKLib.Reflection','Havok.HKLib.Serialization') {
        Add-Type -Path (Join-Path $SmithboxRoot "$dll.dll")
    }
    $oodle = Join-Path $GameRoot 'oo2core_6_win64.dll'
    [Runtime.InteropServices.NativeLibrary]::Load($oodle) | Out-Null
    [SoulsFormats.Oodle].GetField('Oodle6Exists',[Reflection.BindingFlags]'NonPublic,Public,Static').SetValue($null,$true)
    $refs = @('Andre.SoulsFormats','Havok.HKLib','Havok.HKLib.Reflection','Havok.HKLib.Serialization' |
        ForEach-Object { Join-Path $SmithboxRoot "$_.dll" })
    $refs += @(Get-ChildItem -LiteralPath (Join-Path $PSHOME 'ref') -Filter '*.dll' -File | ForEach-Object FullName)
    Add-Type -Path "$PSScriptRoot\NavmeshExport.cs" -ReferencedAssemblies $refs
    foreach ($name in 'Data0','Data1','Data2','Data3','DLC') {
        Assert-Time
        $bhd = Join-Path $GameRoot "$name.bhd"
        if (Test-Path -LiteralPath $bhd) {
            $archives[$name] = [Andre.Formats.BinderArchive]::new($bhd,(Join-Path $GameRoot "$name.bdt"),[Andre.Core.Game]::ER)
        }
    }
    function Read-GameFile([string]$VirtualPath) {
        $hash = [Andre.Core.Util.BhdDictionary]::ComputeHash($VirtualPath,[SoulsFormats.BHD5+Game]::EldenRing)
        foreach ($name in $archives.Keys) {
            $data = $archives[$name].TryReadFileFromHash($hash)
            if ($null -ne $data) { return @{ Bytes = $data; Archive = $name; Path = $VirtualPath } }
        }
        return $null
    }
    $dictionaryPath = Join-Path $SmithboxRoot 'Assets\UXM Dictionaries\EldenRingDictionary.txt'
    $dictionary = Get-Content -LiteralPath $dictionaryPath
    $aliases = @{}
    Get-Content -Raw -LiteralPath (Join-Path $SmithboxRoot 'Assets\Aliases\ER\MapNames.json') |
        ConvertFrom-Json | ForEach-Object { $aliases[$_.ID] = $_.Name }
    $pattern='^/map/m(30|31|32|40|41|42|43)/m[^/]+/.*\.nvmhktbnd\.dcx$'
    $navPaths = @($dictionary | Where-Object { $_ -match $pattern } | Sort-Object -Unique)
    if ($MapIDs.Count) { $navPaths = @($navPaths | Where-Object { ($_ -split '/')[-2] -in $MapIDs }) }
    if (-not $navPaths.Count) { throw 'No matching navigation bundle paths in dictionary' }
    foreach ($requested in $MapIDs) {
        if (-not ($navPaths | Where-Object { ($_ -split '/')[-2] -eq $requested })) {
            throw "Requested map not in dictionary navigation scope: $requested"
        }
    }
    $records = [Collections.Generic.List[object]]::new()
    $errors = [Collections.Generic.List[object]]::new()
    foreach ($path in $navPaths) {
        Assert-Time
        $id = ($path -split '/')[-2]
        try {
            $source = Read-GameFile $path
            if ($null -eq $source) { throw 'Navmesh bundle absent from current install' }
            $dir = Join-Path $OutputRoot $id
            New-Item -ItemType Directory -Force -Path $dir | Out-Null
            $result = [NavmeshExport]::Export($source.Bytes,(Join-Path $dir 'navmesh.json.gz'),$id)
            $nvaSource = Read-GameFile ($path -replace '\.nvmhktbnd\.dcx$', '.nva.dcx')
            if ($null -eq $nvaSource) { throw 'NVA placements absent' }
            [NavmeshExport]::ExportNva($nvaSource.Bytes,(Join-Path $dir 'placements.json.gz'))
            $msbSource = Read-GameFile "/map/mapstudio/$id.msb.dcx"
            if ($null -eq $msbSource) { throw 'MSB layout absent' }
            $msb = [SoulsFormats.MSBE]::Read([byte[]]$msbSource.Bytes)
            $parts = @($msb.Parts.GetEntries() | Where-Object { $_.GetType().Name -in @('MapPiece','Asset','Collision','ConnectCollision') } | ForEach-Object {
                [ordered]@{type=$_.GetType().Name; name=$_.Name; model=$_.ModelName;
                    position=@($_.Position.X,$_.Position.Y,$_.Position.Z);
                    rotation=@($_.Rotation.X,$_.Rotation.Y,$_.Rotation.Z);
                    scale=@($_.Scale.X,$_.Scale.Y,$_.Scale.Z); entity_id=$_.EntityID}
            })
            [ordered]@{map_id=$id; name=$aliases[$id]; parts=$parts} | ConvertTo-Json -Depth 12 -Compress |
                Set-Content -LiteralPath (Join-Path $dir 'layout.json') -Encoding utf8
            $records.Add([pscustomobject]@{map_id=$id;name=$aliases[$id];archive=$source.Archive;
                meshes=$result.Meshes;vertices=$result.Vertices;faces=$result.Faces;edges=$result.Edges;
                source_path=$source.Path;navmesh_sha256=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([byte[]]$source.Bytes));
                nva_sha256=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([byte[]]$nvaSource.Bytes));
                msb_sha256=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([byte[]]$msbSource.Bytes))})
        } catch {
            $errors.Add([pscustomobject]@{map_id=$id;error=$_.Exception.ToString()})
        }
        if (($records.Count+$errors.Count) % 10 -eq 0) {
            Write-Output "Scanned $($records.Count+$errors.Count)/$($navPaths.Count); completed=$($records.Count), errors=$($errors.Count)"
        }
    }
    $records | Export-Csv -LiteralPath (Join-Path $OutputRoot 'maps.csv') -NoTypeInformation -Encoding utf8
    ConvertTo-Json -InputObject @($errors.ToArray()) -Depth 5 |
        Set-Content -LiteralPath (Join-Path $OutputRoot 'errors.json') -Encoding utf8
    [ordered]@{schema_version=1; created_utc=[DateTime]::UtcNow.ToString('o');game_root=$GameRoot;
        exe_version=(Get-Item -LiteralPath (Join-Path $GameRoot 'eldenring.exe')).VersionInfo.FileVersion;
        scope='Small-dungeon navigation from original local archives (m30-m32/m40-m43); no mod overlays';
        dictionary_sha256=(Get-FileHash -LiteralPath $dictionaryPath -Algorithm SHA256).Hash;
        candidates=$navPaths.Count;completed=$records.Count;failed=$errors.Count;
        meshes=($records | Measure-Object meshes -Sum).Sum;faces=($records | Measure-Object faces -Sum).Sum;
        edge_columns=@('vertex_a','vertex_b','raw_opposite_edge','raw_opposite_face','raw_flags');
        face_edges_columns=@('start_edge','num_edges','start_user_edge','num_user_edges');
        limitations=@('Navigation sections and polygons are not semantic rooms. No room labels inferred.',
            'AI navigation mesh omits some player-only paths and is not complete walls/collision.',
            'Raw polygon coordinates require NVA instance scale, rotation and translation.',
            'NVA connectors and gate nodes are navigation links; not guaranteed doors or current door state.',
            'o-prefixed alternative meshes excluded; filenames retained in skipped_entries.',
            'No in-game verification or save access. No files written to game/tool install.')
    } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $OutputRoot 'manifest.json') -Encoding utf8
    Write-Output "Done: completed=$($records.Count), errors=$($errors.Count), elapsed=$([int]$clock.Elapsed.TotalSeconds)s"
    if ($errors.Count) { exit 2 }
} finally {
    foreach ($archive in $archives.Values) { $archive.Dispose() }
    [AppContext]::SetData('APP_CONTEXT_BASE_DIRECTORY', $originalBase)
}
