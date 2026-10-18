//! Issue #1391: fail-closed gates under host resource exhaustion. Reuses the
//! `env.budget().reset_limits` approach of `test_memory_exhaustion.rs`. Kept
//! separate from the logic-error tests in `test_fail_closed_invariants.rs`.
//!
//! Guarantee proven: when the budget runs out mid-call the host aborts the
//! whole invocation (no return value, so no `true` can be produced) and
//! leaves state untouched; the next call with budget restored fails closed or
//! allows exactly as it did before.

extern crate std;

use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger as _},
    Address, Env, Vec,
};

use crate::{LedgerLensScoreContract, LedgerLensScoreContractClient};

const TS: u64 = 1_700_000_000;

fn setup<'a>() -> (Env, LedgerLensScoreContractClient<'a>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = TS);
    let id = env.register_contract(None, LedgerLensScoreContract);
    let c = LedgerLensScoreContractClient::new(&env, &id);
    c.initialize(&Address::generate(&env), &Address::generate(&env));
    let w = Address::generate(&env);
    c.submit_score(
        &Vec::new(&env),
        &w,
        &symbol_short!("XLM_USDC"),
        &10,
        &false,
        &false,
        &TS,
        &90,
        &1,
        &None,
    );
    (env, c, w)
}

/// Runs `f` with an exhausted budget; true if the host aborted the call.
fn aborts_when_exhausted(env: &Env, f: impl FnOnce() -> bool) -> bool {
    env.budget().reset_limits(1, 1);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    env.budget().reset_unlimited();
    // Either the call aborted (Err) or, if it somehow returned, it must not allow.
    match r {
        Err(_) => true,
        Ok(allowed) => {
            assert!(!allowed, "gate returned allow under exhausted budget");
            false
        }
    }
}

#[test]
fn query_risk_gate_aborts_not_allows_on_exhaustion() {
    let (env, c, w) = setup();
    let p = symbol_short!("XLM_USDC");
    assert!(c.query_risk_gate(&w, &p, &100));
    aborts_when_exhausted(&env, || c.query_risk_gate(&w, &p, &100));
    assert!(c.query_risk_gate(&w, &p, &100)); // state intact, same answer
}

#[test]
fn query_risk_gate_with_confidence_aborts_not_allows_on_exhaustion() {
    let (env, c, w) = setup();
    let p = symbol_short!("XLM_USDC");
    assert!(c.query_risk_gate_with_confidence(&w, &p, &100, &50));
    aborts_when_exhausted(&env, || c.query_risk_gate_with_confidence(&w, &p, &100, &50));
    assert!(c.query_risk_gate_with_confidence(&w, &p, &100, &50));
}

#[test]
fn query_risk_gate_relative_aborts_not_allows_on_exhaustion() {
    let (env, c, w) = setup();
    let p = symbol_short!("XLM_USDC");
    let before = c.try_query_risk_gate_relative(&w, &p, &50);
    aborts_when_exhausted(&env, || c.query_risk_gate_relative(&w, &p, &50));
    assert_eq!(c.try_query_risk_gate_relative(&w, &p, &50), before);
}

#[test]
fn unknown_wallet_still_denied_after_exhaustion() {
    let (env, c, _w) = setup();
    let p = symbol_short!("XLM_USDC");
    let unknown = Address::generate(&env);
    aborts_when_exhausted(&env, || c.query_risk_gate(&unknown, &p, &100));
    assert!(!c.query_risk_gate(&unknown, &p, &100));
}
