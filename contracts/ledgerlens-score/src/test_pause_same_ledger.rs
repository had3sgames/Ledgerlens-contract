//! Issue #1395: the global pause and the per-pair breaker transitioning in the
//! same ledger. Soroban serialises transactions within a ledger, so "same
//! ledger" means every call below runs at one fixed timestamp/sequence, in
//! each possible order. Documented rule: see
//! `docs/critical-state-transition-matrix.md` ("Global pause x pair breaker").
//!
//! Precedence: the two flags are independent; a write is allowed only when
//! BOTH are clear. Global pause is evaluated first, so it determines the
//! *shape* of the failure (whole-call `ContractPaused` for a batch) while the
//! pair breaker only rejects its own entries. Neither flag's transition
//! changes the other, and the final state is order-independent.

#![cfg(test)]

use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger as _},
    Address, Env, Symbol, Vec,
};

use crate::{Error, LedgerLensScoreContract, LedgerLensScoreContractClient, ScoreSubmission};

const TS: u64 = 1_700_000_000;

struct Fx<'a> {
    env: Env,
    c: LedgerLensScoreContractClient<'a>,
    pair: Symbol,
    other: Symbol,
}

fn setup<'a>() -> Fx<'a> {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = TS);
    let id = env.register_contract(None, LedgerLensScoreContract);
    let c = LedgerLensScoreContractClient::new(&env, &id);
    c.initialize(&Address::generate(&env), &Address::generate(&env));
    Fx { env, c, pair: symbol_short!("XLM_USDC"), other: symbol_short!("XLM_BTC") }
}

impl<'a> Fx<'a> {
    fn none(&self) -> Vec<Address> {
        Vec::new(&self.env)
    }
    fn pause(&self) {
        self.c.pause(&self.none());
    }
    fn unpause(&self) {
        self.c.unpause(&self.none());
    }
    fn breaker(&self, on: bool) {
        self.c.set_pair_paused(&self.pair, &on);
    }
    fn submit(&self, w: &Address, pair: &Symbol) -> Result<(), Error> {
        match self.c.try_submit_score(&self.none(), w, pair, &50, &false, &false, &TS, &90, &1, &None) {
            Ok(Ok(())) => Ok(()),
            Err(Ok(e)) => Err(e),
            other => panic!("unexpected host failure: {}", other.is_ok()),
        }
    }
    fn batch(&self, w: &Address) -> Result<(u32, u32, u32), Error> {
        let mk = |p: &Symbol| ScoreSubmission {
            wallet: w.clone(),
            asset_pair: p.clone(),
            score: 50,
            benford_flag: false,
            ml_flag: false,
            timestamp: TS,
            confidence: 90,
            model_version: 1,
        };
        let mut b = Vec::new(&self.env);
        b.push_back(mk(&self.pair));
        b.push_back(mk(&self.other));
        match self.c.try_submit_scores_batch(&b) {
            Ok(Ok(r)) => Ok((
                r.accepted_count,
                r.results.get(0).unwrap().rejection_code,
                r.results.get(1).unwrap().rejection_code,
            )),
            Err(Ok(e)) => Err(e),
            other => panic!("unexpected host failure: {}", other.is_ok()),
        }
    }
}

const PAUSED: u32 = Error::ContractPaused as u32;

/// Every ordering of the two transitions in one ledger lands in the same
/// state, and both flags are independently observable afterwards.
#[test]
fn transitions_commute_within_a_ledger() {
    for pause_first in [true, false] {
        let f = setup();
        if pause_first {
            f.pause();
            f.breaker(true);
        } else {
            f.breaker(true);
            f.pause();
        }
        assert!(f.c.is_paused());
        assert!(f.c.is_pair_paused(&f.pair));
        assert_eq!(f.c.get_paused_pairs().len(), 1);
    }
}

