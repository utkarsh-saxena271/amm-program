# Mitigating downtime in DeFi applications

"Downtime" in a DeFi app is not one thing. This AMM has several layers, and each one fails differently, so each needs its own answer.

## 1. Where can it go down?

| Layer | Failure | Who can fix it |
|---|---|---|
| Solana network | Halt, congestion, dropped transactions | Nobody on our side |
| RPC provider | Outage, rate limits, stale data | Us (redundancy) |
| Frontend / hosting | Site down, DNS, bad deploy | Us |
| Indexer / analytics | Lag, crash, missed events | Us |
| Program itself | Bug, exploit, bad upgrade | Us (pause and patch) |
| Keys | Lost or compromised authority | Us (multisig, procedures) |

The goal is not zero downtime, which is impossible when the base chain can halt. The goal is that **user funds are never at risk during an outage, and recovery is fast and boring.**

## 2. On-chain (the program)

- **The program has no external dependencies.** The AMM prices from its own reserves, with no oracle and no off-chain keeper. So it is available whenever Solana is. This is a real advantage of the design, and it should be preserved when adding features.
- **Pause switch.** `lock` / `unlock` is a circuit breaker. If a bug or exploit is found, the authority freezes swaps, deposits, and withdrawals while a fix is prepared.
  - Weakness today: `authority` is a single key. Make it a multisig (e.g. Squads) so one lost or stolen key can neither freeze the pool forever nor be used to grief it.
  - Open question: should `withdraw` stay allowed while locked? Blocking exits during an emergency protects LPs from a live exploit, but it also traps honest LPs if the authority disappears. A common compromise is a time-limited lock that expires on its own.
- **Slippage bounds protect users from delays.** Every instruction carries `min_*` / `max_*`. If a transaction lands late, after the price has moved, it fails instead of executing at a bad price. This matters most during congestion, when landing times are unpredictable.
- **Upgradeability.** Keep the upgrade authority on a multisig, so a fix can ship without waiting on one person, and so no single compromise can replace the program. Consider a timelock for non-emergency upgrades so users can exit if they distrust a change.
- **Leave room to migrate.** Reserve spare bytes (or a `version` field) in `Config` so a future upgrade does not require a risky account migration while the pool is live.

## 3. Transaction delivery (client side)

- **Priority fees and compute budget.** Set them dynamically from recent fee data, so swaps do not stall when the network is busy.
- **Retry with a fresh blockhash.** A transaction expires after about 150 blocks. Rebroadcast while valid, and rebuild with a new blockhash after that, so the same user action is never submitted twice by accident. Check the signature status first.
- **Multiple submission paths.** Send through more than one RPC and, where it makes sense, a block-engine path such as Jito, so one provider's problem does not block users.
- **Simulate first.** Run `simulateTransaction` before asking the user to sign, to show clear errors (for example, "pool is locked") instead of a failed transaction and a wasted fee.

## 4. RPC and data

- **At least two RPC providers** behind a health-checked failover, with timeouts and exponential backoff. Do not depend on a free public endpoint in production.
- **Handle stale reads.** Pick a commitment level deliberately: `confirmed` for UI, `finalized` before anything irreversible is shown as done. Reject data older than a threshold rather than displaying it silently.
- **Do not make the indexer a single point of truth.** Prices and balances shown to users should be derivable from the chain directly (vault balances) if the indexer is down. Analytics can lag, but trading and withdrawing should not depend on them.

## 5. Frontend

- Static hosting behind a CDN, with a second deployment (another provider or IPFS) and a tested DNS switch.
- Keep a **plain, minimal fallback** that can only withdraw liquidity, so LPs can always exit even if the main app is broken. Also publish the program ID and instruction layout so power users and integrators can interact without the UI.
- Roll deploys back quickly. Feature-flag risky changes.

## 6. Detection and response

- **Monitoring**: alert on failed-transaction rates, RPC latency and error rate, vault balance anomalies (a sudden drop in `vault_x` / `vault_y`, or the constant-product `k` decreasing), and the `locked` flag changing.
- **Runbook**, written before it is needed: who can pause, who must approve an upgrade, how users are informed, and in what order things are restored.
- **Communication**: a status page and a social channel. During an incident, say what is affected, whether funds are safe, and when the next update will be.
- **Post-mortems** after every incident, blameless, with concrete follow-ups.

## 7. Testing for it

- Unit and integration tests for the failure paths: locked pool, slippage, wrong authority (already covered by this repo's LiteSVM tests).
- Run on devnet or a local validator with injected failures (dropped transactions, slow RPC) before mainnet.
- Rehearse the pause and recovery procedure, so the first time it runs is not during a real incident.

## Honest limits

If Solana itself halts, nothing in this list brings the app back. What it does is ensure that (a) funds are safe, since the program is self-contained and slippage-bounded, (b) users get accurate information quickly, and (c) the app resumes cleanly when the chain does.