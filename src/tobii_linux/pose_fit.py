"""Recovered rigid fitting step, separate from the complete tracking policy."""
from .pose_projection import project_points,rigid_jacobian
from .pose_linalg import inverse_float32


def rigid_step(vertices, observations, state, focal_factor, model_scale=1.0,
               state_sensitivity=None, observation_sensitivity=None):
    """One regularized weighted update for six pose states.

The original update uses inverse sensitivity diagonals and an explicitly
inverted normal matrix. This finite positive-sensitivity implementation covers the observed point exclusion threshold but does
not cover near-zero sensitivity fallbacks or deformation states.
"""
    import numpy as np
    state=np.asarray(state,dtype=np.float32)
    vertices=np.asarray(vertices,dtype=np.float32)
    observed=np.asarray(observations,dtype=np.float32)
    if state.shape!=(6,) or observed.shape!=(len(vertices),2) or not np.isfinite(state).all() or not np.isfinite(observed).all():
        raise ValueError('Expected six finite state values and matching Nx2 observations')
    ss=np.ones(6,dtype=np.float32) if state_sensitivity is None else np.asarray(state_sensitivity,dtype=np.float32)
    os=np.ones(observed.size,dtype=np.float32) if observation_sensitivity is None else np.asarray(observation_sensitivity,dtype=np.float32)
    if ss.shape!=(6,) or os.shape!=(observed.size,) or not np.isfinite(ss).all() or not np.isfinite(os).all() or np.any(ss<=1e-8) or np.any(os<=1e-8) or np.any(os>=1e8):
        raise ValueError('Unsupported sensitivity extent/range')
    prediction,_=project_points(vertices,state[:3],state[3:],focal_factor,model_scale)
    jacobian=rigid_jacobian(vertices,state[:3],state[3:],focal_factor,model_scale)
    # Original Jacobian gates both rows using the X observation sensitivity.
    # A point at or above 100000 has zero Jacobian rows.
    jacobian[2*np.flatnonzero(os[::2]>=100000)]=0
    jacobian[2*np.flatnonzero(os[::2]>=100000)+1]=0
    weighted=jacobian.T*(np.float32(1)/os)
    normal=(weighted.astype(np.float64)@jacobian.astype(np.float64)+np.diag(np.float32(1)/ss)).astype(np.float32)
    inverse=inverse_float32(normal)
    gain=(inverse.astype(np.float64)@weighted.astype(np.float64)).astype(np.float32)
    residual=(observed-prediction).reshape(-1)
    return (gain.astype(np.float64)@residual.astype(np.float64)+state.astype(np.float64)).astype(np.float32)


def prepare_observations(normalized_points, landmark_ids, width, height):
    """Convert valid FDP landmarks to the fitting plane and observed weights.

Supports D1 landmark groups. Group 10/13 view-dependent weighting and missing
landmark retention require additional tracker state and are not implemented.
"""
    import numpy as np
    points=np.asarray(normalized_points,dtype=np.float32)
    ids=tuple(tuple(x) for x in landmark_ids)
    if (points.shape!=(len(ids),2) or any(len(x)!=2 for x in ids)
            or not 0<width<=32768 or not 0<height<=32768 or not np.isfinite(points).all()):
        raise ValueError('Expected finite normalized XY landmarks and image dimensions')
    if any(group in (10,13) for group,_ in ids):
        raise NotImplementedError('View-dependent group 10/13 weighting requires tracker state')
    aspect=np.array([np.float32(width)/np.float32(height) if width>height else 1,
                     np.float32(height)/np.float32(width) if height>width else 1],dtype=np.float32)
    observed=(np.float32(1)-(points+points))*aspect
    sensitivity=np.full((len(ids),2),np.float32(0.002),dtype=np.float32)
    for i,identifier in enumerate(ids):
        if identifier in ((2,1),(3,5),(3,6)):
            sensitivity[i]=np.float32(100001)
    return observed,sensitivity.reshape(-1)


