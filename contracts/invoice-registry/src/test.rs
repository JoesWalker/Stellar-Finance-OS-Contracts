// Tests may use plain arithmetic and unwrap; the workspace lints target
// contract code (README §5).
#![allow(clippy::arithmetic_side_effects, clippy::unwrap_used, clippy::panic)]

extern crate std;

use soroban_sdk::{
    testutils::{
        storage::Persistent as _, Address as _, Events as _, Ledger as _, MockAuth, MockAuthInvoke,
    },
    token::{StellarAssetClient, TokenClient},
    Address, BytesN, Env, Event as _, IntoVal, Val,
};

use crate::{
    events,
    types::{Error, InvoiceStatus},
    InvoiceRegistry, InvoiceRegistryClient,
};

/// Test fixture: a registry, a SAC token and a business account.
///
/// Auth is mocked explicitly per call (AGENTS.md rule 2), never with
/// `mock_all_auths`.
#[allow(dead_code)]
struct Setup<'a> {
    env: Env,
    registry_id: Address,
    registry: InvoiceRegistryClient<'a>,
    business: Address,
    token: TokenClient<'a>,
    token_admin: Address,
}

impl Setup<'_> {
    fn new() -> Self {
        let env = Env::default();
        let registry_id = env.register(InvoiceRegistry, ());
        let registry = InvoiceRegistryClient::new(&env, &registry_id);
        let token_admin = Address::generate(&env);
        let sac = env.register_stellar_asset_contract_v2(token_admin.clone());
        let token = TokenClient::new(&env, &sac.address());
        let business = Address::generate(&env);
        Setup {
            env,
            registry_id,
            registry,
            business,
            token,
            token_admin,
        }
    }

    fn hash(&self, n: u8) -> BytesN<32> {
        BytesN::from_array(&self.env, &[n; 32])
    }

    fn mint(&self, to: &Address, amount: i128) {
        let args: soroban_sdk::Vec<Val> = (to.clone(), amount).into_val(&self.env);
        auth(
            &self.env,
            &self.token_admin,
            &self.token.address,
            "mint",
            args,
            &[],
        );
        StellarAssetClient::new(&self.env, &self.token.address).mint(to, &amount);
    }

    /// A fresh payer holding `balance` tokens.
    fn payer(&self, balance: i128) -> Address {
        let p = Address::generate(&self.env);
        self.mint(&p, balance);
        p
    }

    fn try_pay(
        &self,
        signer: &Address,
        payer: &Address,
        business: &Address,
        id: &BytesN<32>,
        amount: i128,
    ) -> Result<(), Option<Error>> {
        let args = (payer.clone(), business.clone(), id.clone(), amount).into_val(&self.env);
        let sub = [MockAuthInvoke {
            contract: &self.token.address,
            fn_name: "transfer",
            args: (payer.clone(), business.clone(), amount).into_val(&self.env),
            sub_invokes: &[],
        }];
        auth(&self.env, signer, &self.registry_id, "pay", args, &sub);
        match self.registry.try_pay(payer, business, id, &amount) {
            Ok(Ok(())) => Ok(()),
            Err(Ok(e)) => Err(Some(e)),
            _ => Err(None),
        }
    }

    fn pay(&self, payer: &Address, id: &BytesN<32>, amount: i128) {
        self.try_pay(payer, payer, &self.business, id, amount)
            .expect("pay ok");
    }

    fn try_void(
        &self,
        signer: &Address,
        business: &Address,
        id: &BytesN<32>,
    ) -> Result<(), Option<Error>> {
        let args = (business.clone(), id.clone()).into_val(&self.env);
        auth(&self.env, signer, &self.registry_id, "void", args, &[]);
        match self.registry.try_void(business, id) {
            Ok(Ok(())) => Ok(()),
            Err(Ok(e)) => Err(Some(e)),
            _ => Err(None),
        }
    }

    fn try_close(
        &self,
        signer: &Address,
        business: &Address,
        id: &BytesN<32>,
    ) -> Result<(), Option<Error>> {
        let args = (business.clone(), id.clone()).into_val(&self.env);
        auth(&self.env, signer, &self.registry_id, "close", args, &[]);
        match self.registry.try_close(business, id) {
            Ok(Ok(())) => Ok(()),
            Err(Ok(e)) => Err(Some(e)),
            _ => Err(None),
        }
    }

    fn try_register(
        &self,
        signer: &Address,
        business: &Address,
        id: &BytesN<32>,
        token: &Address,
        amount: i128,
    ) -> Result<(), Option<Error>> {
        let content = self.hash(0xC0);
        let args = (
            business.clone(),
            id.clone(),
            token.clone(),
            amount,
            content.clone(),
        )
            .into_val(&self.env);
        auth(&self.env, signer, &self.registry_id, "register", args, &[]);
        match self
            .registry
            .try_register(business, id, token, &amount, &content)
        {
            Ok(Ok(())) => Ok(()),
            Err(Ok(e)) => Err(Some(e)),
            _ => Err(None),
        }
    }

    /// Register invoice `n` for `amount` from the fixture business.
    fn register(&self, n: u8, amount: i128) -> BytesN<32> {
        let id = self.hash(n);
        self.try_register(
            &self.business,
            &self.business,
            &id,
            &self.token.address,
            amount,
        )
        .expect("register ok");
        id
    }
}

