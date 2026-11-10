# Events: guide for the backend indexer

The backend indexer mirrors contract state by reading events from Stellar RPC
`getEvents`. This page describes exactly what each event looks like on the wire.
The README tables (§3, §4) are the canonical list. **Any change here is a
breaking change and needs a major version bump.**

## Shape

Every event is a Soroban contract event with:

- **topics** (2 entries): `[Symbol(<event name>), <id>]`
  - vault: `<id>` is `U64(proposal_id)`
  - registry: `<id>` is `Bytes(32)` (the invoice id hash)
- **data**: an `ScMap` keyed by `Symbol(<field name>)`, holding the remaining
  fields in alphabetical key order. Events with no data fields (`executed`)
  carry an empty map.

## Topic filters

`getEvents` topic filters take base64 XDR `ScVal`s. Use `"*"` for the id
position to match all ids.

| Contract | Event | Topic 0 (base64 XDR) |
|---|---|---|
| approval-vault | `proposed` | `AAAADwAAAAhwcm9wb3NlZA==` |
| approval-vault | `policy_proposed` | `AAAADwAAAA9wb2xpY3lfcHJvcG9zZWQA` |
| approval-vault | `approved` | `AAAADwAAAAhhcHByb3ZlZA==` |
| approval-vault | `executed` | `AAAADwAAAAhleGVjdXRlZA==` |
| approval-vault | `cancelled` | `AAAADwAAAAljYW5jZWxsZWQAAAA=` |
| approval-vault | `policy_updated` | `AAAADwAAAA5wb2xpY3lfdXBkYXRlZAAA` |
| invoice-registry | `registered` | `AAAADwAAAApyZWdpc3RlcmVkAAA=` |
| invoice-registry | `paid` | `AAAADwAAAARwYWlk` |
| invoice-registry | `voided` | `AAAADwAAAAZ2b2lkZWQAAA==` |
| invoice-registry | `closed` | `AAAADwAAAAZjbG9zZWQAAA==` |

These values are checked by tests (`event_topic_xdr_matches_docs` in each
contract), so this table can't silently drift from the code.

### Example: all `paid` events from the registry

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "getEvents",
  "params": {
    "startLedger": 123456,
    "filters": [
      {
        "type": "contract",
        "contractIds": ["<invoiceRegistry from deployments/testnet.json>"],
        "topics": [["AAAADwAAAARwYWlk", "*"]]
      }
    ]
  }
}
```

## Data fields

### approval-vault

| Event | Fields (ScVal type) |
|---|---|
| `proposed` | `amount` (I128), `proposer` (Address), `to` (Address), `token` (Address) |
| `policy_proposed` | `approvers` (Vec\<Address\>), `proposer` (Address), `threshold` (U32) |
| `approved` | `approvals` (U32), `approver` (Address) |
| `executed` | (none) |
| `cancelled` | `cancelled_by` (Address) |
| `policy_updated` | `approvers` (Vec\<Address\>), `threshold` (U32) |

### invoice-registry

| Event | Fields (ScVal type) |
|---|---|
| `registered` | `amount` (I128), `business` (Address), `payee` (Address), `token` (Address) |
| `paid` | `amount` (I128), `business` (Address), `payer` (Address), `total_paid` (I128) |
| `voided` | `business` (Address) |
| `closed` | `business` (Address), `outstanding` (I128), `paid` (I128) |

## Mirroring rules

**Vault proposals**

- `proposed` / `policy_proposed`: create the record (Pending, 1 approval).
- `approved`: set the approval count to `approvals`. It's the authoritative count *after* stale approvers were pruned, so don't increment a local counter.
- `executed`: mark Executed.
- `cancelled`: mark Cancelled. Look up the id to tell whether it was a payment or a policy proposal: ids are unique across both kinds.
- `policy_updated`: replace the cached approver set and threshold.

**Invoices**

- An invoice is identified by `(business, id)`; the same `id` can exist under several businesses. Key your records on both.
- `registered`: create the record (Open) and store its `payee`: every payment for this invoice lands there (may differ from `business`, e.g. an approval vault).
- `paid`: look up `(business, id)` and set `paid = total_paid`. Mark Paid when `total_paid` equals the registered `amount`. The funds went to the stored `payee`.
- `voided`: look up `(business, id)` and mark Void.
- `closed`: look up `(business, id)` and mark Closed (terminal). `paid` is final and `outstanding == amount - paid`; no further `paid` events will occur.

**Trust**

- Only act on events whose contract id matches `deployments/<network>.json`.
- Only treat `paid` as a real payment when the invoice's `token` is on your
  allow-list (e.g. the USDC SAC). See the threat model, R8.

**Expiry has no event.** A Pending proposal whose `expires_at` has passed is
effectively dead. Derive that from `expires_at` and the ledger close time.
