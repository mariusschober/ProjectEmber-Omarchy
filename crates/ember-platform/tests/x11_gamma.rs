//! Real XRandR integration test. Runs only when an X display is available
//! (Xvfb in CI, or a real X11 session). Harmless otherwise.

use ember_core::{ColorCurve, GammaTable, VerificationLevel};
use ember_platform::backend::DisplayBackend;
use ember_platform::x11::X11Backend;

#[test]
fn x11_gamma_readback_round_trip() {
    if std::env::var_os("DISPLAY").is_none() {
        eprintln!("skipping X11 test: DISPLAY is not set");
        return;
    }
    let mut backend = match X11Backend::connect() {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("skipping X11 test: {error}");
            return;
        }
    };
    assert_eq!(
        backend.capabilities().verification,
        VerificationLevel::Readback
    );
    assert!(backend.capabilities().readback);

    let displays = backend.enumerate().expect("enumerate");
    let Some(display) = displays.iter().find(|display| display.gamma_size >= 2) else {
        eprintln!("skipping X11 test: no gamma-capable output");
        backend.shutdown();
        return;
    };
    let identity = display.identity.clone();
    let baseline = backend.capture_baseline(&identity).expect("baseline");
    assert_eq!(baseline.gamma_source, ember_core::GammaSource::Readback);
    assert!(baseline.gamma_table.is_valid());

    let table = baseline
        .gamma_table
        .applying(ColorCurve::gains_for_warmth(0.71), 0.62);
    let report = backend.apply(&identity, &table).expect("apply");
    assert!(report.accepted);
    assert!(report.verified, "XRandR must read the write back");
    assert_eq!(
        backend.verify(&identity, &table),
        ember_platform::VerifyReport::Verified
    );

    backend.restore(&identity, &baseline).expect("restore");
    assert_eq!(
        backend.verify(&identity, &baseline.gamma_table),
        ember_platform::VerifyReport::Verified
    );

    // Repeated transforms from the baseline never compound.
    let second = baseline
        .gamma_table
        .applying(ColorCurve::gains_for_warmth(0.3), 0.9);
    backend.apply(&identity, &second).expect("second apply");
    assert_eq!(
        backend.verify(&identity, &second),
        ember_platform::VerifyReport::Verified
    );
    backend
        .restore(&identity, &baseline)
        .expect("final restore");
    backend.shutdown();
    let _ = GammaTable::identity(2);
}