def initial_rigid_state(model_points, image_points, width, height, focal_factor, neutral_yaw=3.1415927410125732):
    """Recover reset pose from four corresponding group-3 landmarks.

Both arrays are ordered IDs (3,7), (3,11), (3,8), (3,12). Model points are
Nx3 in the caller's already-prepared mesh space; image points are normalized
XY. This does not assume raw file mesh coordinates equal runtime model space.
"""
    import math
    import numpy as np
    model=np.asarray(model_points,dtype=np.float32)
    image=np.asarray(image_points,dtype=np.float32)
    if (model.shape!=(4,3) or image.shape!=(4,2) or not np.isfinite(model).all()
            or not np.isfinite(image).all() or not 0<width<=32768 or not 0<height<=32768
            or not math.isfinite(focal_factor) or focal_factor<=0 or not math.isfinite(neutral_yaw)):
        raise ValueError('Expected four finite model/image correspondences and positive camera geometry')
    aspect=np.float32(width)/np.float32(height)
    ax=aspect if aspect>1 else np.float32(1)
    ay=np.float32(2)/aspect if aspect<1 else np.float32(2)
    left_x=ax*(image[0,0]+image[1,0])-ax
    right_x=ax*(image[2,0]+image[3,0])-ax
    left_y=(np.float32(.5)-(image[0,1]+image[1,1])*np.float32(.5))*ay
    right_y=(np.float32(.5)-(image[2,1]+image[3,1])*np.float32(.5))*ay
    dx,dy=left_x-right_x,left_y-right_y
    image_distance=np.float32(np.sqrt(dx*dx+dy*dy))
    delta=(model[0,:2]+model[1,:2])*np.float32(.5)-(model[2,:2]+model[3,:2])*np.float32(.5)
    model_distance=np.float32(np.sqrt(delta[0]*delta[0]+delta[1]*delta[1]))
    if image_distance==0 or model_distance==0:
        raise ValueError('Degenerate reset landmark geometry')
    scale=model_distance/image_distance
    return np.array([0,np.float32(neutral_yaw),0,
        (left_x+right_x)*scale*np.float32(-.5),
        (right_y+left_y)*scale*np.float32(.5),scale*np.float32(focal_factor)],dtype=np.float32)


def deformation_step(vertices, observations, state, focal_factor, model_scale,
                     shape_basis, shape_ranges, animation_basis, animation_ranges,
                     state_sensitivity=None, observation_sensitivity=None,
                     shape_mask=None, animation_mask=None):
    """One original-style update including bounded shape and animation states.

Latents are clamped before constructing the model, as in the original update.
The returned step may exceed that bound until the caller's subsequent decode.
Outer retry/validation policy is not supplied by this component.
"""
    import numpy as np
    from .pose_projection import model_jacobian
    state=np.asarray(state,dtype=np.float32).copy()
    ns,na=len(shape_ranges),len(animation_ranges)
    count=6+ns+na
    observed=np.asarray(observations,dtype=np.float32)
    if state.shape!=(count,) or observed.shape!=(len(vertices),2) or not np.isfinite(state).all() or not np.isfinite(observed).all():
        raise ValueError('Invalid full fitting state or observations')
    ss=np.ones(count,dtype=np.float32) if state_sensitivity is None else np.asarray(state_sensitivity,dtype=np.float32)
    os=np.ones(observed.size,dtype=np.float32) if observation_sensitivity is None else np.asarray(observation_sensitivity,dtype=np.float32)
    if ss.shape!=(count,) or os.shape!=(observed.size,) or not np.isfinite(ss).all() or not np.isfinite(os).all() or np.any(ss<=1e-8) or np.any(os<=1e-8) or np.any(os>=1e8):
        raise ValueError('Unsupported sensitivity extent/range')
    state[6:]=np.clip(state[6:],-3,3)
    jacobian,prediction=model_jacobian(vertices,state[:3],state[3:6],focal_factor,model_scale,
        shape_basis,state[6:6+ns],shape_ranges,animation_basis,state[6+ns:],animation_ranges,shape_mask,animation_mask)
    excluded=np.flatnonzero(os[::2]>=100000)
    jacobian[2*excluded]=0;jacobian[2*excluded+1]=0
    weighted=jacobian.T*(np.float32(1)/os)
    normal=(weighted.astype(np.float64)@jacobian.astype(np.float64)+np.diag(np.float32(1)/ss)).astype(np.float32)
    gain=(inverse_float32(normal).astype(np.float64)@weighted.astype(np.float64)).astype(np.float32)
    return (gain.astype(np.float64)@(observed-prediction).reshape(-1).astype(np.float64)+state.astype(np.float64)).astype(np.float32)


