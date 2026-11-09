//! Property tests: invariants that must hold for any payment sequence.
#![allow(clippy::arithmetic_side_effects, clippy::unwrap_used, clippy::panic)]

extern crate std;

use proptest::prelude::*;
use soroban_sdk::{
    testutils::{Address as _, MockAuth, MockAuthInvoke},
    token::{StellarAssetClient, TokenClient},
    Address, BytesN, Env, IntoVal,
};

use crate::{types::InvoiceStatus, InvoiceRegistry, InvoiceRegistryClient};

fn mock(
    env: &Env,
    signer: &Address,
    contract: &Address,
    f: &str,
    args: soroban_sdk::Vec<soroban_sdk::Val>,
    subs: &[MockAuthInvoke],
) {
    env.mock_auths(&[MockAuth {
        address: signer,
        invoke: &MockAuthInvoke {
            contract,
            fn_name: f,
            args,
            sub_invokes: subs,
        },
    }]);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    /// For any invoice amount and any sequence of attempted payments:
    /// `paid` never exceeds `amount`, the business receives exactly `paid`,
    /// and the status is Paid iff `paid == amount`.
    #[test]
    fn paid_never_exceeds_amount(
        amount in 1i128..1_000,
        attempts in proptest::collection::vec(-5i128..400, 1..12),
    ) {
        let env = Env::default();
        let registry_id = env.register(InvoiceRegistry, ());
        let registry = InvoiceRegistryClient::new(&env, &registry_id);
        let admin = Address::generate(&env);
        let sac = env.register_stellar_asset_contract_v2(admin.clone());
        let token = TokenClient::new(&env, &sac.address());
        let business = Address::generate(&env);
        let payer = Address::generate(&env);

        let mint_args = (payer.clone(), 10_000i128).into_val(&env);
        mock(&env, &admin, &token.address, "mint", mint_args, &[]);
        StellarAssetClient::new(&env, &token.address).mint(&payer, &10_000);

        let id = BytesN::from_array(&env, &[1; 32]);
        let content = BytesN::from_array(&env, &[2; 32]);
        let reg_args = (business.clone(), id.clone(), token.address.clone(), amount, content.clone()).into_val(&env);
        mock(&env, &business, &registry_id, "register", reg_args, &[]);
        registry.register(&business, &id, &token.address, &amount, &content);

        for a in attempts {
            let args = (payer.clone(), business.clone(), id.clone(), a).into_val(&env);
            let sub = [MockAuthInvoke {
                contract: &token.address,
                fn_name: "transfer",
                args: (payer.clone(), business.clone(), a).into_val(&env),
                sub_invokes: &[],
            }];
            mock(&env, &payer, &registry_id, "pay", args, &sub);
            let _ = registry.try_pay(&payer, &business, &id, &a);

            let inv = registry.get(&business, &id);
            prop_assert!(inv.paid >= 0 && inv.paid <= inv.amount);
            prop_assert_eq!(token.balance(&business), inv.paid);
            prop_assert_eq!(inv.status == InvoiceStatus::Paid, inv.paid == inv.amount);
        }
    }
    /// After `close`, no payment is accepted and `paid` stays frozen.
    #[test]
    fn no_payment_accepted_after_close(
        amount in 2i128..1_000,
        first in 1i128..400,
        attempts in proptest::collection::vec(-5i128..400, 1..8),
    ) {
        let first = first.min(amount - 1);

    /// For any two businesses sharing one invoice id and any payment
    /// sequence aimed at `honest`: only `honest` is ever credited, and the
    /// other business's invoice and balance never change.
    #[test]
    fn payments_only_credit_the_named_business(
        amount_honest in 1i128..1_000,
        amount_squat in 1i128..1_000,
        attempts in proptest::collection::vec(-5i128..400, 1..12),
        squat_first in any::<bool>(),
    ) {
        let env = Env::default();
        let registry_id = env.register(InvoiceRegistry, ());
        let registry = InvoiceRegistryClient::new(&env, &registry_id);
        let admin = Address::generate(&env);
        let sac = env.register_stellar_asset_contract_v2(admin.clone());
        let token = TokenClient::new(&env, &sac.address());
        let business = Address::generate(&env);
        let payer = Address::generate(&env);
        StellarAssetClient::new(&env, &token.address).mock_all_auths().mint(&payer, &10_000);

        let id = BytesN::from_array(&env, &[1; 32]);
        let content = BytesN::from_array(&env, &[2; 32]);
        let reg_args = (business.clone(), id.clone(), token.address.clone(), amount, content.clone()).into_val(&env);
        mock(&env, &business, &registry_id, "register", reg_args, &[]);
        registry.register(&business, &id, &token.address, &amount, &content);

        let pay = |a: i128| {
            let args = (payer.clone(), id.clone(), a).into_val(&env);
            let sub = [MockAuthInvoke {
                contract: &token.address,
                fn_name: "transfer",
                args: (payer.clone(), business.clone(), a).into_val(&env),
                sub_invokes: &[],
            }];
            mock(&env, &payer, &registry_id, "pay", args, &sub);
            registry.try_pay(&payer, &id, &a)
        };
        pay(first).unwrap().unwrap();
        let close_args = (business.clone(), id.clone()).into_val(&env);
        mock(&env, &business, &registry_id, "close", close_args, &[]);
        registry.close(&business, &id);

        for a in attempts {
            prop_assert!(pay(a).is_err() || pay(a).unwrap().is_err());
            let inv = registry.get(&id);
            prop_assert_eq!(inv.paid, first);
            prop_assert_eq!(inv.status, InvoiceStatus::Closed);
            prop_assert_eq!(token.balance(&business), first);
        let honest = Address::generate(&env);
        let squatter = Address::generate(&env);
        let payer = Address::generate(&env);

        let mint_args = (payer.clone(), 10_000i128).into_val(&env);
        mock(&env, &admin, &token.address, "mint", mint_args, &[]);
        StellarAssetClient::new(&env, &token.address).mint(&payer, &10_000);

        let id = BytesN::from_array(&env, &[1; 32]);
        let content = BytesN::from_array(&env, &[2; 32]);
        let register = |who: &Address, amt: i128| {
            let args = (who.clone(), id.clone(), token.address.clone(), amt, content.clone()).into_val(&env);
            mock(&env, who, &registry_id, "register", args, &[]);
            registry.register(who, &id, &token.address, &amt, &content);
        };
        if squat_first {
            register(&squatter, amount_squat);
            register(&honest, amount_honest);
        } else {
            register(&honest, amount_honest);
            register(&squatter, amount_squat);
        }

        for a in attempts {
            let args = (payer.clone(), honest.clone(), id.clone(), a).into_val(&env);
            let sub = [MockAuthInvoke {
                contract: &token.address,
                fn_name: "transfer",
                args: (payer.clone(), honest.clone(), a).into_val(&env),
                sub_invokes: &[],
            }];
            mock(&env, &payer, &registry_id, "pay", args, &sub);
            let _ = registry.try_pay(&payer, &honest, &id, &a);

            let h = registry.get(&honest, &id);
            let sq = registry.get(&squatter, &id);
            prop_assert_eq!(token.balance(&honest), h.paid);
            prop_assert_eq!(token.balance(&squatter), 0);
            prop_assert_eq!(sq.paid, 0);
            prop_assert_eq!(sq.status, InvoiceStatus::Open);
        }
    }

    /// After `close`, no payment is accepted and `paid` is frozen.
    #[test]
    fn no_payment_accepted_after_close(
        amount in 2i128..1_000,
        first in 1i128..1_000,
        later in proptest::collection::vec(-5i128..400, 1..8),
    ) {
        let first = first % (amount - 1) + 1; // 1..amount-1: partial
        let env = Env::default();
        let registry_id = env.register(InvoiceRegistry, ());
        let registry = InvoiceRegistryClient::new(&env, &registry_id);
        let admin = Address::generate(&env);
        let sac = env.register_stellar_asset_contract_v2(admin.clone());
        let token = TokenClient::new(&env, &sac.address());
        let business = Address::generate(&env);
        let payer = Address::generate(&env);

        let mint_args = (payer.clone(), 10_000i128).into_val(&env);
        mock(&env, &admin, &token.address, "mint", mint_args, &[]);
        StellarAssetClient::new(&env, &token.address).mint(&payer, &10_000);

        let id = BytesN::from_array(&env, &[1; 32]);
        let content = BytesN::from_array(&env, &[2; 32]);
        let reg_args = (business.clone(), id.clone(), token.address.clone(), amount, content.clone()).into_val(&env);
        mock(&env, &business, &registry_id, "register", reg_args, &[]);
        registry.register(&business, &id, &token.address, &amount, &content);

        let pay = |a: i128| {
            let args = (payer.clone(), id.clone(), a).into_val(&env);
            let sub = [MockAuthInvoke {
                contract: &token.address,
                fn_name: "transfer",
                args: (payer.clone(), business.clone(), a).into_val(&env),
                sub_invokes: &[],
            }];
            mock(&env, &payer, &registry_id, "pay", args, &sub);
            registry.try_pay(&payer, &id, &a)
        };
        prop_assert!(pay(first).is_ok());

        let close_args = (business.clone(), id.clone()).into_val(&env);
        mock(&env, &business, &registry_id, "close", close_args, &[]);
        registry.close(&business, &id);

        for a in later {
            prop_assert!(pay(a).is_err());
            let inv = registry.get(&id);
            prop_assert_eq!(inv.status, InvoiceStatus::Closed);
            prop_assert_eq!(inv.paid, first);
            prop_assert_eq!(token.balance(&business), first);
        }
    }
}