#[test]
fn single_submit_rejected_in_all_three_blocked_combinations() {
    let w = |f: &Fx| Address::generate(&f.env);
    // global only
    let f = setup();
    f.pause();
    assert_eq!(f.submit(&w(&f), &f.pair), Err(Error::ContractPaused));
    assert_eq!(f.submit(&w(&f), &f.other), Err(Error::ContractPaused));
    // pair only
    let f = setup();
    f.breaker(true);
    assert_eq!(f.submit(&w(&f), &f.pair), Err(Error::ContractPaused));
    assert_eq!(f.submit(&w(&f), &f.other), Ok(()));
    // both: identical outcome for either order of activation, any pair
    for pause_first in [true, false] {
        let f = setup();
        if pause_first {
            f.pause();
            f.breaker(true);
        } else {
            f.breaker(true);
            f.pause();
        }
        assert_eq!(f.submit(&w(&f), &f.pair), Err(Error::ContractPaused));
        assert_eq!(f.submit(&w(&f), &f.other), Err(Error::ContractPaused));
    }
}

/// Global pause dominates the *shape* of a batch failure; the pair breaker
/// alone only rejects its own entries.
#[test]
fn batch_precedence_global_pause_fails_whole_call_pair_breaker_fails_entry() {
    let f = setup();
    let w = Address::generate(&f.env);

    f.breaker(true);
    assert_eq!(f.batch(&w), Ok((1, PAUSED, 0)));

    f.pause(); // breaker already active, pause lands in the same ledger
    assert_eq!(f.batch(&w), Err(Error::ContractPaused));

    f.unpause(); // pause lifts; breaker must still bite (fresh wallet: no prior submit)
    let w2 = Address::generate(&f.env);
    assert_eq!(f.batch(&w2), Ok((1, PAUSED, 0)));
}

/// Lifting one flag never lifts the other.
#[test]
fn unpause_does_not_clear_breaker_and_breaker_clear_does_not_unpause() {
    let f = setup();
    let w = Address::generate(&f.env);
    f.pause();
    f.breaker(true);

    f.unpause();
    assert!(!f.c.is_paused());
    assert!(f.c.is_pair_paused(&f.pair));
    assert_eq!(f.submit(&w, &f.pair), Err(Error::ContractPaused));
    assert_eq!(f.submit(&w, &f.other), Ok(()));

    f.pause();
    f.breaker(false);
    assert!(f.c.is_paused());
    assert!(!f.c.is_pair_paused(&f.pair));
    assert_eq!(f.submit(&w, &f.pair), Err(Error::ContractPaused));

    f.unpause();
    assert_eq!(f.submit(&w, &f.pair), Ok(()));
}

/// Governance stays reachable while paused: the breaker can be toggled and the
/// contract unpaused during a global pause (no deadlock), and repeating a
/// transition in the same ledger is idempotent.
#[test]
fn governance_actions_remain_available_and_idempotent_while_paused() {
    let f = setup();
    f.pause();
    f.breaker(true);
    f.breaker(true);
    assert_eq!(f.c.get_paused_pairs().len(), 1);
    f.breaker(false);
    f.breaker(false);
    assert_eq!(f.c.get_paused_pairs().len(), 0);
    f.unpause();
    assert!(!f.c.is_paused());
}

/// Reads are never gated by either mechanism; the risk gate fails closed under
/// global pause (no failover configured) but ignores the pair breaker.
#[test]
fn reads_and_gate_in_the_transition_ledger() {
    let f = setup();
    let w = Address::generate(&f.env);
    assert_eq!(f.submit(&w, &f.pair), Ok(()));

    f.breaker(true);
    assert_eq!(f.c.get_score(&w, &f.pair).score, 50);
    assert!(f.c.query_risk_gate(&w, &f.pair, &100)); // breaker alone: gate evaluates

    f.pause(); // same ledger
    assert_eq!(f.c.get_score(&w, &f.pair).score, 50); // reads stay available
    assert!(!f.c.query_risk_gate(&w, &f.pair, &100)); // global pause: fail closed
    assert_eq!(f.c.get_score_history(&w, &f.pair).len(), 1);
}
