"""Load the researched IR4/IR6 graphs into native OpenVINO operations.

Optional dependencies: numpy and openvino. This performs graph inference only;
camera normalization, crop selection and pose fitting are not implemented here.
Unknown operations and incompatible tensor shapes fail rather than being guessed.
"""
from pathlib import Path
import hashlib
import json
import math
import xml.etree.ElementTree as ET


def load_model(directory):
    import numpy as np
    import openvino as ov
    from openvino import opset1 as op

    directory = Path(directory)
    manifest = json.loads((directory / 'manifest.json').read_text())
    data = {}
    for filename in ('model.xml', 'model.bin'):
        data[filename] = (directory / filename).read_bytes()
        if hashlib.sha256(data[filename]).hexdigest() != manifest['files'][filename]['sha256']:
            raise ValueError(f'Model hash mismatch: {filename}')
    tree = ET.fromstring(data['model.xml'])
    if tree.get('version') not in ('4', '6'):
        raise ValueError('Expected researched IR4/IR6 graph')
    layers = {n.get('id'): n for n in tree.findall('./layers/layer')}
    if len(layers) != len(tree.findall('./layers/layer')):
        raise ValueError('Duplicate layer IDs')
    edges = {}
    consumed = set()
    for edge in tree.findall('./edges/edge'):
        dest = (edge.get('to-layer'), int(edge.get('to-port')))
        src = (edge.get('from-layer'), int(edge.get('from-port')))
        if dest in edges:
            raise ValueError('Multiply connected input')
        edges[dest] = src
        consumed.add(src)
    values, parameters = {}, []
    pending = dict(layers)

    def shape(port):
        return [int(d.text) for d in port.findall('dim')]

    def ints(attrs, name, default=None):
        value = attrs.get(name, default)
        if value is None:
            raise ValueError(f'Missing attribute {name}')
        return [int(x) for x in value.split(',')]

    def blob(layer, tag, dims, dtype=np.dtype('<f4')):
        item = layer.find('./blobs/' + tag)
        if item is None:
            raise ValueError(f'Missing blob {layer.get("name")}/{tag}')
        offset, size = int(item.get('offset')), int(item.get('size'))
        if offset < 0 or size != math.prod(dims) * np.dtype(dtype).itemsize or offset+size > len(data['model.bin']):
            raise ValueError('Invalid blob extent')
        return np.frombuffer(data['model.bin'], dtype=dtype, count=math.prod(dims), offset=offset).reshape(dims).copy()

    while pending:
        progressed = False
        for lid, layer in list(pending.items()):
            inputs = sorted(layer.findall('./input/port'), key=lambda p: int(p.get('id')))
            sources = [edges.get((lid, int(p.get('id')))) for p in inputs]
            if any(s is None for s in sources):
                raise ValueError(f'Unconnected input in {lid}')
            if any(s not in values for s in sources):
                continue
            args = [values[s] for s in sources]
            for arg, port in zip(args, inputs):
                if list(arg.get_shape()) != shape(port):
                    raise ValueError(f'Input shape mismatch at {lid}')
            outputs = layer.findall('./output/port')
            if len(outputs) != 1:
                raise ValueError('Only single-output legacy layers are supported')
            dims = shape(outputs[0])
            kind = layer.get('type')
            element = layer.find('data')
            attrs = element.attrib if element is not None else {}
            if kind == 'Input':
                if layer.get('precision') != 'FP32':
                    raise ValueError('Expected FP32 input')
                node = op.parameter(dims, np.float32, name=layer.get('name'))
                parameters.append(node)
            elif kind == 'Const':
                dtype = {'I32': '<i4', 'I64': '<i8', 'FP32': '<f4'}.get(layer.get('precision'))
                if dtype is None:
                    raise ValueError('Unsupported constant precision')
                node = op.constant(blob(layer, 'custom', dims, np.dtype(dtype)))
            elif kind == 'Convolution':
                kernel = ints(attrs, 'kernel')
                group = int(attrs.get('group', '1'))
                channels = int(args[0].get_shape()[1])
                out_channels = int(attrs['output'])
                if channels % group or out_channels % group:
                    raise ValueError('Invalid convolution groups')
                weights = blob(layer, 'weights', [out_channels, channels // group, *kernel])
                kwargs = dict(strides=ints(attrs, 'strides'), pads_begin=ints(attrs, 'pads_begin'),
                              pads_end=ints(attrs, 'pads_end'), dilations=ints(attrs, 'dilations', '1,1'))
                if group == 1:
                    node = op.convolution(args[0], op.constant(weights), **kwargs)
                else:
                    weights = weights.reshape(group, out_channels // group, channels // group, *kernel)
                    node = op.group_convolution(args[0], op.constant(weights), **kwargs)
                if layer.find('./blobs/biases') is not None:
                    node = op.add(node, op.constant(blob(layer, 'biases', [1, out_channels, 1, 1])))
            elif kind == 'ReLU':
                slope = float(attrs.get('negative_slope', '0'))
                node = op.relu(args[0]) if slope == 0 else op.prelu(args[0], op.constant(np.float32(slope)))
            elif kind == 'PReLU':
                count = 1 if attrs.get('channel_shared') == '1' else int(args[0].get_shape()[1])
                node = op.prelu(args[0], op.constant(blob(layer, 'weights', [count])))
            elif kind == 'Pooling':
                kwargs = dict(strides=ints(attrs, 'strides'), pads_begin=ints(attrs, 'pads_begin'),
                              pads_end=ints(attrs, 'pads_end'), kernel_shape=ints(attrs, 'kernel'),
                              rounding_type=attrs.get('rounding_type', 'floor'))
                if attrs['pool-method'] == 'max':
                    node = op.max_pool(args[0], **kwargs)
                elif attrs['pool-method'] == 'avg':
                    node = op.avg_pool(args[0], exclude_pad=attrs.get('exclude-pad') == 'true', **kwargs)
                else:
                    raise ValueError('Unknown pooling method')
            elif kind == 'Reshape':
                pattern = args[1] if len(args) == 2 else op.constant(np.array(ints(attrs, 'dim'), dtype=np.int64))
                node = op.reshape(args[0], pattern, special_zero=True)
            elif kind == 'FullyConnected':
                out_size = int(float(attrs['out-size']))
                features = math.prod(list(args[0].get_shape())[1:])
                flattened = op.reshape(args[0], op.constant(np.array([0, -1], dtype=np.int64)), special_zero=True)
                node = op.matmul(flattened, op.constant(blob(layer, 'weights', [out_size, features])), False, True)
                if layer.find('./blobs/biases') is not None:
                    node = op.add(node, op.constant(blob(layer, 'biases', [out_size])))
            elif kind == 'SoftMax':
                node = op.softmax(args[0], int(attrs['axis']))
            elif kind == 'Permute':
                node = op.transpose(args[0], op.constant(np.array(ints(attrs, 'order'), dtype=np.int64)))
            elif kind == 'Concat':
                node = op.concat(args, int(attrs['axis']))
            elif kind == 'Eltwise' and attrs.get('operation') == 'sum':
                node = args[0]
                for arg in args[1:]:
                    node = op.add(node, arg)
            elif kind == 'Activation' and attrs.get('type') == 'sigmoid':
                node = op.sigmoid(args[0])
            else:
                raise ValueError(f'Unsupported operation: {kind} {attrs}')
            node.set_friendly_name(layer.get('name'))
            value = node.output(0)
            if list(value.get_shape()) != dims:
                raise ValueError(f'Output shape mismatch at {lid}: {list(value.get_shape())} != {dims}')
            values[(lid, int(outputs[0].get('id')))] = value
            del pending[lid]
            progressed = True
        if not progressed:
            raise ValueError('Cyclic graph or missing edge source')
    results = [op.result(value) for port, value in values.items() if port not in consumed]
    return ov.Model(results, parameters, tree.get('name'))
