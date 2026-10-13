import importlib.util
import unittest


@unittest.skipUnless(importlib.util.find_spec('numpy'), 'Optional NumPy is not installed')
class PoseProjectionTests(unittest.TestCase):
    def test_neutral_perspective_and_scale(self):
        import numpy as np
        from tobii_linux.pose_projection import project_points
        projected, camera=project_points([[1,2,3]], [0,0,0], [0,0,4], 2, 2)
        np.testing.assert_allclose(camera,[[2,4,10]])
        np.testing.assert_allclose(projected,[[.4,.8]])

    def test_rotation_order_on_basis_vector(self):
        import numpy as np
        from tobii_linux.pose_projection import project_points
        _,camera=project_points([[1,0,0]], [np.pi/2,np.pi/2,np.pi/2], [0,0,4], 1)
        # Rz maps X→Y, Rx maps Y→Z, Ry maps Z→X.
        np.testing.assert_allclose(camera,[[1,0,4]],atol=1e-6)

    def test_clamp_preserves_bounded_coefficients(self):
        import numpy as np
        from tobii_linux.pose_projection import bounded_coefficients
        coefficients,latent=bounded_coefficients([-10,0,10],[[-2,4]]*3)
        np.testing.assert_array_equal(latent,[-3,0,3])
        self.assertEqual(float(coefficients[1]),1)
        self.assertTrue(np.all(coefficients>-2) and np.all(coefficients<4))

    def test_zero_depth_rejected(self):
        from tobii_linux.pose_projection import project_points
        with self.assertRaises(ValueError):project_points([[1,2,0]],[0,0,0],[0,0,0],1)

    def test_rigid_step_reduces_synthetic_reprojection_error(self):
        import numpy as np
        from tobii_linux.pose_projection import project_points
        from tobii_linux.pose_fit import rigid_step
        points=np.random.default_rng(57).uniform(-1,1,(12,3)).astype(np.float32)
        state=np.array([0,0,0,0,0,6],dtype=np.float32)
        observed,_=project_points(points,[.15,-.2,.1],[.1,-.2,5.5],1.7,.8)
        before,_=project_points(points,state[:3],state[3:],1.7,.8)
        updated=rigid_step(points,observed,state,1.7,.8)
        after,_=project_points(points,updated[:3],updated[3:],1.7,.8)
        self.assertLess(float(np.linalg.norm(after-observed)),float(np.linalg.norm(before-observed)))

    def test_rigid_step_rejects_unsupported_sensitivity(self):
        from tobii_linux.pose_fit import rigid_step
        with self.assertRaises(ValueError):
            rigid_step([[1,2,3]],[[0,0]],[0,0,0,0,0,4],1,state_sensitivity=[0]*6)

    def test_observations_flip_axes_and_preserve_aspect(self):
        import numpy as np
        from tobii_linux.pose_fit import prepare_observations
        observed,weights=prepare_observations([[.25,.75],[.5,.5]],[(4,1),(3,5)],640,480)
        np.testing.assert_allclose(observed,[[2/3,-.5],[0,0]],atol=1e-7)
        np.testing.assert_array_equal(weights,np.array([.002,.002,100001,100001],dtype=np.float32))

    def test_excluded_point_cannot_change_rigid_step(self):
        import numpy as np
        from tobii_linux.pose_fit import rigid_step
        state=np.array([0,0,0,0,0,6],dtype=np.float32)
        result=rigid_step([[1,2,3]],[[999,-999]],state,1,observation_sensitivity=[100001,.3])
        np.testing.assert_array_equal(result,state)
