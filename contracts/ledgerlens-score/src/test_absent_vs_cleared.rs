//! Issue #1396: every public read entry point whose result could differ
//! between "wallet never submitted to this pair" and "wallet submitted, then
//! had its data cleared" is driven through four states and asserted against
//! the documented table in `docs/read-entry-points-absent-vs-cleared.md`.
//!
//! **Adding a read entry point?** Add a field to [`Observed`], fill it in
//! `observe`, add the expected value to each state in the tests below, and
//! add a row to the doc table. Deletion semantics themselves live in
//! `test_deletion_policy.rs` and `docs/privacy-model.md` (not repeated here).
//!
//! States: `Never` (no submission), `Live` (two submissions), `HistCleared`
//! (`clear_score_history` only), `ScoreCleared` (`clear_score` only), and
//! `BothCleared` (both).

#![cfg(test)]

use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger as _},
    Address, Env, Symbol, Vec,
};

use crate::{Error, LedgerLensScoreContract, LedgerLensScoreContractClient};

const START_TS: u64 = 1_700_000_000;
const GAP: u64 = 4_000; // > DEFAULT_COOLDOWN_SECS

#[derive(Debug, PartialEq)]
struct Observed {
    get_score: Result<u32, Error>,
    get_score_exists: bool,
    get_score_opt: Option<u32>,
    history_len: u32,
    score_count: u32,
    pair_list_len: u32,
    pair_count: u32,
    pair_score_count: u64,
    score_age: u64,
    variance: u64,
    provenance_present: bool,
    aggregate: Result<u32, Error>,
    risk_gate: bool,
    risk_cluster: Result<u32, Error>,
}

struct Fx<'a> {
    env: Env,
    c: LedgerLensScoreContractClient<'a>,
    wallet: Address,
    pair: Symbol,
}

fn setup<'a>() -> Fx<'a> {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = START_TS);
    let id = env.register_contract(None, LedgerLensScoreContract);
    let c = LedgerLensScoreContractClient::new(&env, &id);
    c.initialize(&Address::generate(&env), &Address::generate(&env));
    let wallet = Address::generate(&env);
    Fx { env, c, wallet, pair: symbol_short!("XLM_USDC") }
}

impl<'a> Fx<'a> {
    fn submit_two(&self) {
        let none = Vec::new(&self.env);
        self.c.submit_score(&none, &self.wallet, &self.pair, &40, &false, &false, &START_TS, &90, &1, &None);
        self.env.ledger().with_mut(|l| l.timestamp += GAP);
        self.c.submit_score(&none, &self.wallet, &self.pair, &50, &false, &false, &(START_TS + GAP), &90, &1, &None);
        self.env.ledger().with_mut(|l| l.timestamp += GAP);
    }

    fn observe(&self) -> Observed {
        let (c, w, p) = (&self.c, &self.wallet, &self.pair);
        let flat = |r: Result<Result<u32, _>, Result<Error, _>>| match r {
            Ok(Ok(v)) => Ok(v),
            Err(Ok(e)) => Err(e),
            other => panic!("unexpected host failure: {:?}", other.is_ok()),
        };
        Observed {
            get_score: flat(c.try_get_score(w, p).map(|r| r.map(|s| s.score))),
            get_score_exists: c.get_score_exists(w, p),
            get_score_opt: c.get_score_opt(w, p).map(|s| s.score),
            history_len: c.get_score_history(w, p).len(),
            score_count: c.get_score_count(w, p),
            pair_list_len: c.get_wallet_pair_list(w).len(),
            pair_count: c.get_wallet_pair_count(w),
            pair_score_count: c.get_pair_score_count(p),
            score_age: c.get_score_age(w, p),
            variance: c.get_score_variance(w, p),
            provenance_present: c.try_get_submission_provenance(w, p).is_ok_and(|r| r.is_ok()),
            aggregate: flat(c.try_get_aggregate_score(w).map(|r| r.map(|a| a.aggregate_score))),
            risk_gate: c.query_risk_gate(w, p, &100),
            risk_cluster: flat(c.try_assign_risk_cluster(w, p)),
        }
    }
}