/// Mock exactly one authorisation: `signer` authorises `contract.fn_name(args)`.
fn auth(
    env: &Env,
    signer: &Address,
    contract: &Address,
    fn_name: &str,
    args: soroban_sdk::Vec<Val>,
    sub_invokes: &[MockAuthInvoke],
) {
    env.mock_auths(&[MockAuth {
        address: signer,
        invoke: &MockAuthInvoke {
            contract,
            fn_name,
            args,
            sub_invokes,
        },
    }]);
}

#[test]
fn get_unknown_invoice_returns_not_found() {
    let s = Setup::new();
    let res = s.registry.try_get(&s.business, &s.hash(7));
    assert_eq!(res, Err(Ok(Error::InvoiceNotFound)));
}

// ---------------------------------------------------------------- register

#[test]
fn register_creates_open_invoice() {
    let s = Setup::new();
    let id = s.register(1, 500);
    let inv = s.registry.get(&s.business, &id);
    assert_eq!(inv.id, id);
    assert_eq!(inv.business, s.business);
    assert_eq!(inv.token, s.token.address);
    assert_eq!(inv.amount, 500);
    assert_eq!(inv.paid, 0);
    assert_eq!(inv.status, InvoiceStatus::Open);
    assert_eq!(inv.content_hash, s.hash(0xC0));
}

#[test]
fn register_emits_registered_event() {
    let s = Setup::new();
    let id = s.register(1, 500);
    let expected = events::Registered {
        id,
        business: s.business.clone(),
        token: s.token.address.clone(),
        amount: 500,
        content_hash: s.hash(0xC0),
    };
    assert_eq!(
        s.env
            .events()
            .all()
            .filter_by_contract(&s.registry_id)
            .events(),
        [expected.to_xdr(&s.env, &s.registry_id)]
    );
}

#[test]
fn register_rejects_duplicate_id() {
    let s = Setup::new();
    let id = s.register(1, 500);
    assert_eq!(
        s.try_register(&s.business, &s.business, &id, &s.token.address, 900),
        Err(Some(Error::InvoiceExists))
    );
    assert_eq!(s.registry.get(&s.business, &id).amount, 500);
}

#[test]
fn register_same_id_from_other_business_is_independent() {
    let s = Setup::new();
    let id = s.register(1, 500);
    let other = Address::generate(&s.env);
    // Same id under a different business is a different invoice.
    assert_eq!(
        s.try_register(&other, &other, &id, &s.token.address, 1),
        Ok(())
    );
    assert_eq!(s.registry.get(&s.business, &id).amount, 500);
    assert_eq!(s.registry.get(&other, &id).amount, 1);
}

#[test]
fn register_rejects_zero_amount() {
    let s = Setup::new();
    let id = s.hash(1);
    assert_eq!(
        s.try_register(&s.business, &s.business, &id, &s.token.address, 0),
        Err(Some(Error::InvalidAmount))
    );
}

#[test]
fn register_rejects_negative_amount() {
    let s = Setup::new();
    let id = s.hash(1);
    assert_eq!(
        s.try_register(&s.business, &s.business, &id, &s.token.address, -10),
        Err(Some(Error::InvalidAmount))
    );
}

#[test]
fn register_rejects_wrong_signer() {
    let s = Setup::new();
    let id = s.hash(1);
    let attacker = Address::generate(&s.env);
    assert_eq!(
        s.try_register(&attacker, &s.business, &id, &s.token.address, 10),
        Err(None)
    );
    assert_eq!(
        s.registry.try_get(&s.business, &id),
        Err(Ok(Error::InvoiceNotFound))
    );
}

#[test]
fn register_rejects_unknown_token() {
    let s = Setup::new();
    let id = s.hash(1);
    let not_a_token = Address::generate(&s.env);
    assert_eq!(
        s.try_register(&s.business, &s.business, &id, &not_a_token, 10),
        Err(Some(Error::InvalidToken))
    );
}

