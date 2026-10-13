#!/usr/bin/env python3
"""Validate face assets and export decoded LBF coefficients without dependencies."""
import hashlib
import json
from pathlib import Path
from tobii_linux.face_assets import read_mesh, read_fdp, read_landmark_ids, read_shapes, landmark_vertices
from tobii_linux.lbf import read_lbf, read_lv, quantize_matrix

ROOT = Path(__file__).resolve().parents[1]
SUPPORT = ROOT / 'models/tobii-et5/support'
OUT = ROOT / 'models/tobii-et5/decoded-lbf'


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
    for name in ('fa', 'fc'):
        path = SUPPORT / f'bdtsdata/NN/{name}.lbf'
        model = read_lbf(path)
        target = OUT / name
        target.mkdir(parents=True, exist_ok=True)
        manifest = {'source': str(path.relative_to(ROOT)), 'source_sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
                    'source_bytes': model['source_bytes'], 'forest_end': model['forest_end'],
                    'landmarks': len(model['points']), 'trees': model['tree_count'], 'matrices': [],
                    'layout': 'Little-endian float32 weights[input, output], biases[output]; decoded file values before runtime int8 quantization.'}
        for i, stage in enumerate(model['stages']):
            for j, matrix in enumerate(stage):
                item = {'stage': i, 'matrix': j, 'shape': matrix['shape']}
                for kind in ('weights', 'biases'):
                    filename = f'stage{i}-matrix{j}-{kind}.f32'
                    data = matrix[kind]
                    (target / filename).write_bytes(data)
                    item[kind] = {'file': filename, 'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest()}
                if j == 0:
                    qweights, scales = quantize_matrix(matrix)
                    for kind, data, suffix in [('quantized_weights', qweights, 'i8'), ('scales', scales, 'f32')]:
                        filename = f'stage{i}-matrix{j}-{kind}.{suffix}'
                        (target / filename).write_bytes(data)
                        item[kind] = {'file': filename, 'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest()}
                manifest['matrices'].append(item)
        (target / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
        report['lbf'][name] = {k: manifest[k] for k in ('source_sha256', 'source_bytes', 'forest_end', 'landmarks', 'trees')}
        report['lbf'][name]['stage_shapes'] = [[m['shape'] for m in stage] for stage in model['stages']]
    lv_path = SUPPORT / 'bdtsdata/LBF/lv'
    lv = read_lv(lv_path)
    target = OUT / 'lv'
    target.mkdir(parents=True, exist_ok=True)
    manifest = {'source': str(lv_path.relative_to(ROOT)),
                'source_sha256': hashlib.sha256(lv_path.read_bytes()).hexdigest(),
                'source_bytes': lv['source_bytes'], 'records': [],
                'layout': 'Little-endian float32 weights[input, output]; no stored biases.'}
    for i, record in enumerate(lv['records']):
        filename = f'record{i}-weights.f32'
        data = record['matrix']['weights']
        (target / filename).write_bytes(data)
        manifest['records'].append({'id': record['id'], 'shape': record['matrix']['shape'],
                                    'trees': len(record['forest'][2]), 'matrix_offset': record['matrix_offset'],
                                    'file': filename, 'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest()})
    (target / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    report['lbf']['lv'] = manifest
    (ROOT / 'models/tobii-et5/face-assets-validation.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
