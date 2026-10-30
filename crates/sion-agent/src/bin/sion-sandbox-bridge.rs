//! The bridge a skill's script runs behind inside the sandbox when its
//! skill names the hosts it may reach: `sion-sandbox-bridge <listen>
//! <socket> <program> [args...]` (see `sion_agent::egress`). The `sion`
//! command carries the same bridge as `sion sandbox-bridge`.

fn main() {
    std::process::exit(sion_agent::egress::bridge_cli(std::env::args().skip(1)));
}