#[test]
fn register_rejects_non_token_contract() {
    let s = Setup::new();
    let id = s.hash(1);
    // A real contract that does not implement the token interface.
    let other_registry = s.env.register(InvoiceRegistry, ());
    assert_eq!(
        s.try_register(&s.business, &s.business, &id, &other_registry, 10),
        Err(Some(Error::InvalidToken))
    );
}

// ---------------------------------------------------------------- pay

#[test]
fn pay_in_full_transfers_and_marks_paid() {
    let s = Setup::new();
    let id = s.register(1, 500);
    let payer = s.payer(1_000);
    s.pay(&payer, &id, 500);

    assert_eq!(s.token.balance(&payer), 500);
    assert_eq!(s.token.balance(&s.business), 500);
    assert_eq!(s.token.balance(&s.registry_id), 0);
    let inv = s.registry.get(&s.business, &id);
    assert_eq!(inv.paid, 500);
    assert_eq!(inv.status, InvoiceStatus::Paid);
}

#[test]
fn pay_emits_paid_event() {
    let s = Setup::new();
    let id = s.register(1, 500);
    let payer = s.payer(1_000);
    s.pay(&payer, &id, 200);
    let expected = events::Paid {
        id,
        business: s.business.clone(),
        payer,
        amount: 200,
        total_paid: 200,
    };
    assert_eq!(
        s.env
            .events()
            .all()
            .filter_by_contract(&s.registry_id)
            .events(),
        [expected.to_xdr(&s.env, &s.registry_id)]
    );
}

#[test]
fn partial_payments_accumulate_until_paid() {
    let s = Setup::new();
    let id = s.register(1, 500);
    let payer = s.payer(1_000);

    s.pay(&payer, &id, 200);
    let inv = s.registry.get(&s.business, &id);
    assert_eq!((inv.paid, inv.status), (200, InvoiceStatus::Open));

    s.pay(&payer, &id, 250);
    let inv = s.registry.get(&s.business, &id);
    assert_eq!((inv.paid, inv.status), (450, InvoiceStatus::Open));

    s.pay(&payer, &id, 50);
    let expected = events::Paid {
        id: id.clone(),
        business: s.business.clone(),
        payer: payer.clone(),
        amount: 50,
        total_paid: 500,
    };
    assert_eq!(
        s.env
            .events()
            .all()
            .filter_by_contract(&s.registry_id)
            .events(),
        [expected.to_xdr(&s.env, &s.registry_id)]
    );
    let inv = s.registry.get(&s.business, &id);
    assert_eq!((inv.paid, inv.status), (500, InvoiceStatus::Paid));
    assert_eq!(s.token.balance(&s.business), 500);
}

#[test]
fn multiple_payers_can_settle_one_invoice() {
    let s = Setup::new();
    let id = s.register(1, 300);
    let a = s.payer(100);
    let b = s.payer(200);
    s.pay(&a, &id, 100);
    s.pay(&b, &id, 200);
    assert_eq!(s.registry.get(&s.business, &id).status, InvoiceStatus::Paid);
    assert_eq!(s.token.balance(&s.business), 300);
}

#[test]
fn pay_rejects_overpayment_in_one_go() {
    let s = Setup::new();
    let id = s.register(1, 500);
    let payer = s.payer(1_000);
    assert_eq!(
        s.try_pay(&payer, &payer, &s.business, &id, 501),
        Err(Some(Error::Overpayment))
    );
    assert_eq!(s.token.balance(&payer), 1_000);
    assert_eq!(s.registry.get(&s.business, &id).paid, 0);
}

#[test]
fn pay_rejects_overpayment_after_partial() {
    let s = Setup::new();
    let id = s.register(1, 500);
    let payer = s.payer(1_000);
    s.pay(&payer, &id, 400);
    assert_eq!(
        s.try_pay(&payer, &payer, &s.business, &id, 101),
        Err(Some(Error::Overpayment))
    );
    assert_eq!(s.registry.get(&s.business, &id).paid, 400);
}

#[test]
fn pay_rejects_paid_invoice() {
    let s = Setup::new();
    let id = s.register(1, 500);
    let payer = s.payer(1_000);
    s.pay(&payer, &id, 500);
    assert_eq!(
        s.try_pay(&payer, &payer, &s.business, &id, 1),
        Err(Some(Error::NotOpen))
    );
}

