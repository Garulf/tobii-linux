#!/usr/bin/env python3
"""Validate face assets and export decoded LBF coefficients without dependencies."""
from itertools import groupby
import json
from pathlib import Path
from tobii_linux.face_assets import read_mesh, read_fdp, read_landmark_ids, read_shapes, landmark_vertices
from tobii_linux.lbf import LBF_MODELS, decoded_lbf_tree, write_tree

ROOT = Path(__file__).resolve().parents[1]
MODELS = ROOT / 'models/tobii-et5'
SUPPORT = MODELS / 'support'


def main():
    mesh = read_mesh(SUPPORT / 'jk_300.wfm')
    ids = read_landmark_ids(SUPPORT / 'bdtsdata/NN/vnn_nir/landmarks.bin')
    vertices = landmark_vertices(mesh, read_fdp(SUPPORT / 'jk_300_rigid.fdp'), ids)
    report = {'mesh_vertices': len(mesh.vertices), 'mesh_triangles': len(mesh.triangles),
              'matched_landmarks': len(vertices), 'shapes': {}, 'lbf': {},
              'limitations': 'Static assets only; tree semantics and pose inference are not implemented; int8 conversion is checked only in bounded oracle cases.'}
    for name, dims in [('bdtsdata/NN/vnn_nir/init_shape.bin', 2), ('bdtsdata/NN/is.bin', 3)]:
        shape, _ = read_shapes(SUPPORT / name, dims)
        report['shapes'][name] = shape
    tree = decoded_lbf_tree(SUPPORT)
    write_tree(tree, MODELS)
    for name in LBF_MODELS:
        manifest = json.loads(tree[f'decoded-lbf/{name}/manifest.json'])
        report['lbf'][name] = {k: manifest[k] for k in ('source_sha256', 'source_bytes', 'forest_end', 'landmarks', 'trees')}
        report['lbf'][name]['stage_shapes'] = [[m['shape'] for m in stage] for _, stage in
                                               groupby(manifest['matrices'], key=lambda m: m['stage'])]
    report['lbf']['lv'] = json.loads(tree['decoded-lbf/lv/manifest.json'])
    (MODELS / 'face-assets-validation.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
