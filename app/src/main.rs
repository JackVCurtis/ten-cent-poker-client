//! Ten-Cent Poker desktop / headless client.
//!
//! Subcommands:
//! - (no args)         : headless smoke test (wallet + net + crypto), works with no display.
//! - `host`            : create a table, print the `tcpoker://` URI, wait for a guest, and play
//!                       N hands with the built-in auto strategy, printing each hand result.
//! - `join <uri>`      : join a table by URI and play with the auto strategy.
//! - `--gui`           : (requires the `gui` feature) launch the egui desktop window.
//!
//! The `host` / `join` demo is what a human runs across two terminals / machines to play a
//! full networked hand. Every peer runs the identical replicated state machine in
//! `poker-protocol`, so all peers compute the same winners and chip deltas.

use poker_crypto::{g1_scalar_mul, scalar_field_bits};
use poker_net::sample_table_uri;
use poker_protocol::{
    run_guest, run_host, CallStationBot, GameReport, HostOptions,
};
use poker_wallet::{derive_address, DEV_MNEMONIC};

#[cfg(feature = "gui")]
mod gui_state;
#[cfg(feature = "gui")]
mod ui;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "gui")]
    if std::env::args().any(|a| a == "--gui") {
        return Ok(ui::run()?);
    }

    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("host") => run_cli_host(&args),
        Some("join") => run_cli_join(&args),
        Some("netcheck") => run_netcheck(),
        _ => smoke(),
    }
}

/// `poker netcheck` — probe whether this machine can host for REMOTE players: stand up a node for
/// ~10s and report what the real game's UPnP-IGD + interface discovery finds. Prints whether UPnP
/// mapped an external address, the discovered listen addresses, and an overall remote-hosting verdict.
fn run_netcheck() -> Result<(), Box<dyn std::error::Error>> {
    use poker_net::{
        classify_multiaddr, decode_table_uri, host, is_internet_routable, Node, NodeEvent,
    };
    use tokio::time::{timeout_at, Duration, Instant};

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        println!("netcheck: probing reachability (UPnP + network interfaces) for ~10s…\n");
        let Node { handle, mut events } = host(None)?;
        let deadline = Instant::now() + Duration::from_secs(10);

        // Some(Some(addr)) = UPnP mapped; Some(None) = UPnP reported unavailable; None = no response.
        let mut upnp: Option<Option<String>> = None;
        let mut best_uri: Option<String> = None;
        let mut warning: Option<String> = None;
        let mut listen: Vec<String> = Vec::new();

        loop {
            match timeout_at(deadline, events.recv()).await {
                Err(_) | Ok(None) => break, // 10s elapsed, or node stopped
                Ok(Some(ev)) => match ev {
                    NodeEvent::NewListenAddr(a) => {
                        let cls = classify_multiaddr(&a);
                        listen.push(format!("  {a}  [{cls:?}]"));
                    }
                    NodeEvent::UpnpExternalAddr(a) => upnp = Some(Some(a.to_string())),
                    NodeEvent::UpnpUnavailable => {
                        if upnp.is_none() {
                            upnp = Some(None);
                        }
                    }
                    NodeEvent::TableUriReady(u) => best_uri = Some(u),
                    NodeEvent::ReachabilityWarning(r) => warning = Some(format!("{r:?}")),
                    _ => {}
                },
            }
        }
        handle.shutdown().await;

        println!("listen addresses:");
        if listen.is_empty() {
            println!("  (none discovered)");
        }
        for l in &listen {
            println!("{l}");
        }
        println!();

        match &upnp {
            Some(Some(addr)) => println!("UPnP : ✅ available — mapped external address {addr}"),
            Some(None) => {
                println!("UPnP : ❌ unavailable (no IGD gateway, or the gateway is non-routable/CGNAT)")
            }
            None => println!("UPnP : ❔ no response within 10s (likely no UPnP-IGD gateway on this network)"),
        }

        let upnp_ok = matches!(upnp, Some(Some(_)));
        let routable_uri = best_uri
            .as_deref()
            .and_then(|u| decode_table_uri(u).ok())
            .map(|addrs| addrs.iter().any(is_internet_routable))
            .unwrap_or(false);

        println!();
        if upnp_ok || routable_uri {
            println!("Remote hosting: ✅ you appear to have a routable address — you can host for remote players.");
            if let Some(u) = &best_uri {
                println!("  shareable URI: {u}");
            }
        } else {
            println!("Remote hosting: ❌ no routable address found.");
            if let Some(w) = &warning {
                println!("  reachability: {w}");
            }
            println!("  To host remotely, do ONE of:");
            println!("    • enable UPnP-IGD on your router, or");
            println!("    • set a fixed Listen port in the host lobby and forward that TCP+UDP port, or");
            println!("    • host from a machine with a public IP.");
            println!("  (Same-LAN play works regardless of the above.)");
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    })
}