#[test]
fn pay_rejects_zero_amount() {
    let s = Setup::new();
    let id = s.register(1, 500);
    let payer = s.payer(1_000);
    assert_eq!(
        s.try_pay(&payer, &payer, &s.business, &id, 0),
        Err(Some(Error::InvalidAmount))
    );
}

#[test]
fn pay_rejects_negative_amount() {
    let s = Setup::new();
    let id = s.register(1, 500);
    let payer = s.payer(1_000);
    assert_eq!(
        s.try_pay(&payer, &payer, &s.business, &id, -1),
        Err(Some(Error::InvalidAmount))
    );
}

#[test]
fn pay_rejects_unknown_invoice() {
    let s = Setup::new();
    let payer = s.payer(1_000);
    assert_eq!(
        s.try_pay(&payer, &payer, &s.business, &s.hash(9), 1),
        Err(Some(Error::InvoiceNotFound))
    );
}

#[test]
fn pay_rejects_wrong_signer() {
    let s = Setup::new();
    let id = s.register(1, 500);
    let victim = s.payer(1_000);
    let attacker = Address::generate(&s.env);
    assert_eq!(
        s.try_pay(&attacker, &victim, &s.business, &id, 500),
        Err(None)
    );
    assert_eq!(s.token.balance(&victim), 1_000);
    assert_eq!(s.registry.get(&s.business, &id).paid, 0);
}

#[test]
fn pay_fails_atomically_with_insufficient_funds() {
    let s = Setup::new();
    let id = s.register(1, 500);
    let payer = s.payer(100);
    // The token's own InsufficientBalance (#10) is decoded with this
    // contract's Error enum, so don't assert on the exact variant.
    assert!(s.try_pay(&payer, &payer, &s.business, &id, 200).is_err());
    // State update is rolled back with the failed transfer.
    let inv = s.registry.get(&s.business, &id);
    assert_eq!((inv.paid, inv.status), (0, InvoiceStatus::Open));
}

#[test]
fn pay_rejects_overflowing_total() {
    let s = Setup::new();
    let id = s.register(1, i128::MAX);
    let payer = s.payer(i128::MAX);
    s.pay(&payer, &id, i128::MAX - 1);
    assert_eq!(
        s.try_pay(&payer, &payer, &s.business, &id, i128::MAX),
        Err(Some(Error::ArithmeticOverflow))
    );
}

// ---------------------------------------------------------------- void

#[test]
fn void_marks_void_and_emits_event() {
    let s = Setup::new();
    let id = s.register(1, 500);
    assert_eq!(s.try_void(&s.business, &s.business, &id), Ok(()));
    let expected = events::Voided {
        id: id.clone(),
        business: s.business.clone(),
    };
    assert_eq!(
        s.env
            .events()
            .all()
            .filter_by_contract(&s.registry_id)
            .events(),
        [expected.to_xdr(&s.env, &s.registry_id)]
    );
    assert_eq!(s.registry.get(&s.business, &id).status, InvoiceStatus::Void);
}

#[test]
fn void_blocks_payment() {
    let s = Setup::new();
    let id = s.register(1, 500);
    s.try_void(&s.business, &s.business, &id).unwrap();
    let payer = s.payer(1_000);
    assert_eq!(
        s.try_pay(&payer, &payer, &s.business, &id, 100),
        Err(Some(Error::NotOpen))
    );
    assert_eq!(s.token.balance(&payer), 1_000);
}

#[test]
fn void_rejects_after_partial_payment() {
    let s = Setup::new();
    let id = s.register(1, 500);
    let payer = s.payer(1_000);
    s.pay(&payer, &id, 1);
    assert_eq!(
        s.try_void(&s.business, &s.business, &id),
        Err(Some(Error::AlreadyPaid))
    );
    assert_eq!(s.registry.get(&s.business, &id).status, InvoiceStatus::Open);
}

#[test]
fn void_rejects_paid_invoice() {
    let s = Setup::new();
    let id = s.register(1, 500);
    let payer = s.payer(1_000);
    s.pay(&payer, &id, 500);
    assert_eq!(
        s.try_void(&s.business, &s.business, &id),
        Err(Some(Error::NotOpen))
    );
}

#[test]
fn void_rejects_already_void() {
    let s = Setup::new();
    let id = s.register(1, 500);
    s.try_void(&s.business, &s.business, &id).unwrap();
    assert_eq!(
        s.try_void(&s.business, &s.business, &id),
        Err(Some(Error::NotOpen))
    );
}

