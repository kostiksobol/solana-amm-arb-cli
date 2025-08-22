# solana-amm-arb-cli — concise guide

## Install (global CLI)

```bash
# From the project root
cargo install --path .

# Ensure cargo bin dir is on PATH
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.bashrc && source ~/.bashrc
# (or add to ~/.zshrc if you use zsh)

# Alternative (system-wide)
cargo build --release
sudo cp target/release/solana-amm-arb-cli /usr/local/bin/
```

Now you can run `solana-amm-arb-cli` from anywhere.

---

## One-time configuration (interactive)

```bash
solana-amm-arb-cli config set-rpc-url        # checks node health
solana-amm-arb-cli config set-keypair        # validates keypair file
solana-amm-arb-cli config set-pools          # set PoolA/PoolB → detect mints, pick mint_in, set amount_in
solana-amm-arb-cli config set-amount-in      # re-pick mint_in and amount_in if needed
solana-amm-arb-cli config set-spread-threshold-bps
solana-amm-arb-cli config set-slippage-bps
solana-amm-arb-cli config set-priority-fee   # micro-lamports
solana-amm-arb-cli config set-simulate       # true/false

# Inspect / reset persisted state
solana-amm-arb-cli config show
solana-amm-arb-cli config reset-defaults
```

---

## Run (uses state; flags override)

```bash
solana-amm-arb-cli   --amount-in 0.01   --spread-threshold-bps 100   --slippage-bps 500   --priority-fee 150000   --simulate-only true
```

Supported flags:

- `--rpc-url <STRING>`
- `--keypair <PATH>`
- `--amount-in <DECIMAL>` (in `mint_in` units)
- `--spread-threshold-bps <U32>` (e.g., `100` = 1.00%)
- `--slippage-bps <U32>` (e.g., `500` = 5.00%)
- `--priority-fee <U64>` (micro-lamports)
- `--simulate-only <BOOL>` (`true` to only simulate, `false` to send)

Output on each run:
- Logs (control with `RUST_LOG=info|debug`)
- `./arbitrage_result.json` with full analysis, decision, and tx/simulation result

---

## State: location, shape, defaults

### Location

State file `state.json` is stored via `directories::ProjectDirs` with app id `com.yourorg.solana-amm-arb-cli`:

- **Linux:** `${XDG_STATE_HOME:-$HOME/.local/state}/solana-amm-arb-cli/state.json`  
  (falls back to `${XDG_CONFIG_HOME:-$HOME/.config}/solana-amm-arb-cli/state.json`)
- **macOS:** `~/Library/Application Support/solana-amm-arb-cli/state.json`
- **Windows:** `%APPDATA%\solana-amm-arb-cli\state.json`

### JSON shape

```json
{
  "pool_a": "string | null",
  "pool_b": "string | null",
  "mint_in": "string | null",
  "mint_out": "string | null",
  "amount_in": 0.0,
  "spread_threshold_bps": 0,
  "slippage_bps": 0,
  "priority_fee_microlamports": 0,
  "simulate_only": true,
  "rpc_url": "string | null",
  "keypair_path": "string | null"
}
```

### Shipped defaults (`default_state()`)

```json
{
  "pool_a": "4jgpwmuwaUrZgTvUjio8aBVNQJ6HcsF3YKAekpwwxTou",
  "pool_b": "7JuwJuNU88gurFnyWeiyGKbFmExMWcmRZntn9imEzdny",
  "mint_in": "So11111111111111111111111111111111111111112",
  "mint_out": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
  "amount_in": 0.00001,
  "spread_threshold_bps": 100,
  "slippage_bps": 500,
  "priority_fee_microlamports": 100000,
  "simulate_only": true,
  "rpc_url": "https://api.mainnet-beta.solana.com",
  "keypair_path": "/home/coolman/solana-amm-arb-cli/keypair.json"
}
```

You can modify these defaults in code (`default_state()`); users can override via interactive `config` or per-run flags.
