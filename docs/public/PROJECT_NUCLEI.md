# Project Nuclei — VRG 0.6.0

Project Nuclei is the read-only local graph viewer in the Linux x86_64 GitHub release. It uses the existing VRG indices and the browser's WebGL2 renderer. No JavaScript package manager, external asset service or network connection is needed during viewing.

## Open an existing authorized index

```sh
vrg graph /absolute/path/to/registered-project
vrg view --all
vrg view /absolute/path/to/registered-project --no-open --port 0
vrg --config /absolute/path/to/profile.toml view /absolute/path/to/authorized-root
```

`graph` followed by an explicit project path opens the same native viewer as `view`. The existing `vrg graph ROOT_ID RELATIVE_PATH` command still lists relations for an indexed source. `--no-open` prints the local session URL without launching a browser. Ctrl+C closes the server.

The URL includes a temporary session secret. Do not share it, publish it, include it in a screenshot or retain it in logs. The server accepts loopback requests only and expires its session. It provides no endpoints for editing notes, indexing, migration, model downloads or permission changes.

Opening a path does not authorize its contents or scan neighboring directories. Use an existing explicitly authorized profile and index. If no registration matches, select an existing profile with `--config`; the viewer does not create one.

## Registered projects and source profiles

The project catalog reads `$XDG_CONFIG_HOME/vrg/global-projects.json`, with the normal `~/.config` fallback. Several source or memory profiles can belong to one canonical project. Transport/profile IDs remain distinct for scoped inspector requests; canonical identity determines the visual nucleus. Worktrees and aliases retain their authorized memberships instead of becoming additional projects.

An optional pinned source-profile registry is read from `$XDG_CONFIG_HOME/vrg/viewer-sources.json`. `VRG_VIEW_SOURCE_REGISTRY` can select an explicit registry file instead. These files are installation configuration and are not included in the release. A source registry references existing profiles and their hashes; it is not a directory-discovery mechanism.

The source adapter accepts the supported schema 1/2 projection policies. It checks profile and manifest pins, authorized roots and source identities, exact output membership, current source revisions and the indexed generation. Changing a source, profile, authorization or active snapshot can exclude an affected entry or invalidate the view. Nothing in the viewer repairs or weakens those checks. New sources require their existing ingestion/admission workflow separately.

## Navigate the map

- The overview labels only real project names. Each project has a stable color and a separate spatial region.
- Hover a project to inspect its available chunk, folder and relation counts. Click it to focus its real folder groups.
- Click a folder to explore its individual chunks. Shared origins remain available through their real folder memberships without manufacturing new chunks.
- Click a chunk to open its inspector, native ID, authorized origin, available exact content, revision and relations. The inspector can be collapsed.
- Use wheel zoom, drag pan, fit, back navigation and the minimap. Search, classification/revision filters and the accessible node list remain available.
- Separation and internal density controls alter the spatial presentation. Project colors are the default; classification colors require an explicit mode change.

World coordinates are deterministic for the same identities, memberships and settings. Resizing the viewport or applying a filter changes the camera or selected set rather than recomputing the world layout. Changes to the underlying data can legitimately change the corresponding layout.

## Real data, LOD and coverage

The viewer progressively receives the authorized snapshot pages and retains prior pages. LOD selects real points for drawing at a distance; it does not replace native chunks with invented records. Closer navigation and the accessible list retain access to individual authorized chunks. Every authorized loaded chunk remains represented in the GPU data set when WebGL2 is available.

Counters distinguish indexed entries, entries allowed by the current policy, received memberships, retained canonical chunk identities, GPU residence, LOD draw count, viewport visibility and filters. Source/revision denials and partial-profile failures remain explicit. A denial is not a render limit and does not grant permission to display the entry anyway. Counts come from the current snapshots and are never fixed to a release-demo volume.

Edges retain their recorded provenance or use the existing compatible-vector similarity mechanism. Spatial distance is not a semantic score. Line detail can be reduced to preserve responsiveness without discarding the loaded data. Independent profiles do not gain cross-project relations merely because they share a canvas; the viewer never invents connecting lines for appearance.

## Scope and recovery

Linux x86_64 is the release scope. A WebGL2-capable browser is required for the GPU graph; the accessible list remains available when GPU rendering is unavailable. Performance depends on index volume, authorization checks, storage, browser and GPU. Local validation is not a guarantee for every machine or a certification of unrelated VRG phases.

No personal projects, source notes, graph snapshots, private screenshots, models, credentials or local registries are distributed. Keep derived stores outside source roots. Installing this package does not activate Codex hooks, trust a hook, replace another MCP executable or migrate a database.

For rollback, close the viewer and restore the previous executable or PATH entry. Retain profiles and stores; a viewer rollback does not require deleting or rewriting an index. See the [release protocol](../RELEASE_PROTOCOL.md) for artifact verification and publication safeguards.