#[test]
fn void_cannot_reach_other_business_invoice() {
    let s = Setup::new();
    let id = s.register(1, 500);
    let other = Address::generate(&s.env);
    assert_eq!(
        s.try_void(&other, &other, &id),
        Err(Some(Error::InvoiceNotFound))
    );
    assert_eq!(s.registry.get(&s.business, &id).status, InvoiceStatus::Open);
}

#[test]
fn void_rejects_wrong_signer() {
    let s = Setup::new();
    let id = s.register(1, 500);
    let attacker = Address::generate(&s.env);
    assert_eq!(s.try_void(&attacker, &s.business, &id), Err(None));
    assert_eq!(s.registry.get(&s.business, &id).status, InvoiceStatus::Open);
}

#[test]
fn void_rejects_unknown_invoice() {
    let s = Setup::new();
    assert_eq!(
        s.try_void(&s.business, &s.business, &s.hash(4)),
        Err(Some(Error::InvoiceNotFound))
    );
}

// ---------------------------------------------------------------- close

#[test]
fn close_marks_closed_and_emits_event() {
    let s = Setup::new();
    let id = s.register(1, 500);
    let payer = s.payer(1_000);
    s.pay(&payer, &id, 200);
    assert_eq!(s.try_close(&s.business, &s.business, &id), Ok(()));
    let expected = events::Closed {
        id: id.clone(),
        business: s.business.clone(),
        paid: 200,
        outstanding: 300,
    };
    assert_eq!(
        s.env
            .events()
            .all()
            .filter_by_contract(&s.registry_id)
            .events(),
        [expected.to_xdr(&s.env, &s.registry_id)]
    );
    assert_eq!(
        s.registry.get(&s.business, &id).status,
        InvoiceStatus::Closed
    );
}

#[test]
fn close_blocks_payment() {
    let s = Setup::new();
    let id = s.register(1, 500);
    let payer = s.payer(1_000);
    s.pay(&payer, &id, 200);
    s.try_close(&s.business, &s.business, &id).unwrap();
    assert_eq!(
        s.try_pay(&payer, &payer, &s.business, &id, 100),
        Err(Some(Error::NotOpen))
    );
    assert_eq!(s.token.balance(&payer), 800);
}

#[test]
fn close_rejects_wrong_signer() {
    let s = Setup::new();
    let id = s.register(1, 500);
    let payer = s.payer(1_000);
    s.pay(&payer, &id, 200);
    let attacker = Address::generate(&s.env);
    assert_eq!(s.try_close(&attacker, &s.business, &id), Err(None));
    assert_eq!(s.registry.get(&s.business, &id).status, InvoiceStatus::Open);
}

#[test]
fn close_rejects_other_business() {
    let s = Setup::new();
    let id = s.register(1, 500);
    let payer = s.payer(1_000);
    s.pay(&payer, &id, 200);
    // Another business can only reach its own `(business, id)` key.
    let other = Address::generate(&s.env);
    assert_eq!(
        s.try_close(&other, &other, &id),
        Err(Some(Error::InvoiceNotFound))
    );
    assert_eq!(s.registry.get(&s.business, &id).status, InvoiceStatus::Open);
}

#[test]
fn close_rejects_unknown_invoice() {
    let s = Setup::new();
    assert_eq!(
        s.try_close(&s.business, &s.business, &s.hash(4)),
        Err(Some(Error::InvoiceNotFound))
    );
}

#[test]
fn close_rejects_paid_void_and_closed() {
    let s = Setup::new();
    let payer = s.payer(1_000);
    let paid = s.register(1, 100);
    s.pay(&payer, &paid, 100);
    let voided = s.register(2, 100);
    s.try_void(&s.business, &s.business, &voided).unwrap();
    let closed = s.register(3, 100);
    s.pay(&payer, &closed, 10);
    s.try_close(&s.business, &s.business, &closed).unwrap();
    for id in [paid, voided, closed] {
        assert_eq!(
            s.try_close(&s.business, &s.business, &id),
            Err(Some(Error::NotOpen))
        );
    }
}

#[test]
fn close_rejects_nothing_paid() {
    let s = Setup::new();
    let id = s.register(1, 500);
    assert_eq!(
        s.try_close(&s.business, &s.business, &id),
        Err(Some(Error::NothingPaid))
    );
    assert_eq!(s.registry.get(&s.business, &id).status, InvoiceStatus::Open);
}

