#!/usr/bin/env python3
"""Bounded Linux inference checks; synthetic inputs, not tracking parity.

PYTHONPATH=.local/inference:src python3 tools/check_models.py
Requires optional numpy/OpenVINO. An independent NumPy evaluation of AlignPre
checks convolution, pooling, flattening and dense weight orientation end to end.
"""
import hashlib
import json
from pathlib import Path
import time
import xml.etree.ElementTree as ET
import numpy as np
import openvino as ov
from tobii_linux.legacy_ir import load_model

ROOT = Path(__file__).resolve().parents[1]


def alignpre_reference(x):
    """Independent evaluation of this exact sequential graph, not a generic IR parser."""
    directory = ROOT / 'models/tobii-et5/alignpre'
    tree = ET.parse(directory / 'model.xml').getroot()
    weights = (directory / 'model.bin').read_bytes()
    for layer in tree.findall('./layers/layer'):
        typ = layer.get('type')
        data = layer.find('data')
        a = data.attrib if data is not None else {}

        def blob(tag):
            node = layer.find('./blobs/' + tag)
            return np.frombuffer(weights, dtype='<f4', count=int(node.get('size')) // 4,
                                 offset=int(node.get('offset')))

        if typ == 'Input':
            continue
        if typ == 'Convolution':
            assert a['kernel'] == '3,3' and a['strides'] == a['dilations'] == '1,1'
            assert a['pads_begin'] == a['pads_end'] == '1,1' and a['group'] == '1'
            w = blob('weights').reshape(int(a['output']), x.shape[1], 3, 3)
            windows = np.lib.stride_tricks.sliding_window_view(np.pad(x, ((0,0),(0,0),(1,1),(1,1))), (3,3), axis=(2,3))
            x = np.einsum('nchwkl,ockl->nohw', windows, w, optimize=True)
            x += blob('biases').reshape(1,-1,1,1)
        elif typ == 'ReLU':
            x = np.maximum(x, np.float32(0))
        elif typ == 'Pooling':
            assert a['pool-method'] == 'max' and a['kernel'] == a['strides'] == '2,2'
            x = x.reshape(x.shape[0], x.shape[1], x.shape[2]//2, 2, x.shape[3]//2, 2).max(axis=(3,5))
        elif typ == 'Reshape':
            x = x.reshape(tuple(int(v) for v in a['dim'].split(',')))
        elif typ == 'FullyConnected':
            x = x @ blob('weights').reshape(int(a['out-size']), -1).T + blob('biases')
        elif typ == 'SoftMax':
            x = np.exp(x - x.max(axis=1, keepdims=True))
            x /= x.sum(axis=1, keepdims=True)
        else:
            raise ValueError(typ)
    return x


def main():
    report = {'openvino': ov.get_version(), 'numpy': np.__version__,
              'device': 'CPU', 'precision_hint': 'f32', 'cases': [],
              'limitation': 'Synthetic graph execution; no original Tobii inference comparison or tracking validation'}
    core = ov.Core()
    for name in ('alignpre', 'fr', 'ff', 'd1'):
        model = load_model(ROOT / 'models/tobii-et5' / name)
        compiled = core.compile_model(model, 'CPU', {'INFERENCE_PRECISION_HINT': 'f32', 'INFERENCE_NUM_THREADS': 2})
        shape = list(model.input(0).get_shape())
        rng = np.random.default_rng(1701)
        for label, x in [('zero', np.zeros(shape, dtype=np.float32)),
                         ('ramp', np.linspace(-1, 1, int(np.prod(shape)), dtype=np.float32).reshape(shape)),
                         ('noise', rng.uniform(-1, 1, shape).astype(np.float32))]:
            start = time.monotonic()
            y = compiled([x])[0].copy()
            seconds = time.monotonic() - start
            if not np.isfinite(y).all():
                raise ValueError(f'Non-finite output: {name}/{label}')
            case = {'model': name, 'input': label, 'input_sha256': hashlib.sha256(x.tobytes()).hexdigest(),
                    'output_shape': list(y.shape), 'all_finite': True, 'seconds': seconds,
                    'output_min': float(y.min()), 'output_max': float(y.max())}
            if name == 'alignpre':
                ref = alignpre_reference(x)
                np.testing.assert_allclose(y, ref, atol=2e-6, rtol=2e-4)
                case['numpy_reference_max_absolute_error'] = float(np.max(np.abs(y-ref)))
            report['cases'].append(case)
            print(f'{name}/{label}: passed', flush=True)
    destination = ROOT / 'models/tobii-et5/inference-validation.json'
    destination.write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
