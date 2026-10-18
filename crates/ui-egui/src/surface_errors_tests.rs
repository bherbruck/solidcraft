//! A surface the driver fails to reconfigure was a crash (#1935): on D3D12, `Surface::configure`
//! reported "Failed to wait for GPU to come idle before reconfiguring the Surface", and the
//! device's default uncaptured-error handler panicked on it. The vendored egui-wgpu configures
//! its surfaces under `capture_errors`, logs the error and tries again on the next frame. A
//! driver's refusal can't be provoked here, so these check the capture on a real device with an
//! error the validation always reports.

use egui_wgpu::{capture_errors, wgpu};

#[test]
fn an_error_under_capture_is_returned_not_a_panic() {
    let Some((_, device, _)) = crate::gpu::offscreen_device() else { return };
    // Mapped at creation with a size that isn't a multiple of 4: a validation error.
    let (_, error) = capture_errors(&device, || {
        device.create_buffer(&wgpu::BufferDescriptor { label: Some("bad"), size: 3, usage: wgpu::BufferUsages::MAP_WRITE, mapped_at_creation: true })
    });
    assert!(error.is_some_and(|e| matches!(e, wgpu::Error::Validation { .. })), "the validation error is captured");
    // The device goes on working, and a clean call reports nothing.
    let (buffer, error) = capture_errors(&device, || {
        device.create_buffer(&wgpu::BufferDescriptor { label: Some("good"), size: 4, usage: wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })
    });
    assert!(error.is_none(), "{error:?}");
    assert_eq!(buffer.size(), 4);
}

/// A surface that keeps refusing its configuration doesn't spin a frame loop: the next try waits
/// 16 ms, doubling with each refusal in a row up to 1 s, and every few refusals the surface is
/// recreated (asked in review of #1966).
#[test]
fn refused_configures_back_off_and_recreate_the_surface() {
    use egui_wgpu::{SURFACE_RECREATE_AFTER, configure_retry};
    use std::time::Duration;
    let delays: Vec<Duration> = (1..=12).map(|n| configure_retry(n).0).collect();
    assert_eq!(delays.first(), Some(&Duration::from_millis(16)));
    assert!(delays.windows(2).all(|w| w[0] <= w[1]), "{delays:?}");
    assert_eq!(delays.last(), Some(&Duration::from_secs(1)), "capped at a second");
    let recreate: Vec<u32> = (1..=12).filter(|n| configure_retry(*n).1).collect();
    assert_eq!(recreate, vec![SURFACE_RECREATE_AFTER, 2 * SURFACE_RECREATE_AFTER, 3 * SURFACE_RECREATE_AFTER, 4 * SURFACE_RECREATE_AFTER]);
    assert_eq!(configure_retry(0), (Duration::from_millis(16), false));
    assert_eq!(configure_retry(u32::MAX).0, Duration::from_secs(1));
}
