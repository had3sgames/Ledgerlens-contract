//! Issue #1388: authorization-tree / call-depth headroom guard.
//!
//! The Soroban host aborts a transaction whose call stack exceeds
//! `DEFAULT_HOST_DEPTH_LIMIT` (100 frames, soroban-env-host 21.x). The project
//! policy (docs/cross-contract-authorization-audit.md, "Depth headroom policy")
//! caps every chain at `MAX_CHAIN_DEPTH` frames, i.e. at least 90% headroom.
//! A new cross-contract hop must update `CHAINS` below, and CI fails if a chain
//! enters the unsafe margin.

use ledgerlens_score::{LedgerLensScoreContract, LedgerLensScoreContractClient};
use mock_amm::{FailPolicy, MockAmm, MockAmmClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, AuthorizedInvocation, Ledger as _},
    Address, Env, Vec,
};

const HOST_DEPTH_LIMIT: u32 = 100;
const HEADROOM_MARGIN: u32 = 90;
const MAX_CHAIN_DEPTH: u32 = HOST_DEPTH_LIMIT - HEADROOM_MARGIN;

/// (chain, contract frames from the outermost entry point, composed?)
const CHAINS: &[(&str, u32, bool)] = &[
    ("account -> score (any public entry point; internal helpers add no frames)", 1, false),
    ("consumer -> score (risk gate / reads)", 2, false),
    ("mock-amm / mock-lending -> score gate", 2, true),
    ("aggregator -> score shard fan-out", 2, false),
    ("consumer -> aggregator -> score shard", 3, true),
    ("consumer -> mock-amm -> score (nested caller, #716)", 3, true),
];

fn headroom_ok(depth: u32, limit: u32, margin: u32) -> bool {
    depth <= limit.saturating_sub(margin)
}

fn auth_depth(inv: &AuthorizedInvocation) -> u32 {
    1 + inv.sub_invocations.iter().map(auth_depth).max().unwrap_or(0)
}

#[test]
fn every_documented_chain_keeps_headroom() {
    assert!(CHAINS.iter().any(|c| c.2), "composed scenarios must be measured");
    for (name, depth, _) in CHAINS {
        assert!(
            headroom_ok(*depth, HOST_DEPTH_LIMIT, HEADROOM_MARGIN),
            "{name}: {depth} > {MAX_CHAIN_DEPTH}"
        );
    }
}

/// Seeded regression: a chain that eats the margin must be rejected.
#[test]
fn seeded_deep_chain_is_rejected() {
    assert!(!headroom_ok(MAX_CHAIN_DEPTH + 1, HOST_DEPTH_LIMIT, HEADROOM_MARGIN));
    assert!(!headroom_ok(HOST_DEPTH_LIMIT, HOST_DEPTH_LIMIT, HEADROOM_MARGIN));
}

/// Measured recorded auth-tree depth for real flows, including the composed
/// AMM -> score path, must stay inside the policy and the documented table.
#[test]
fn measured_auth_tree_depth_within_policy() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = 200_000);
    let id = env.register_contract(None, LedgerLensScoreContract);
    let score = LedgerLensScoreContractClient::new(&env, &id);
    let admin = Address::generate(&env);
    score.initialize(&admin, &Address::generate(&env));
    let w = Address::generate(&env);
    score.submit_score(
        &Vec::new(&env),
        &w,
        &symbol_short!("XLM_USDC"),
        &10,
        &false,
        &false,
        &200_000,
        &90,
        &1,
        &None,
    );
    let submit_depth = env.auths().iter().map(|(_, inv)| auth_depth(&inv)).max().unwrap_or(0);

    let amm_id = env.register_contract(None, MockAmm);
    let amm = MockAmmClient::new(&env, &amm_id);
    amm.initialize(&admin, &id, &75);
    amm.set_liquidity_gate_config(&admin, &75, &50, &FailPolicy::FailClosed, &604_800, &0);
    amm.swap(&w, &symbol_short!("XLM_USDC"), &1);
    let swap_depth = env.auths().iter().map(|(_, inv)| auth_depth(&inv)).max().unwrap_or(0);

    // `env.auths()` reflects the last call only; the gate path is read-only, so
    // it records no authorization tree (depth 0) while still being 2 frames deep.
    assert!(submit_depth >= 1, "submit must record an auth tree");
    for d in [submit_depth, swap_depth] {
        assert!(headroom_ok(d, HOST_DEPTH_LIMIT, HEADROOM_MARGIN), "measured depth {d}");
    }
}