#[test]
fn close_extends_invoice_ttl() {
    let s = Setup::new();
    let id = s.register(1, 500);
    let payer = s.payer(1_000);
    s.pay(&payer, &id, 100);
    s.pay(&payer, &id, 200);
    let step = 2 * finance_os_common::ttl::DAY_IN_LEDGERS;
    s.env.ledger().with_mut(|l| l.sequence_number += step);
    assert!(invoice_ttl(&s, &id) < finance_os_common::ttl::PERSISTENT_LIFETIME_THRESHOLD);
    s.try_close(&s.business, &s.business, &id).unwrap();
    assert!(invoice_ttl(&s, &id) >= finance_os_common::ttl::PERSISTENT_LIFETIME_THRESHOLD);
}

// ---------------------------------------------------------------- storage TTL

fn invoice_ttl(s: &Setup, id: &BytesN<32>) -> u32 {
    s.env.as_contract(&s.registry_id, || {
        s.env
            .storage()
            .persistent()
            .get_ttl(&crate::storage::DataKey::Invoice(
                s.business.clone(),
                id.clone(),
            ))
    })
}

#[test]
fn invoice_ttl_is_extended_on_write() {
    let s = Setup::new();
    let id = s.register(1, 500);
    assert!(invoice_ttl(&s, &id) >= finance_os_common::ttl::PERSISTENT_LIFETIME_THRESHOLD);
}

#[test]
fn invoice_ttl_is_extended_on_access() {
    let s = Setup::new();
    let id = s.register(1, 500);
    let start = invoice_ttl(&s, &id);
    let step = 2 * finance_os_common::ttl::DAY_IN_LEDGERS;
    s.env.ledger().with_mut(|l| l.sequence_number += step);
    assert!(invoice_ttl(&s, &id) < finance_os_common::ttl::PERSISTENT_LIFETIME_THRESHOLD);
    s.registry.get(&s.business, &id);
    assert_eq!(invoice_ttl(&s, &id), start);
}

// ---------------------------------------------------------------- edge cases

#[test]
fn invoices_are_independent() {
    let s = Setup::new();
    let a = s.register(1, 100);
    let b = s.register(2, 200);
    let payer = s.payer(1_000);
    s.pay(&payer, &a, 100);
    assert_eq!(s.registry.get(&s.business, &a).status, InvoiceStatus::Paid);
    let inv_b = s.registry.get(&s.business, &b);
    assert_eq!((inv_b.paid, inv_b.status), (0, InvoiceStatus::Open));
}

#[test]
fn voiding_one_invoice_leaves_others_payable() {
    let s = Setup::new();
    let a = s.register(1, 100);
    let b = s.register(2, 100);
    s.try_void(&s.business, &s.business, &a).unwrap();
    let payer = s.payer(100);
    s.pay(&payer, &b, 100);
    assert_eq!(s.registry.get(&s.business, &b).status, InvoiceStatus::Paid);
}

#[test]
fn same_content_hash_may_back_different_ids() {
    let s = Setup::new();
    // try_register always uses the same content hash.
    s.register(1, 100);
    s.register(2, 100);
    assert_eq!(
        s.registry.get(&s.business, &s.hash(1)).content_hash,
        s.registry.get(&s.business, &s.hash(2)).content_hash
    );
}

#[test]
fn business_can_pay_its_own_invoice() {
    let s = Setup::new();
    let id = s.register(1, 100);
    s.mint(&s.business, 100);
    s.pay(&s.business, &id, 100);
    assert_eq!(s.registry.get(&s.business, &id).status, InvoiceStatus::Paid);
    assert_eq!(s.token.balance(&s.business), 100);
}

#[test]
fn invoices_in_different_tokens_settle_in_their_own_token() {
    let s = Setup::new();
    let other_admin = Address::generate(&s.env);
    let other = s
        .env
        .register_stellar_asset_contract_v2(other_admin.clone());
    let other_token = TokenClient::new(&s.env, &other.address());
    let id = s.hash(5);
    s.try_register(&s.business, &s.business, &id, &other.address(), 40)
        .unwrap();

    let payer = Address::generate(&s.env);
    let args: soroban_sdk::Vec<Val> = (payer.clone(), 40i128).into_val(&s.env);
    auth(&s.env, &other_admin, &other.address(), "mint", args, &[]);
    StellarAssetClient::new(&s.env, &other.address()).mint(&payer, &40);

    let args = (payer.clone(), s.business.clone(), id.clone(), 40i128).into_val(&s.env);
    let sub = [MockAuthInvoke {
        contract: &other.address(),
        fn_name: "transfer",
        args: (payer.clone(), s.business.clone(), 40i128).into_val(&s.env),
        sub_invokes: &[],
    }];
    auth(&s.env, &payer, &s.registry_id, "pay", args, &sub);
    s.registry.pay(&payer, &s.business, &id, &40);

    assert_eq!(other_token.balance(&s.business), 40);
    assert_eq!(s.token.balance(&s.business), 0);
}

