"""Recovered fitting mesh centering and scaling, without device-unit claims."""
import math


def dense_deformations(mesh):
    """Expand plaintext sparse file units to Nx3xK arrays in file order."""
    import numpy as np
    result=[]
    for units in (mesh.animation_units,mesh.shape_units):
        basis=np.zeros((len(mesh.vertices),3,len(units)),dtype=np.float32)
        for k,(_,entries) in enumerate(units):
            for index,x,y,z in entries:basis[index,:,k]=(x,y,z)
        result.append(basis)
    return tuple(result)


def normalize_mesh(vertices, first_basis, second_basis, landmark_vertex_ids, target_distance=0.065):
    """Center four mapped landmarks and scale their XY midpoint separation.

Indices are ordered FDP (3,7), (3,11), (3,8), (3,12). The default 0.065 is
observed in the runtime constructors; public coordinate units are not inferred.
"""
    import numpy as np
    vertices=np.asarray(vertices,dtype=np.float32)
    first=np.asarray(first_basis,dtype=np.float32);second=np.asarray(second_basis,dtype=np.float32)
    ids=tuple(landmark_vertex_ids)
    if (vertices.ndim!=2 or vertices.shape[1]!=3 or len(ids)!=4
            or any(not isinstance(i,(int,np.integer)) or not 0<=i<len(vertices) for i in ids)
            or not math.isfinite(target_distance) or target_distance<=0 or not np.isfinite(vertices).all()):
        raise ValueError('Invalid model vertices, four indices or scale target')
    for basis in (first,second):
        if basis.ndim!=3 or basis.shape[:2]!=vertices.shape or not np.isfinite(basis).all():
            raise ValueError('Invalid deformation basis')
    a,b,c,d=vertices[list(ids)]
    left=(a[:2]+b[:2])*np.float32(.5);right=(c[:2]+d[:2])*np.float32(.5)
    delta=left-right
    distance=np.float32(np.sqrt(delta[0]*delta[0]+delta[1]*delta[1]))
    if distance==0:raise ValueError('Degenerate reference landmark distance')
    scale=np.float32(target_distance)/distance
    center=np.array([*(right+left)*np.float32(.5),
                     np.float32(np.float32(np.float32(c[2]+d[2])+a[2])+b[2])*np.float32(.25)],dtype=np.float32)
    return (vertices-center)*scale,first*scale,second*scale


def rescale_mesh(vertices, first_basis, second_basis, previous_distance, target_distance):
    """Apply the subsequent runtime distance change (0x140052bc0)."""
    import numpy as np
    if not all(math.isfinite(x) and x>0 for x in (previous_distance,target_distance)):
        raise ValueError('Distances must be finite and positive')
    factor=np.float32(target_distance)/np.float32(previous_distance)
    arrays=tuple(np.asarray(x,dtype=np.float32) for x in (vertices,first_basis,second_basis))
    if not all(np.isfinite(x).all() for x in arrays):raise ValueError('Non-finite model data')
    return tuple(x*factor for x in arrays)