def mode_step(model, observations, rigid_state, shape_coefficients, animation_coefficients,
              fitting_mode, observation_sensitivity=None, active_state=None):
    """One flag-3 (pose/shape) or flag-5 (pose/animation) update.

Inactive deformation coefficients stay in physical coefficient space, exactly
as stored by the original model. The returned latents belong only to the active
state block and are clipped by the subsequent decode, as in the outer loop.
Pass active_state back for consecutive updates in the same mode; re-encoding
physical coefficients between those updates can introduce additional rounding.
"""
    import numpy as np
    from .pose_model import coefficient_latents
    from .pose_projection import model_jacobian, bounded_coefficients
    if fitting_mode not in (3,5):
        raise ValueError('Expected fitting flag 3 or 5')
    pose=np.asarray(rigid_state,dtype=np.float32)
    shape=np.asarray(shape_coefficients,dtype=np.float32).copy()
    animation=np.asarray(animation_coefficients,dtype=np.float32).copy()
    observed=np.asarray(observations,dtype=np.float32)
    ns,na=len(model.shape_ranges),len(model.animation_ranges)
    if (pose.shape!=(6,) or shape.shape!=(ns,) or animation.shape!=(na,)
            or observed.shape!=(len(model.vertices),2) or not all(np.isfinite(x).all() for x in (pose,shape,animation,observed))):
        raise ValueError('Invalid mode-specific state or observations')
    qshape=coefficient_latents(shape,model.shape_ranges)
    qanimation=coefficient_latents(animation,model.animation_ranges)
    active=np.r_[np.arange(6),np.arange(6,6+ns) if fitting_mode==3 else np.arange(6+ns,6+ns+na)]
    if fitting_mode==3:qshape=np.clip(qshape,-3,3)
    else:qanimation=np.clip(qanimation,-3,3)
    state=np.concatenate((pose,qshape,qanimation))[active]
    if active_state is not None:
        state=np.asarray(active_state,dtype=np.float32).copy()
        if state.shape!=(len(active),) or not np.isfinite(state).all():
            raise ValueError('Invalid retained active fitting state')
        state[6:]=np.clip(state[6:],-3,3)
        pose=state[:6]
        if fitting_mode==3:qshape=state[6:]
        else:qanimation=state[6:]
    jacobian,prediction=model_jacobian(model.vertices,pose[:3],pose[3:],model.focal_factor,1,
        model.shape_basis,qshape,model.shape_ranges,model.animation_basis,qanimation,model.animation_ranges,
        model.shape_mask,model.animation_mask,
        fixed_first_coefficients=shape if fitting_mode==5 else None,
        fixed_second_coefficients=animation if fitting_mode==3 else None)
    jacobian=jacobian[:,active]
    ss=model.state_sensitivity[active]
    os=np.ones(observed.size,dtype=np.float32) if observation_sensitivity is None else np.asarray(observation_sensitivity,dtype=np.float32)
    if os.shape!=(observed.size,) or not np.isfinite(os).all() or np.any(os<=1e-8) or np.any(os>=1e8):
        raise ValueError('Unsupported observation sensitivity')
    excluded=np.flatnonzero(os[::2]>=100000)
    jacobian[2*excluded]=0;jacobian[2*excluded+1]=0
    weighted=jacobian.T*(np.float32(1)/os)
    normal=(weighted.astype(np.float64)@jacobian.astype(np.float64)+np.diag(np.float32(1)/ss)).astype(np.float32)
    gain=(inverse_float32(normal).astype(np.float64)@weighted.astype(np.float64)).astype(np.float32)
    updated=(gain.astype(np.float64)@(observed-prediction).reshape(-1).astype(np.float64)+state.astype(np.float64)).astype(np.float32)
    if fitting_mode==3:shape,updated[6:]=bounded_coefficients(updated[6:],model.shape_ranges)
    else:animation,updated[6:]=bounded_coefficients(updated[6:],model.animation_ranges)
    return {'rigid_state':updated[:6],'shape_coefficients':shape,'animation_coefficients':animation,'active_state':updated}