#[test]
fn get_returns_void_invoice() {
    let s = Setup::new();
    let id = s.register(1, 100);
    s.try_void(&s.business, &s.business, &id).unwrap();
    let inv = s.registry.get(&s.business, &id);
    assert_eq!(
        (inv.status, inv.paid, inv.amount),
        (InvoiceStatus::Void, 0, 100)
    );
}

#[test]
fn void_does_not_free_the_id_for_reuse() {
    let s = Setup::new();
    let id = s.register(1, 100);
    s.try_void(&s.business, &s.business, &id).unwrap();
    assert_eq!(
        s.try_register(&s.business, &s.business, &id, &s.token.address, 100),
        Err(Some(Error::InvoiceExists))
    );
}

// ---------------------------------------------------------------- id squatting (issue #3)

/// Mint to a payer and pay `(business, id)`, for tests with several businesses.
fn pay_to(s: &Setup, payer: &Address, business: &Address, id: &BytesN<32>, amount: i128) {
    s.try_pay(payer, payer, business, id, amount)
        .expect("pay ok");
}

#[test]
fn two_businesses_can_register_the_same_id_independently() {
    let s = Setup::new();
    let id = s.hash(1);
    let other = Address::generate(&s.env);
    s.try_register(&s.business, &s.business, &id, &s.token.address, 500)
        .unwrap();
    s.try_register(&other, &other, &id, &s.token.address, 70)
        .unwrap();
    let a = s.registry.get(&s.business, &id);
    let b = s.registry.get(&other, &id);
    assert_eq!((a.business, a.amount), (s.business.clone(), 500));
    assert_eq!((b.business, b.amount), (other.clone(), 70));
    // Voiding one leaves the other untouched.
    s.try_void(&other, &other, &id).unwrap();
    assert_eq!(s.registry.get(&s.business, &id).status, InvoiceStatus::Open);
    assert_eq!(s.registry.get(&other, &id).status, InvoiceStatus::Void);
}

#[test]
fn squatter_cannot_receive_payment_meant_for_real_business() {
    let s = Setup::new();
    let id = s.hash(1);
    let attacker = Address::generate(&s.env);
    // Attacker registers the id first, for a bigger amount.
    s.try_register(&attacker, &attacker, &id, &s.token.address, 500)
        .unwrap();
    // The real business can still register the same id.
    s.try_register(&s.business, &s.business, &id, &s.token.address, 500)
        .unwrap();

    let client = s.payer(500);
    pay_to(&s, &client, &s.business, &id, 500);

    assert_eq!(s.token.balance(&s.business), 500);
    assert_eq!(s.token.balance(&attacker), 0);
    assert_eq!(s.registry.get(&attacker, &id).paid, 0);
    assert_eq!(s.registry.get(&s.business, &id).status, InvoiceStatus::Paid);
}

#[test]
fn paying_a_business_that_never_registered_the_id_fails() {
    let s = Setup::new();
    let id = s.hash(1);
    let attacker = Address::generate(&s.env);
    // Only the attacker registered this id; the client pays the real business.
    s.try_register(&attacker, &attacker, &id, &s.token.address, 500)
        .unwrap();
    let client = s.payer(500);
    assert_eq!(
        s.try_pay(&client, &client, &s.business, &id, 500),
        Err(Some(Error::InvoiceNotFound))
    );
    assert_eq!(s.token.balance(&client), 500);
    assert_eq!(s.token.balance(&attacker), 0);
}

#[test]
fn register_duplicate_error_only_for_same_business() {
    let s = Setup::new();
    let id = s.register(1, 500);
    let other = Address::generate(&s.env);
    assert_eq!(
        s.try_register(&s.business, &s.business, &id, &s.token.address, 1),
        Err(Some(Error::InvoiceExists))
    );
    assert_eq!(
        s.try_register(&other, &other, &id, &s.token.address, 1),
        Ok(())
    );
}

#[test]
fn get_unknown_business_returns_not_found() {
    let s = Setup::new();
    let id = s.register(1, 500);
    let other = Address::generate(&s.env);
    assert_eq!(
        s.registry.try_get(&other, &id),
        Err(Ok(Error::InvoiceNotFound))
    );
}

