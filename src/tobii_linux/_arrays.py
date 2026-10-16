"""float32 arrays to and from the little-endian bytes the native head-pose modules take."""


def floats(values):
    import numpy as np
    return np.ascontiguousarray(values, dtype=np.float32)


def raw(values):
    """C-order float32 bytes of an array-like."""
    return floats(values).tobytes()


def array(data, *shape):
    """A writable float32 array of the given shape from native bytes."""
    import numpy as np
    return np.frombuffer(data, dtype=np.float32).reshape(shape).copy()