fn never() -> Observed {
    Observed {
        get_score: Err(Error::ScoreNotFound),
        get_score_exists: false,
        get_score_opt: None,
        history_len: 0,
        score_count: 0,
        pair_list_len: 0,
        pair_count: 0,
        pair_score_count: 0,
        score_age: 0,
        variance: 0,
        provenance_present: false,
        aggregate: Err(Error::ScoreNotFound),
        risk_gate: false,
        risk_cluster: Err(Error::ScoreNotFound),
    }
}

fn live() -> Observed {
    Observed {
        get_score: Ok(50),
        get_score_exists: true,
        get_score_opt: Some(50),
        history_len: 2,
        score_count: 2,
        pair_list_len: 1,
        pair_count: 1,
        pair_score_count: 2,
        score_age: GAP,
        variance: 2_500,
        provenance_present: true,
        aggregate: Ok(50),
        risk_gate: true,
        risk_cluster: Ok(5),
    }
}

#[test]
fn never_submitted_is_the_documented_empty_state() {
    assert_eq!(setup().observe(), never());
}

#[test]
fn live_state_baseline() {
    let f = setup();
    f.submit_two();
    assert_eq!(f.observe(), live());
}

#[test]
fn history_cleared_keeps_the_latest_score_and_counters() {
    let f = setup();
    f.submit_two();
    f.c.clear_score_history(&Vec::new(&f.env), &f.wallet, &f.pair);
    assert_eq!(f.observe(), Observed { history_len: 0, variance: 0, ..live() });
}

#[test]
fn score_cleared_differs_from_never_only_in_lifetime_records() {
    let f = setup();
    f.submit_two();
    f.c.clear_score(&Vec::new(&f.env), &f.wallet, &f.pair);
    let got = f.observe();
    // Live-score reads are indistinguishable from "never submitted".
    assert_eq!(got.get_score, never().get_score);
    assert_eq!(got.get_score_exists, never().get_score_exists);
    assert_eq!(got.get_score_opt, never().get_score_opt);
    assert_eq!(got.pair_list_len, never().pair_list_len);
    assert_eq!(got.pair_count, never().pair_count);
    assert_eq!(got.aggregate, never().aggregate);
    assert_eq!(got.risk_gate, never().risk_gate);
    assert_eq!(got.risk_cluster, never().risk_cluster);
    // Lifetime records survive: they are the only way to tell the states apart.
    assert_eq!(got.history_len, 2);
    assert_eq!(got.score_count, 2);
    assert_eq!(got.pair_score_count, 2);
    assert_eq!(got.score_age, GAP);
    assert!(got.provenance_present);
}

#[test]
fn both_cleared_leaves_only_monotonic_counters_and_provenance() {
    let f = setup();
    f.submit_two();
    let none = Vec::new(&f.env);
    f.c.clear_score_history(&none, &f.wallet, &f.pair);
    f.c.clear_score(&none, &f.wallet, &f.pair);
    let got = f.observe();
    assert_eq!(
        got,
        Observed {
            score_count: 2,
            pair_score_count: 2,
            score_age: GAP,
            provenance_present: true,
            ..never()
        }
    );
}

#[test]
fn clearing_is_idempotent_on_a_never_submitted_wallet() {
    let f = setup();
    let none = Vec::new(&f.env);
    f.c.clear_score_history(&none, &f.wallet, &f.pair);
    f.c.clear_score(&none, &f.wallet, &f.pair);
    assert_eq!(f.observe(), never());
}

#[test]
fn resubmission_after_clear_continues_the_lifetime_counter() {
    let f = setup();
    f.submit_two();
    let none = Vec::new(&f.env);
    f.c.clear_score_history(&none, &f.wallet, &f.pair);
    f.c.clear_score(&none, &f.wallet, &f.pair);
    f.c.submit_score(&none, &f.wallet, &f.pair, &70, &false, &false, &(START_TS + 2 * GAP), &90, &1, &None);
    let got = f.observe();
    assert_eq!(got.get_score, Ok(70));
    assert_eq!(got.score_count, 3);
    assert_eq!(got.history_len, 1);
    assert_eq!(got.pair_count, 1);
}
