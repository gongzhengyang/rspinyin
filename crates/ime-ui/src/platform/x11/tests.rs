use super::*;

// The display-free half of this file's never-takes-focus policy is pinned by
// the source scans in `crates/ime-ui/tests/focus_policy.rs`: this file sits at
// its line budget, so those tests live beside the platform module's own suite.

#[test]
fn test_connect_with_unparsable_display_reports_unavailable() {
    // A display name without a colon cannot be parsed, so the call fails before any
    // socket is opened and no display server is needed to test it.
    let result = X11Backend::connect(320, 80, 1.0, Some("rspinyin-invalid"));
    assert!(matches!(result, Err(PlatformError::Unavailable)));
}

#[test]
#[ignore = "needs a live X server; the lab job runs it with DISPLAY set"]
fn test_x11_backend_adopts_the_scale_the_surface_synthesizes() {
    let mut backend =
        X11Backend::connect(320, 80, 1.0, None).expect("a live X server reachable through DISPLAY");
    // The surface hands the synthesized event in through the poll it already drives;
    // the backend adopts it and reports the ratio it actually runs at.
    let mut out = vec![SurfaceEvent::Scale { factor: 2.0 }];
    backend
        .poll_events(&mut out)
        .expect("polling does not fail");
    assert_eq!(
        backend.geometry(),
        (320, 80, 2.0),
        "the ratio is adopted and the logical canvas is the invariant it re-expresses"
    );
    assert!(
        out.contains(&SurfaceEvent::Scale { factor: 2.0 }),
        "the ratio actually adopted is reported back for the window to adopt"
    );
    assert_eq!(
        backend.apply_scale(2.0).expect("the request is delivered"),
        None,
        "a repeated report re-scales nothing"
    );
    // A configure at the adopted ratio back-computes the same canvas: the two cannot
    // drift.
    backend.apply_size(640, 160).expect("the size is adopted");
    assert_eq!(
        backend.geometry(),
        (320, 80, 2.0),
        "the canvas the echo reports is the one the adopted ratio describes"
    );
}
