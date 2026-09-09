//! CLI entrypoint. Runs the multi-agent pipeline over a watchlist on Alpaca paper.
//!
//!   alpaca-agent --watchlist AAPL,MSFT        # ask before each order (default)
//!   alpaca-agent --watchlist AAPL,MSFT --auto # skip the human gate
//!
//! SCAFFOLD: wires the pieces; the agent bodies are `todo!()` until implemented.

use alpaca_agents::{alpaca::Alpaca, llm, pipeline, Config};
use std::io::Write;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (watchlist, auto) = parse_args();
    if watchlist.is_empty() {
        eprintln!("usage: alpaca-agent --watchlist AAPL,MSFT [--auto]");
        std::process::exit(2);
    }

    let mut cfg = Config::from_env()?;
    if auto {
        cfg.human_in_loop = false;
    }
    let alpaca = Alpaca::new(cfg.alpaca_key.clone(), cfg.alpaca_secret.clone(), cfg.alpaca_base.clone());
    let claude = llm::Claude::new(cfg.anthropic_key.clone(), llm::SONNET);

    for symbol in &watchlist {
        println!("\n=== {symbol} ===");
        let order = pipeline::run(&cfg, &alpaca, &claude, symbol, gate).await?;
        match order {
            Some(o) => println!("order placed: {} ({})", o.id, o.status),
            None => println!("no order"),
        }
    }
    Ok(())
}

/// Human-in-the-loop gate: print the decision, read y/N from stdin.
fn gate(d: &pipeline::Decision) -> bool {
    print!("{:?} {} x{} (score {:.2}) — {}\napprove? [y/N] ", d.action, d.symbol, d.qty, d.score, d.reason);
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).is_ok() && matches!(line.trim(), "y" | "Y")
}

/// Minimal arg parse: `--watchlist A,B,C` and `--auto`. clap can come later.
fn parse_args() -> (Vec<String>, bool) {
    let mut args = std::env::args().skip(1);
    let (mut watchlist, mut auto) = (Vec::new(), false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--watchlist" => {
                if let Some(v) = args.next() {
                    watchlist = v.split(',').map(|s| s.trim().to_uppercase()).filter(|s| !s.is_empty()).collect();
                }
            }
            "--auto" => auto = true,
            _ => {}
        }
    }
    (watchlist, auto)
}
