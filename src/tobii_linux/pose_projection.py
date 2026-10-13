"""Recovered face-model projection primitive, not a pose estimator.

Coordinates remain in the fitting model's local convention. Camera calibration,
parameter optimization and public head-pose coordinate conversion are separate.
"""
import math


def project_points(vertices, rotation, translation, focal_factor, model_scale=1.0):
    """Project finite Nx3 model points with Ry(ry) @ Rx(rx) @ Rz(rz).

Angles are radians. Rotation/translation operate before perspective division;
no principal-point offset is added in this observed primitive. Returned values
are (projected Nx2, transformed Nx3). Inputs with zero depth are rejected.
"""
    import numpy as np
    points = np.asarray(vertices, dtype=np.float32)
    angles = np.asarray(rotation, dtype=np.float32)
    offset = np.asarray(translation, dtype=np.float32)
    if (points.ndim != 2 or points.shape[1] != 3 or angles.shape != (3,) or offset.shape != (3,)
            or not np.isfinite(points).all() or not np.isfinite(angles).all() or not np.isfinite(offset).all()
            or not math.isfinite(focal_factor) or not math.isfinite(model_scale)):
        raise ValueError('Expected finite Nx3 vertices and three rotation/translation values')
    sx, sy, sz = (np.float32(math.sin(float(a))) for a in angles)
    cx, cy, cz = (np.float32(math.cos(float(a))) for a in angles)
    x, y, z = points.T
    scale = np.float32(model_scale)
    # Keep the original float32 operation order rather than a reassociated GEMM.
    px = (y*(sy*sx*cz-cy*sz) + x*(sy*sx*sz+cz*cy) + z*(cx*sy))*scale+offset[0]
    py = ((y*(cz*cx)+x*(cx*sz))-z*sx)*scale+offset[1]
    pz = (y*(cy*sx*cz+sz*sy) + x*(cy*sx*sz-cz*sy) + z*cy*cx)*scale+offset[2]
    if np.any(pz == 0):
        raise ValueError('Zero projection depth')
    factor = (np.float32(1)/pz)*np.float32(focal_factor)
    projected = np.column_stack((factor*px, factor*py))
    transformed = np.column_stack((px,py,pz))
    if not np.isfinite(projected).all() or not np.isfinite(transformed).all():
        raise ValueError('Non-finite projection result')
    return projected, transformed


def bounded_coefficients(latent, ranges):
    """Decode fitting latents via tanh(clamp(q,-3,3))*half_range+midpoint.

Returns both decoded coefficients and clamped latent state. This does not
assign a particular deformation block to animation/shape configuration.
"""
    import numpy as np
    latent = np.asarray(latent, dtype=np.float32)
    ranges = np.asarray(ranges, dtype=np.float32)
    if (latent.ndim != 1 or ranges.shape != (len(latent), 2)
            or not np.isfinite(latent).all() or not np.isfinite(ranges).all()
            or np.any(ranges[:, 1] <= ranges[:, 0])):
        raise ValueError('Expected finite latent vector and increasing bounds')
    clamped = np.clip(latent, np.float32(-3), np.float32(3))
    half = (ranges[:, 1]-ranges[:, 0])*np.float32(0.5)
    middle = (ranges[:, 1]+ranges[:, 0])*np.float32(0.5)
    values = np.asarray([math.tanh(float(x)) for x in clamped],dtype=np.float32)*half+middle
    return values,clamped


def deform_vertices(vertices, first_basis, first_coefficients, second_basis, second_coefficients):
    """Add two Nx3xK deformation bases in the observed first-then-second order."""
    import numpy as np
    vertices = np.asarray(vertices, dtype=np.float32)
    if vertices.ndim != 2 or vertices.shape[1] != 3 or not np.isfinite(vertices).all():
        raise ValueError('Expected finite Nx3 vertices')
    result = vertices
    for basis, coefficients in ((first_basis,first_coefficients),(second_basis,second_coefficients)):
        basis, coefficients = np.asarray(basis,dtype=np.float32),np.asarray(coefficients,dtype=np.float32)
        if (basis.shape != (*vertices.shape,len(coefficients)) or coefficients.ndim != 1
                or not np.isfinite(basis).all() or not np.isfinite(coefficients).all()):
            raise ValueError('Inconsistent/non-finite deformation basis')
        result = (basis.astype(np.float64) @ coefficients.astype(np.float64)
                  + result.astype(np.float64)).astype(np.float32)
    return result


