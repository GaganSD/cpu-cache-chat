use std::time::Duration;

#[test]
fn default_simulation_uses_short_software_deadlines() {
    let cfg = RuntimeConfig::new(Role::A, ChannelMode::Sim, AppKind::Chat);
    assert_eq!(cfg.ack_timeout, Duration::from_millis(250));
    assert_eq!(cfg.sync_timeout, Duration::from_secs(5));
    assert_eq!(cfg.response_budget(19), Duration::from_millis(250));
    let physical = RuntimeConfig::new(Role::A, ChannelMode::Cpu, AppKind::Chat);
    assert_eq!(physical.ack_timeout, Duration::from_secs(45));
}

use ccc_core::{AppKind, ChannelMode, Role, RuntimeConfig};

#[test]
fn physical_budgets_cover_complete_control_and_maximum_frames() {
    for millis in [10, 20, 50] {
        let mut cfg = RuntimeConfig::new(Role::A, ChannelMode::Cpu, AppKind::Chat);
        cfg.slot = Duration::from_millis(millis);
        cfg.ack_timeout = Duration::from_millis(1);
        cfg.sync_timeout = Duration::from_millis(1);
        // Encoded control is 19 bytes. A maximum protocol payload is 256 bytes.
        let control = cfg.physical_frame_duration(19);
        let max_frame = cfg.physical_frame_duration(19 + 256);
        assert_eq!(control, cfg.slot * 403);
        assert_eq!(max_frame, cfg.slot * 3731);
        assert!(cfg.send_budget(19) > control);
        assert!(cfg.send_budget(275) > max_frame);
        assert!(cfg.response_budget(19) > control);
        assert!(
            cfg.failed_send_recovery_budget(275, 19)
                >= cfg.send_budget(275) + cfg.response_budget(19)
        );
        assert!(cfg.sync_listen_budget(19) >= cfg.response_budget(19) * 2);
    }
}

#[test]
fn simulation_preserves_configured_timeouts_and_physical_math_saturates() {
    let mut cfg = RuntimeConfig::new(Role::B, ChannelMode::Sim, AppKind::Chat);
    cfg.ack_timeout = Duration::from_millis(17);
    cfg.sync_timeout = Duration::from_millis(37);
    assert_eq!(cfg.send_budget(275), cfg.ack_timeout);
    assert_eq!(cfg.response_budget(19), cfg.ack_timeout);
    assert_eq!(cfg.sync_listen_budget(19), cfg.sync_timeout);
    assert_eq!(cfg.failed_send_recovery_budget(275, 19), cfg.ack_timeout);
    cfg.channel = ChannelMode::Cpu;
    cfg.slot = Duration::MAX;
    assert_eq!(cfg.physical_frame_duration(usize::MAX), Duration::MAX);
    assert_eq!(cfg.send_budget(usize::MAX), Duration::MAX);
    assert_eq!(cfg.response_budget(19), Duration::MAX);
    assert_eq!(cfg.failed_send_recovery_budget(275, 19), Duration::MAX);
    assert_eq!(cfg.sync_listen_budget(19), Duration::MAX);
}
