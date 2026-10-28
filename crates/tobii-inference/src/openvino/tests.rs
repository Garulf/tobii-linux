use crate::test_util::{models, runtime};

#[test]
fn compiles_and_runs_every_network() {
    let Some(models) = models() else { return };
    let Some(runtime) = runtime() else { return };
    assert!(!runtime.version().unwrap().is_empty());
    for (name, outputs) in [("alignpre", 9), ("fr", 256), ("ff", 25380), ("d1", 171)] {
        let network = runtime.load(&models.join(name)).unwrap();
        let size: i64 = network.input_shape().iter().product();
        let output = network.infer(&vec![0.0; size as usize]).unwrap();
        assert_eq!(output.len(), outputs, "{name}");
        assert!(output.iter().all(|v| v.is_finite()), "{name}");
    }
}