def rigid_jacobian(vertices, rotation, translation, focal_factor, model_scale=1.0):
    """Analytic 2N×6 projection Jacobian for the six enabled rigid pose states.

Deformation-state columns and observation gating are not included here.
"""
    import numpy as np
    _,camera = project_points(vertices,rotation,translation,focal_factor,model_scale)
    points = np.asarray(vertices,dtype=np.float32)
    angles = np.asarray(rotation,dtype=np.float32)
    sx,sy,sz = (np.float32(math.sin(float(a))) for a in angles)
    cx,cy,cz = (np.float32(math.cos(float(a))) for a in angles)
    x,y,z = points.T
    px,py,pz = camera.T
    # Preserve the original scalar float32 derivative order; reassociation is
    # visible after inversion of the configured 69-state normal matrix.
    sy_sx,cy_sx = sy*sx,cy*sx
    r00=sy_sx*sz+cz*cy; r01=sy_sx*cz-cy*sz; r02=cx*sy
    r20=cy_sx*sz-cz*sy; r21=cy_sx*cz+sz*sy; r22=cy*cx
    inverse_depth=np.float32(1)/pz
    translation_factor=inverse_depth*np.float32(focal_factor)
    depth_factor=inverse_depth*translation_factor
    factor=depth_factor*np.float32(model_scale)
    dz_rx=(y*(r22*cz)+x*(r22*sz))-z*cy_sx
    dz_ry=(x*(-r00)-y*r01)-z*r02
    dz_rz=x*r21-y*r20
    result=np.zeros((len(points),2,6),dtype=np.float32)
    result[:,0,0]=(((y*(r02*cz)+x*(r02*sz))-z*sy_sx)*pz-dz_rx*px)*factor
    result[:,1,0]=(-((x*(sz*sx)+y*(cz*sx)+z*cx)*pz)-dz_rx*py)*factor
    result[:,0,1]=((y*r21+x*r20+z*r22)*pz-dz_ry*px)*factor
    result[:,1,1]=-(dz_ry*py*factor)
    result[:,0,2]=((y*(-r00)+x*r01)*pz-dz_rz*px)*factor
    result[:,1,2]=((x*(cz*cx)-y*(cx*sz))*pz-dz_rz*py)*factor
    result[:,0,3]=translation_factor
    result[:,1,4]=translation_factor
    result[:,0,5]=-(depth_factor*px)
    result[:,1,5]=-(depth_factor*py)
    return result.reshape(-1,6)


def model_jacobian(vertices, rotation, translation, focal_factor, model_scale,
                   first_basis, first_latent, first_ranges,
                   second_basis, second_latent, second_ranges,
                   first_mask=None, second_mask=None, *,
                   fixed_first_coefficients=None, fixed_second_coefficients=None):
    """Reproduce the original full model's update matrix and prediction.

The installed implementation transforms each basis component with
(1-tanh(b)^2)*tanh(b)*half_range. This is deliberately preserved; it is not
replaced with the usual derivative with respect to the current latent state.
"""
    import numpy as np
    first,second=np.asarray(first_basis,dtype=np.float32),np.asarray(second_basis,dtype=np.float32)
    c1,_=bounded_coefficients(first_latent,first_ranges);c2,_=bounded_coefficients(second_latent,second_ranges)
    if fixed_first_coefficients is not None:c1=np.asarray(fixed_first_coefficients,dtype=np.float32)
    if fixed_second_coefficients is not None:c2=np.asarray(fixed_second_coefficients,dtype=np.float32)
    deformed=deform_vertices(vertices,first,c1,second,c2)
    prediction,camera=project_points(deformed,rotation,translation,focal_factor,model_scale)
    rigid=rigid_jacobian(deformed,rotation,translation,focal_factor,model_scale)
    angles=np.asarray(rotation,dtype=np.float32)
    sx,sy,sz=(np.float32(math.sin(float(a))) for a in angles)
    cx,cy,cz=(np.float32(math.cos(float(a))) for a in angles)
    rotation_matrix=np.array([[sy*sx*sz+cz*cy,sy*sx*cz-cy*sz,cx*sy],
                              [cx*sz,cz*cx,-sx],
                              [cy*sx*sz-cz*sy,cy*sx*cz+sz*sy,cy*cx]],dtype=np.float32)
    blocks=[rigid]
    px,py,pz=camera.T
    inverse_depth=np.float32(1)/pz
    factor=(inverse_depth*(inverse_depth*np.float32(focal_factor)))*np.float32(model_scale)
    for basis,ranges,mask in ((first,first_ranges,first_mask),(second,second_ranges,second_mask)):
        ranges=np.asarray(ranges,dtype=np.float32)
        enabled=np.ones(basis.shape[2],dtype=bool) if mask is None else np.asarray(mask,dtype=bool)
        if enabled.shape!=(basis.shape[2],):raise ValueError('Deformation mask extent mismatch')
        half=(ranges[:,1]-ranges[:,0])*np.float32(.5)
        t=np.asarray([math.tanh(float(x)) for x in basis.flat],dtype=np.float32).reshape(basis.shape)
        derivative=((np.float32(1)-t*t)*t)*half
        derivative[:,:,~enabled]=0
        x,y,z=derivative[:,0,:],derivative[:,1,:],derivative[:,2,:]
        dz=y*rotation_matrix[2,1]+x*rotation_matrix[2,0]+z*rotation_matrix[2,2]
        dx=y*rotation_matrix[0,1]+x*rotation_matrix[0,0]+z*rotation_matrix[0,2]
        dy=y*rotation_matrix[1,1]+x*rotation_matrix[1,0]+z*rotation_matrix[1,2]
        block=np.stack(((dx*pz[:,None]-dz*px[:,None])*factor[:,None],
                        (dy*pz[:,None]-dz*py[:,None])*factor[:,None]),axis=1)
        blocks.append(block.reshape(len(vertices)*2,-1).astype(np.float32))
    return np.column_stack(blocks),prediction
