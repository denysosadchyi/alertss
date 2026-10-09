#!/usr/bin/env python3
"""
Generate src/map_data.rs: simplified, land-only oblast outlines of Ukraine.

Source: Natural Earth 10m admin-1 states/provinces (public domain), which is clipped to the
coastline (no territorial waters). Only the Ukrainian features (iso_3166_2 UA-xx) are kept and
cached in tools/ne_ukraine.geojson so reruns do not re-download the ~40 MB file.

Topology-consistent simplification: every ring is split into "arcs" at the vertices where the set
of regions owning the edges changes (junctions). Each unique arc (a shared border is the same arc
seen from two regions, possibly reversed) is simplified ONCE with Douglas-Peucker with fixed
endpoints, then rings are reassembled. Neighbouring borders therefore coincide exactly.

Usage: python tools/gen_map.py [--tol 5.0] [--preview]   (--preview writes tools/preview_points.json)
"""
import argparse
import json
import math
import sys
import urllib.request
from pathlib import Path

URL = ('https://raw.githubusercontent.com/nvkelso/natural-earth-vector/master/'
       'geojson/ne_10m_admin_1_states_provinces.geojson')

VIEW_W = 4000
LAT_REF = 48.4

# iso_3166_2 -> exact name used by the alerts source
ISO_TO_NAME = {
    'UA-05': 'Вінницька область', 'UA-07': 'Волинська область',
    'UA-12': 'Дніпропетровська область', 'UA-14': 'Донецька область',
    'UA-18': 'Житомирська область', 'UA-21': 'Закарпатська область',
    'UA-23': 'Запорізька область', 'UA-26': 'Івано-Франківська область',
    'UA-32': 'Київська область', 'UA-35': 'Кіровоградська область',
    'UA-09': 'Луганська область', 'UA-46': 'Львівська область',
    'UA-48': 'Миколаївська область', 'UA-51': 'Одеська область',
    'UA-53': 'Полтавська область', 'UA-56': 'Рівненська область',
    'UA-59': 'Сумська область', 'UA-61': 'Тернопільська область',
    'UA-63': 'Харківська область', 'UA-65': 'Херсонська область',
    'UA-68': 'Хмельницька область', 'UA-71': 'Черкаська область',
    'UA-77': 'Чернівецька область', 'UA-74': 'Чернігівська область',
    'UA-30': 'м. Київ', 'UA-40': 'Севастополь', 'UA-43': 'Автономна Республіка Крим',
}
EXPECTED = set(ISO_TO_NAME.values())
ENCLAVES = ('м. Київ', 'Севастополь')


def load_subset(script_dir: Path) -> dict:
    sub = script_dir / 'ne_ukraine.geojson'
    if sub.exists():
        return json.loads(sub.read_text(encoding='utf-8'))
    print(f'Downloading {URL} (~40 MB)')
    with urllib.request.urlopen(URL) as r:
        full = json.loads(r.read().decode('utf-8'))
    feats = [f for f in full['features'] if f['properties'].get('iso_3166_2') in ISO_TO_NAME]
    for f in feats:
        p = f['properties']
        print('  found', p['iso_3166_2'], p['name'], p.get('adm0_a3'))
    sub.write_text(json.dumps({'type': 'FeatureCollection', 'features': feats}), encoding='utf-8')
    return {'features': feats}


def outer_rings(geom):
    c = geom['coordinates']
    polys = [c] if geom['type'] == 'Polygon' else c
    return [p[0][:-1] for p in polys if p and p[0]]  # open rings, holes dropped


def area(r):
    a = 0.0
    for i in range(len(r)):
        x1, y1 = r[i][:2]
        x2, y2 = r[(i + 1) % len(r)][:2]
        a += x1 * y2 - x2 * y1
    return abs(a) / 2


