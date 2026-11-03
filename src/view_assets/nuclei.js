/* Deterministic project/folder geometry for the existing VRG WebGL renderer.
 * Spatial disks are navigation bounds, never substitute graph/chunk records.
 * MIT OR Apache-2.0. No network, DOM, timers, dependencies or random state. */
'use strict';
(function (global) {
  const TAU = Math.PI * 2, GOLDEN = Math.PI * (3 - Math.sqrt(5));
  const colors = new Map(), projectLabels = new Map();
  const clamp = (n, lo, hi) => Math.max(lo, Math.min(hi, n));
  const keyOf = parts => JSON.stringify(parts);
  const compare = (a, b) => a < b ? -1 : a > b ? 1 : 0;

  function hash(value) {
    let h = 2166136261;
    for (const c of String(value == null ? '' : value)) {
      h ^= c.codePointAt(0); h = Math.imul(h, 16777619);
    }
    h ^= h >>> 16; h = Math.imul(h, 0x7feb352d);
    h ^= h >>> 15; h = Math.imul(h, 0x846ca68b);
    return (h ^ h >>> 16) >>> 0;
  }

  function hsl(hue, saturation, lightness) {
    const a = saturation * Math.min(lightness, 1 - lightness);
    const f = n => {
      const k = (n + hue / 30) % 12;
      return lightness - a * Math.max(-1, Math.min(k - 3, 9 - k, 1));
    };
    const rgb = [f(0), f(8), f(4)];
    return {rgb, css: 'rgb(' + rgb.map(v => Math.round(v * 255)).join(',') + ')', hue};
  }

  function color(id, label) {
    id = String(id);
    const hint = label || projectLabels.get(id) || id;
    if (!colors.has(id)) colors.set(id, hsl(hash(id + ':' + hint) % 360, .79, .64));
    return colors.get(id);
  }

  function register(projects) {
    const list = [...new Map((projects || []).map(p => {
      const rawId = p.nucleus_id || p.id || p.project_id;
      if (rawId == null || rawId === '') throw new Error('NUCLEUS_PROJECT_ID_MISSING');
      const id = String(rawId);
      return [id, {id, label: String(p.nucleus_label || p.label || p.project_label || id)}];
    })).values()].sort((a, b) => compare(a.id, b.id));
    const allocated = [];
    // Stable canonical identities, never installation-specific project names.
    colors.clear(); projectLabels.clear();
    for (const p of list) projectLabels.set(p.id, p.label);
    for (const p of list) {
      if (colors.has(p.id)) continue;
      let best = hash(p.id) % 360, bestDistance = -1;
      for (let i = 0; i < 48; i++) {
        const hue = (hash(p.id) % 360 + i * 137.50776405) % 360;
        const distance = allocated.length ? Math.min(...allocated.map(h => Math.min(Math.abs(h - hue), 360 - Math.abs(h - hue)))) : 180;
        if (distance > bestDistance) {bestDistance = distance; best = hue;}
      }
      allocated.push(best); colors.set(p.id, hsl(best, .79, .64));
    }
    return list;
  }

  function folderColor(id, path) {
    const base = color(id), value = hash(path);
    return hsl((base.hue + (value % 13) - 6 + 360) % 360, .70, .56 + (value % 13) / 100);
  }

  function memberInfo(node, member) {
    const id = String(member.nucleus_id || node.nucleus_id || member.project_id || node.project_id || '');
    if (!id) throw new Error('NUCLEUS_PROJECT_ID_MISSING');
    const source = member.source || node.source || {};
    const path = String(member.hierarchy_path || member.origin?.relative_path || source.relative_path || '');
    const parts = path.split('/').filter(Boolean); parts.pop();
    return {
      id, label: String(member.nucleus_label || node.nucleus_label || member.project_label || node.project_label || id),
      parts, path: parts.join('/'), scopeKey: member.folderKey,
      originalRoot: member.origin?.original_root || member.origin?.origins?.[0]?.original_root || source.root_id || '',
      sort: keyOf([id, path, member.origin?.original_root || '', source.root_id || '', source.relative_path || ''])
    };
  }

  function folderRecord(project, parts, parent) {
    const key = keyOf([project, ...parts]);
    return {key, parentKey: parent ? parent.key : null, depth: parts.length, projectId: project,
      label: parts[parts.length - 1] || '/', path: parts.join('/'), parts,
      children: new Map(), direct: new Set(), owned: new Set(), anchored: new Set(), scopeKeys: new Set(), sourceRoots: new Set(),
      x: 0, y: 0, radius: 0};
  }

  // Weighted circle packing on a phyllotactic candidate lattice. Folder area
  // follows its actual descendants, so a deep leaf does not lose a fixed
  // fraction of space for every unrelated sibling. Project radii remain capped.
  function pack(items, seed) {
    if (!items.length) return [];
    if (items.length === 1) return [{item: items[0], x: 0, y: 0, radius: 1}];
    const weighted = items.map(item => ({item, radius: Math.sqrt(Math.max(1, item.count))}));
    const sorted = weighted.sort((a, b) => b.radius - a.radius || compare(a.item.key, b.item.key));
    const cells = new Map(), placed = [], cellSize = sorted[0].radius * 2 + .4, phase = hash(seed) / 4294967296 * TAU;
    let candidate = 0, enclosing = 0;
    const key = (x, y) => x + ',' + y;
    for (const entry of sorted) {
      const {item, radius} = entry;
      let x, y, cx, cy;
      for (;;) {
        const distance = 3.15 * Math.sqrt(candidate), angle = candidate * GOLDEN + phase; candidate++;
        x = Math.cos(angle) * distance; y = Math.sin(angle) * distance;
        cx = Math.floor(x / cellSize); cy = Math.floor(y / cellSize);
        let collision = false;
        for (let dx = -1; dx <= 1 && !collision; dx++) for (let dy = -1; dy <= 1 && !collision; dy++) {
          for (const other of cells.get(key(cx + dx, cy + dy)) || []) {
            if (Math.hypot(x - other.x, y - other.y) < radius + other.radius + .18) {collision = true; break;}
          }
        }
        if (!collision) break;
      }
      const circle = {item, x, y, radius}; placed.push(circle);
      const cell = key(cx, cy); if (!cells.has(cell)) cells.set(cell, []); cells.get(cell).push(circle);
      enclosing = Math.max(enclosing, Math.hypot(x, y) + radius);
    }
    return placed.map(c => ({item: c.item, x: c.x / enclosing, y: c.y / enclosing, radius: c.radius / enclosing}));
  }

  function build(nodes, projects, options) {
    options = options || {};
    const separation = clamp(Number(options.separation) || 1, .2, 3);
    const density = clamp(Number(options.density) || 1, .4, 3);
    const groups = new Map(), primaries = new Map(), positions = new Map();
    const addProject = (id, label) => {
      if (!groups.has(id)) groups.set(id, {id, label, root: folderRecord(id, [], null), folderMap: new Map(), owned: new Set(), anchored: new Set()});
      return groups.get(id);
    };
    for (const p of projects || []) {
      const rawId = p.nucleus_id || p.id || p.project_id;
      if (rawId == null || rawId === '') throw new Error('NUCLEUS_PROJECT_ID_MISSING');
      const id = String(rawId);
      addProject(id, String(p.nucleus_label || p.label || p.project_label || id));
    }
    for (const node of nodes || []) {
      if (node.id == null) throw new Error('NUCLEUS_NODE_ID_MISSING');
      const members = (node.memberships?.length ? node.memberships : [node]).map(m => memberInfo(node, m)).sort((a, b) => compare(a.sort, b.sort));
      if (!members.length) throw new Error('NUCLEUS_MEMBERSHIP_MISSING');
      const primary = members[0]; primaries.set(node.id, primary);
      const seen = new Set();
      for (const member of members) {
        const project = addProject(member.id, member.label); project.owned.add(node.id);
        let folder = project.root;
        const folders = [folder];
        for (let i = 0; i < member.parts.length; i++) {
          const parts = member.parts.slice(0, i + 1), key = keyOf([project.id, ...parts]);
          if (!project.folderMap.has(key)) {
            const child = folderRecord(project.id, parts, folder); project.folderMap.set(key, child); folder.children.set(key, child);
          }
          folder = project.folderMap.get(key); folders.push(folder);
        }
        for (const f of folders) {
          f.owned.add(node.id);
          if (member.scopeKey) f.scopeKeys.add(member.scopeKey);
          if (member.originalRoot) f.sourceRoots.add(member.originalRoot);
        }
        if (!seen.has(folder.key)) {folder.direct.add(node.id); seen.add(folder.key);}
      }
      const anchoredProject = groups.get(primary.id); anchoredProject.anchored.add(node.id);
      let folder = anchoredProject.root; folder.anchored.add(node.id);
      for (let i = 0; i < primary.parts.length; i++) {
        folder = anchoredProject.folderMap.get(keyOf([primary.id, ...primary.parts.slice(0, i + 1)])); folder.anchored.add(node.id);
      }
    }

    register([...groups.values()]);
    const ordered = [...groups.values()].sort((a, b) => compare(a.id, b.id));
    const maxRadius = 440, spacing = (2.6 + separation * .7) * maxRadius;
    const centers = new Map();
    // Six evenly spaced slots per ring, for any number/names of projects.
    // No empty slot is emitted as a synthetic project.
    const slots = Math.min(6, ordered.length);
    ordered.forEach((project, index) => {
      if (ordered.length === 1) { centers.set(project.id, {x: 0, y: 0}); return; }
      const ring = 1 + Math.floor(index / 6), angle = -Math.PI / 2 + (index % 6) * TAU / slots;
      centers.set(project.id, {x: Math.cos(angle) * spacing * ring * 1.25, y: Math.sin(angle) * spacing * ring * .8});
    });
    const placeNodes = (ids, primaryFolder, x, y, radius) => {
      for (const id of ids) {
        const primary = primaries.get(id);
        if (!primary || keyOf([primary.id, ...primary.parts]) !== primaryFolder.key) continue;
        const radial = Math.sqrt((hash('radius:' + id) + .5) / 4294967296);
        const angle = (hash('angle:' + id) * GOLDEN) % TAU;
        const distance = radial * radius * .88;
        positions.set(id, {x: x + Math.cos(angle) * distance, y: y + Math.sin(angle) * distance,
          projectId: primary.id, folderKey: primaryFolder.key});
      }
    };
    const placeFolder = (folder, x, y, radius) => {
      folder.x = x; folder.y = y; folder.radius = radius;
      const children = [...folder.children.values()];
      if (!children.length) {placeNodes(folder.direct, folder, x, y, radius); return;}
      // A physical directory chain is metadata, not repeated geometric loss.
      if (children.length === 1 && !folder.direct.size) {
        placeFolder(children[0], x, y, radius); return;
      }
      const items = children.map(child => ({key: child.key, count: child.owned.size, folder: child}));
      if (folder.direct.size) items.push({key: folder.key + ':root-files', count: folder.direct.size, direct: true});
      for (const circle of pack(items, folder.key)) {
        const r = circle.radius * radius * .86, px = x + circle.x * radius * .86, py = y + circle.y * radius * .86;
        if (circle.item.direct) placeNodes(folder.direct, folder, px, py, r);
        else placeFolder(circle.item.folder, px, py, r);
      }
    };
    let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
    const resultProjects = [];
    for (const project of ordered) {
      const center = centers.get(project.id), radius = clamp(285 + Math.log2(1 + project.anchored.size) * 12, 300, 440) / Math.sqrt(density);
      placeFolder(project.root, center.x, center.y, radius);
      const folders = [...project.folderMap.values()].map(f => ({
        key: f.key, parentKey: f.parentKey, depth: f.depth, projectId: project.id,
        label: f.label, path: f.path, x: f.x, y: f.y, radius: f.radius,
        count: f.owned.size, nodeIds: [...f.owned].sort(compare), anchorNodeIds: [...f.anchored].sort(compare),
        memberScopeKeys: [...f.scopeKeys].sort(compare), sourceRoots: [...f.sourceRoots].sort(compare),
        childKeys: [...f.children.keys()].sort(compare), color: folderColor(project.id, f.path)
      })).sort((a, b) => compare(a.key, b.key));
      resultProjects.push({id: project.id, key: project.root.key, label: project.label, x: center.x, y: center.y, radius,
        color: color(project.id), folders, nodeIds: [...project.owned].sort(compare), anchorNodeIds: [...project.anchored].sort(compare),
        rootNodeIds: [...project.root.direct].sort(compare), count: project.owned.size});
      const margin = radius + 90;
      minX = Math.min(minX, center.x - margin); maxX = Math.max(maxX, center.x + margin);
      minY = Math.min(minY, center.y - margin); maxY = Math.max(maxY, center.y + margin);
    }
    if (!resultProjects.length) {minX = minY = -100; maxX = maxY = 100;}
    return {positions, projects: resultProjects, bounds: {minX, minY, maxX, maxY},
      options: {separation, density}, nodeCount: positions.size, layout: 'deterministic-project-folder-disks-v1'};
  }

  global.VrgNuclei = Object.freeze({build, color, folderColor, register, hash});
})(typeof window === 'undefined' ? globalThis : window);