/// Default number of hands to play in the headless demo (overridable via `host <n>`).
const DEMO_HANDS: u64 = 3;

/// `poker host [hands]` — create a table, print the URI, wait for a guest, play `hands` hands.
fn run_cli_host(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let hands = args.get(2).and_then(|s| s.parse::<u64>().ok()).unwrap_or(DEMO_HANDS);
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        println!("hosting a TRUSTLESS table — cards are dealt by the distributed");
        println!("Barnett-Smart mental-poker protocol; no peer (host included) can see another");
        println!("player's hole cards. Share the URI below with one other player:");
        let opts = HostOptions {
            hands,
            // Trustless deal (default). Set `mental: false` for the insecure placeholder.
            ..HostOptions::default()
        };
        let report = run_host(CallStationBot, opts, |uri| {
            println!("\n  tcpoker URI: {uri}\n");
            println!("(waiting for a guest to join...)");
        })
        .await?;
        print_report("HOST", &report);
        Ok::<_, Box<dyn std::error::Error>>(())
    })
}

/// `poker join <uri>` — join a table and play.
fn run_cli_join(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let uri = args.get(2).ok_or("usage: poker join <tcpoker://...>")?.clone();
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        println!("joining table {uri} ...");
        let report = run_guest(&uri, CallStationBot, None).await?;
        print_report("GUEST", &report);
        Ok::<_, Box<dyn std::error::Error>>(())
    })
}

/// Format a single card compactly, e.g. "As", "Td", "2c".
fn fmt_card(c: &poker_protocol::Card) -> String {
    format!("{:?}{:?}", c.rank, c.suit)
}

/// Print a human-readable summary of a completed game. For a TRUSTLESS hand each side prints
/// ITS OWN two hole cards (decrypted locally — no other peer can see them), the shared public
/// board, and the result (deltas + stacks) that every peer computed identically.
fn print_report(who: &str, report: &GameReport) {
    println!("\n===== {who}: game over — {} hand(s) played =====", report.hands.len());
    for h in report.hands.iter() {
        let o = &h.outcome;
        let board: Vec<String> = o.community.iter().map(fmt_card).collect();
        let hole = match &h.local_hole {
            Some([a, b]) => format!("{} {}", fmt_card(a), fmt_card(b)),
            None => "(folded/not dealt in)".to_string(),
        };
        println!(
            "hand #{:<3} button=seat{}  my hole=[{}]  board=[{}]  deltas={:?}  stacks={:?}",
            o.hand_no,
            o.button,
            hole,
            board.join(" "),
            o.deltas,
            o.final_stacks,
        );
    }
}

fn smoke() -> Result<(), Box<dyn std::error::Error>> {
    println!("┌─────────────────────────────────────────────┐");
    println!("│  Ten-Cent Poker — client smoke test          │");
    println!("└─────────────────────────────────────────────┘");

    // 1. Wallet — derive the dev address (account 0 of the canonical mnemonic).
    let addr = derive_address(DEV_MNEMONIC, 0)?;
    println!("wallet : account 0 address = {addr}");

    // 2. Net — produce a shareable, dialable table URI from a fresh peer identity.
    let uri = sample_table_uri();
    println!("net    : table URI         = {uri}");

    // 3. Crypto — exercise the BN254 proving stack used for ZK card dealing / settlement.
    let _point = g1_scalar_mul(42);
    println!("crypto : BN254 scalar field = {} bits (g·42 computed)", scalar_field_bits());

    println!("\nOK: wallet + net + crypto smoke passed.");
    println!("\nTo play across two terminals:");
    println!("  terminal A:  poker host");
    println!("  terminal B:  poker join <tcpoker://... printed by A>");
    Ok(())
}