def dp_indices(pts, tol):
    """Douglas-Peucker on an open polyline; returns kept indices (endpoints always kept)."""
    keep = [False] * len(pts)
    keep[0] = keep[-1] = True
    stack = [(0, len(pts) - 1)]
    while stack:
        a, b = stack.pop()
        (ax, ay), (bx, by) = pts[a], pts[b]
        dx, dy = bx - ax, by - ay
        l2 = dx * dx + dy * dy
        dmax, idx = -1.0, -1
        for i in range(a + 1, b):
            px, py = pts[i]
            if l2 == 0:
                d = math.hypot(px - ax, py - ay)
            else:
                t = max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / l2))
                d = math.hypot(px - ax - t * dx, py - ay - t * dy)
            if d > dmax:
                dmax, idx = d, i
        if dmax > tol:
            keep[idx] = True
            stack += [(a, idx), (idx, b)]
    return [i for i, k in enumerate(keep) if k]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--tol', type=float, default=5.0, help='simplification tolerance in view units')
    ap.add_argument('--preview', action='store_true')
    args = ap.parse_args()

    script_dir = Path(__file__).parent
    gi = script_dir / '.gitignore'
    have = gi.read_text() if gi.exists() else ''
    add = [n for n in ('ne_ukraine.geojson', 'preview_points.json', '*.png', 'preview.svg') if n not in have]
    if add:
        with open(gi, 'a') as f:
            f.write(''.join(n + '\n' for n in add))

    data = load_subset(script_dir)

    # ---- collect kept rings (lon/lat quantized to 1e-5 so shared vertices are identical)
    q = lambda v: round(v * 1e5) / 1e5
    regions = {}
    for f in data['features']:
        iso = f['properties'].get('iso_3166_2')
        if iso not in ISO_TO_NAME:
            continue
        name = ISO_TO_NAME[iso]
        rings = [[(q(x), q(y)) for x, y in r] for r in outer_rings(f['geometry'])]
        rings.sort(key=area, reverse=True)
        big = area(rings[0])
        kept = [r for i, r in enumerate(rings)
                if i == 0 or (name not in ENCLAVES and area(r) >= big * 0.004)]
        for i, r in enumerate(rings):
            if i > 0 or len(rings) > 1:
                print(f'  {name}: ring {len(r)} pts rel.area {area(r) / big:.4f}',
                      'KEEP' if r in kept else 'drop')
        regions[name] = kept
    missing = EXPECTED - set(regions)
    if missing:
        sys.exit(f'Missing regions: {missing}')
    assert set(regions) == EXPECTED

    # ---- project and scale
    cosr = math.cos(math.radians(LAT_REF))
    allp = [(p[0] * cosr, -p[1]) for rs in regions.values() for r in rs for p in r]
    minx, maxx = min(p[0] for p in allp), max(p[0] for p in allp)
    miny, maxy = min(p[1] for p in allp), max(p[1] for p in allp)
    scale = VIEW_W / (maxx - minx)
    view_h = math.ceil((maxy - miny) * scale)
    cache = {}

    def P(p):
        if p not in cache:
            cache[p] = ((p[0] * cosr - minx) * scale, (-p[1] - miny) * scale)
        return cache[p]

    # ---- edge ownership: which regions use each undirected edge
    names = sorted(regions)
    owners = {}
    for name in names:
        for r in regions[name]:
            for i in range(len(r)):
                owners.setdefault(frozenset((r[i], r[(i + 1) % len(r)])), set()).add(name)

    simp = {}

    def simplify_arc(arc):
        key = tuple(arc)
        rkey = key[::-1]
        if rkey < key:
            return simplify_arc(list(rkey))[::-1]
        if key not in simp:
            idx = dp_indices([P(p) for p in key], args.tol)
            simp[key] = [key[i] for i in idx]
        return list(simp[key])

    out = {}
    for name in names:
        new_rings = []
        for r in regions[name]:
            n = len(r)
            own = [frozenset(owners[frozenset((r[i], r[(i + 1) % n]))]) for i in range(n)]
            breaks = [j for j in range(n) if own[j - 1] != own[j]]
            if not breaks:
                s = min(range(n), key=lambda i: r[i])
                r = r[s:] + r[:s]
                far = max(range(n), key=lambda i: math.dist(P(r[0]), P(r[i])))
                arcs = [r[:far + 1], r[far:] + [r[0]]]
            else:
                s = breaks[0]
                r = r[s:] + r[:s]
                own = own[s:] + own[:s]
                bs = [j for j in range(n) if own[j - 1] != own[j]] + [n]
                rc = r + r[:1]
                arcs = [rc[a:b + 1] for a, b in zip(bs, bs[1:])]
            pts = []
            for arc in arcs:
                sa = simplify_arc(arc)
                pts += sa if not pts else sa[1:]
            pts = pts[:-1]  # drop closing duplicate
            ip = []
            for p in pts:
                x, y = P(p)
                v = (round(x), round(y))
                if not ip or ip[-1] != v:
                    ip.append(v)
            if len(ip) > 1 and ip[0] == ip[-1]:
                ip.pop()
            if len(ip) >= 3:
                new_rings.append(ip + [ip[0]])
        out[name] = new_rings

    total = sum(len(r) for rs in out.values() for r in rs)
    print(f'VIEW_W={VIEW_W} VIEW_H={view_h} total points={total} tol={args.tol}')

    order = sorted((n for n in out if n not in ENCLAVES),
                   key=lambda n: -max(area(r) for r in out[n])) + list(ENCLAVES)
    for n in order:
        print(f'  {n}: {len(out[n])} rings, {sum(len(r) for r in out[n])} pts')

    if args.preview:
        (script_dir / 'preview_points.json').write_text(json.dumps(
            {'w': VIEW_W, 'h': view_h, 'regions': [{'name': n, 'rings': out[n]} for n in order]}),
            encoding='utf-8')

    lines = [
        '//! Simplified oblast outlines of Ukraine for the flyout map.',
        '//! GENERATED by tools/gen_map.py from Natural Earth 10m admin-1 (public domain, coastline-clipped):',
        '//! ' + URL,
        '//! Do not edit.',
        '//!',
        '//! Coordinates are in a local equirectangular projection, scaled so the whole country fits',
        '//! `0..VIEW_W` × `0..VIEW_H` (y grows downward, north is up). Shared borders were simplified',
        '//! once, so neighbouring regions coincide exactly.',
        '',
        'pub struct Region {',
        '    /// Name exactly as the alerts source spells it ("Вінницька область", "м. Київ",',
        '    /// "Севастополь"), or "Автономна Республіка Крим" which has no alert data.',
        "    pub name: &'static str,",
        "    /// Outer rings of the region's polygons (holes dropped), each a closed list of points.",
        "    pub rings: &'static [&'static [(i16, i16)]],",
        '}',
        '',
        f'pub const VIEW_W: f32 = {VIEW_W}.0;',
        f'pub const VIEW_H: f32 = {view_h}.0;',
        '',
        '/// Drawing order: large regions first, then small enclaves (м. Київ, Севастополь) on top.',
        '/// Hit-testing should iterate in reverse.',
        'pub static REGIONS: &[Region] = &[',
    ]
    for n in order:
        rs = ', '.join('&[' + ', '.join(f'({x}, {y})' for x, y in r) + ']' for r in out[n])
        lines.append(f'    Region {{ name: "{n}", rings: &[{rs}] }},')
    lines.append('];')
    path = script_dir.parent / 'src' / 'map_data.rs'
    path.write_text('\n'.join(lines) + '\n', encoding='utf-8')
    print('wrote', path)


if __name__ == '__main__':
    main()
