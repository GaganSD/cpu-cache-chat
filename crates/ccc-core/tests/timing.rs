use std::time::Duration;

use ccc_core::{AppKind, ChannelMode, Role, RuntimeConfig};

#[test]
fn default_simulation_uses_short_software_deadlines() {
    let cfg = RuntimeConfig::new(Role::A, ChannelMode::Sim, AppKind::Chat);
    assert_eq!(cfg.ack_timeout, Duration::from_millis(250));
    assert_eq!(cfg.sync_timeout, Duration::from_secs(5));
    assert_eq!(cfg.response_budget(19), Duration::from_millis(250));
    assert_eq!(cfg.recv_poll(), Duration::from_millis(20));
    assert_eq!(cfg.idle_dwell(), Duration::from_millis(80));
    let physical = RuntimeConfig::new(Role::A, ChannelMode::Cpu, AppKind::Chat);
    assert_eq!(physical.ack_timeout, Duration::from_secs(45));
}

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
        // Send budget = airtime + margin, where the turn-around margin is
        // airtime/4 + 8 slots, plus one second of scheduling slack. The
        // formula has no size-independent floor.
        let margin = control / 4 + cfg.slot * 8;
        assert_eq!(
            cfg.send_budget(19),
            control + margin + Duration::from_secs(1)
        );
        assert!(cfg.send_budget(275) > max_frame);
        // Response budget = the response's own send budget + one turn-around.
        assert_eq!(
            cfg.response_budget(19),
            cfg.send_budget(19) + cfg.slot * 8 + Duration::from_secs(1)
        );
        assert!(cfg.response_budget(19) > control);
        // Failed-send drain = response budget + the interrupted frame's full
        // send budget, a separate bounded maximum frame lifetime.
        assert_eq!(
            cfg.failed_send_recovery_budget(275, 19),
            cfg.response_budget(19) + cfg.send_budget(275)
        );
        // The whole-operation startup deadline covers two full responses.
        assert_eq!(cfg.sync_listen_budget(19), cfg.response_budget(19) * 2);
    }
}

#[test]
fn default_physical_control_budgets_carry_no_small_frame_floor() {
    // Default physical config: 50ms slots, legacy 45s ack timeout.
    let cfg = RuntimeConfig::new(Role::A, ChannelMode::Cpu, AppKind::Chat);
    assert_eq!(cfg.slot, Duration::from_millis(50));
    assert_eq!(cfg.ack_timeout, Duration::from_secs(45));
    // Airtime of a 19-byte control frame is 403 slots = 20.15s.
    let airtime = cfg.physical_frame_duration(19);
    assert_eq!(airtime, Duration::from_millis(20_150));
    // Send budget = 20.15s + 20.15s/4 (5.0375s) + 0.4s (8 slots) + 1s = 26.5875s.
    let send = cfg.send_budget(19);
    assert_eq!(
        send,
        airtime + airtime / 4 + cfg.slot * 8 + Duration::from_secs(1)
    );
    assert_eq!(send, Duration::from_micros(26_587_500));
    // Response budget = 26.5875s + 0.4s + 1s = 27.9875s.
    let response = cfg.response_budget(19);
    assert_eq!(response, send + cfg.slot * 8 + Duration::from_secs(1));
    assert_eq!(response, Duration::from_micros(27_987_500));
    // The removed floor: control budgets sit below the legacy 45s default.
    assert!(send < cfg.ack_timeout);
    assert!(response < cfg.ack_timeout);
}

#[test]
fn physical_recv_poll_is_a_bounded_service_interval() {
    let mut cfg = RuntimeConfig::new(Role::A, ChannelMode::Cpu, AppKind::Chat);
    for (millis, want) in [(1, 100), (25, 100), (50, 200), (100, 200), (1000, 200)] {
        cfg.slot = Duration::from_millis(millis);
        assert_eq!(cfg.recv_poll(), Duration::from_millis(want));
    }
    cfg.slot = Duration::from_micros(50);
    assert_eq!(cfg.recv_poll(), Duration::from_millis(100));
}

#[test]
fn idle_dwell_is_conservative_on_physical_channels_only() {
    for mode in [ChannelMode::Cache, ChannelMode::Cpu] {
        let cfg = RuntimeConfig::new(Role::A, mode, AppKind::Chat);
        assert_eq!(cfg.idle_dwell(), Duration::from_secs(2));
    }
    let sim = RuntimeConfig::new(Role::A, ChannelMode::Sim, AppKind::Chat);
    assert_eq!(sim.idle_dwell(), Duration::from_millis(80));
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
    assert_eq!(cfg.recv_poll(), Duration::from_millis(200));
}