#[test]
fn paid_and_voided_events_carry_business() {
    let s = Setup::new();
    let id = s.register(1, 500);
    let payer = s.payer(100);
    s.pay(&payer, &id, 100);
    let paid = events::Paid {
        id: id.clone(),
        business: s.business.clone(),
        payer,
        amount: 100,
        total_paid: 100,
    };
    assert_eq!(
        s.env
            .events()
            .all()
            .filter_by_contract(&s.registry_id)
            .events(),
        [paid.to_xdr(&s.env, &s.registry_id)]
    );
    let id2 = s.register(2, 500);
    s.try_void(&s.business, &s.business, &id2).unwrap();
    let voided = events::Voided {
        id: id2,
        business: s.business.clone(),
    };
    assert_eq!(
        s.env
            .events()
            .all()
            .filter_by_contract(&s.registry_id)
            .events(),
        [voided.to_xdr(&s.env, &s.registry_id)]
    );
}

// ---------------------------------------------------------------- recorded auth trees

fn contract_call(
    env: &Env,
    contract: &Address,
    fn_name: &str,
    args: soroban_sdk::Vec<Val>,
    subs: std::vec::Vec<soroban_sdk::testutils::AuthorizedInvocation>,
) -> soroban_sdk::testutils::AuthorizedInvocation {
    soroban_sdk::testutils::AuthorizedInvocation {
        function: soroban_sdk::testutils::AuthorizedFunction::Contract((
            contract.clone(),
            soroban_sdk::Symbol::new(env, fn_name),
            args,
        )),
        sub_invocations: subs,
    }
}

#[test]
fn pay_requires_payer_auth_for_pay_and_nested_transfer() {
    let s = Setup::new();
    let id = s.register(1, 100);
    let payer = s.payer(100);
    s.pay(&payer, &id, 60);
    let pay_args = (payer.clone(), s.business.clone(), id, 60i128).into_val(&s.env);
    let transfer_args = (payer.clone(), s.business.clone(), 60i128).into_val(&s.env);
    assert_eq!(
        s.env.auths(),
        std::vec![(
            payer,
            contract_call(
                &s.env,
                &s.registry_id,
                "pay",
                pay_args,
                std::vec![contract_call(
                    &s.env,
                    &s.token.address,
                    "transfer",
                    transfer_args,
                    std::vec![]
                )]
            )
        )]
    );
}

#[test]
fn register_requires_exactly_the_business_auth() {
    let s = Setup::new();
    let id = s.register(1, 100);
    let args = (
        s.business.clone(),
        id,
        s.token.address.clone(),
        100i128,
        s.hash(0xC0),
    )
        .into_val(&s.env);
    assert_eq!(
        s.env.auths(),
        std::vec![(
            s.business.clone(),
            contract_call(&s.env, &s.registry_id, "register", args, std::vec![])
        )]
    );
}

// ---------------------------------------------------------------- docs/events.md

#[test]
fn event_topic_xdr_matches_docs() {
    use soroban_sdk::xdr::{ContractEventBody, Limits, ScVal, WriteXdr};
    let env = Env::default();
    let contract = Address::generate(&env);
    let a = Address::generate(&env);
    let id = BytesN::from_array(&env, &[1; 32]);
    let cases: std::vec::Vec<(soroban_sdk::xdr::ContractEvent, &str)> = std::vec![
        (
            events::Registered {
                id: id.clone(),
                business: a.clone(),
                token: a.clone(),
                amount: 1,
                content_hash: id.clone()
            }
            .to_xdr(&env, &contract),
            "AAAADwAAAApyZWdpc3RlcmVkAAA=",
        ),
        (
            events::Paid {
                id: id.clone(),
                business: a.clone(),
                payer: a.clone(),
                amount: 1,
                total_paid: 1
            }
            .to_xdr(&env, &contract),
            "AAAADwAAAARwYWlk",
        ),
        (
            events::Voided {
                id: id.clone(),
                business: a.clone()
            }
            .to_xdr(&env, &contract),
            "AAAADwAAAAZ2b2lkZWQAAA=="
        ),
        (
            events::Closed {
                id: id.clone(),
                business: a.clone(),
                paid: 1,
                outstanding: 0
            }
            .to_xdr(&env, &contract),
            "AAAADwAAAAZjbG9zZWQAAA=="
        ),
    ];
    for (ev, expected) in cases {
        let ContractEventBody::V0(body) = ev.body;
        assert_eq!(
            body.topics[0].to_xdr_base64(Limits::none()).unwrap(),
            expected
        );
        assert!(matches!(body.topics[1], ScVal::Bytes(ref b) if b.len() == 32));
        let ScVal::Map(Some(map)) = body.data else {
            panic!("data is not a map")
        };
        let keys: std::vec::Vec<_> = map.iter().map(|e| e.key.clone()).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
    }
}
