use super::*;
use pgdt_io::{Allowance, Arrangement, ArrangementRequest};

/// **The account's total is the statistics' draw on the run's ledger**: its
/// first update replaces the `held` the arrangement opened the draw at —
/// what the cache handed the pass, which the account loads — and every later
/// one moves it, a charge retained or released included.
#[test]
fn the_account_draws_its_total_on_its_ledger() {
    let request =
        ArrangementRequest { statistics: 300, ..ArrangementRequest::recommended(1, None) };
    let arrangement = Arrangement::resolve(&Allowance::Stated(1 << 30), request);
    let ledger = arrangement.ledger();
    assert_eq!(ledger.statistics(), 300, "the draw opens at what is held");
    let allowance = arrangement.statistics_allowance();
    let account = Arc::new(StatisticsAccount::charging(allowance, ledger.clone()));
    account.apply(&[(Term::Loaded, 300)]);
    assert_eq!(ledger.statistics(), 300, "loaded, the same bytes");
    {
        let mut charge = Charge::new(Arc::clone(&account), Term::Gathering);
        charge.set(1000, 24);
        assert_eq!(ledger.statistics(), 1324);
        charge.retain(400);
        assert_eq!(ledger.statistics(), 700);
    }
    account.apply(&[(Term::Loaded, -300)]);
    assert_eq!(ledger.statistics(), 400, "a back-fill's replaced block leaves it");
    assert_eq!(ledger.drawn(), 400, "no line draws");
}
