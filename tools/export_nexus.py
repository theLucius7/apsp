#!/usr/bin/env python3
"""Export Nexus's deployed private-member business without admin or payment secrets."""
import argparse
import json
import time
from decimal import Decimal
from contextlib import closing
from migration_snapshot import connect, rows, write_private

PERIODS = {
    "monthly": ("月付", 1, 31), "quarterly": ("季付", 3, 93),
    "half_yearly": ("半年付", 6, 186), "yearly": ("年付", 12, 366),
    "two_yearly": ("两年付", 24, 732), "three_yearly": ("三年付", 36, 1098),
}
HISTORY = {
    "v2_order": ("order", "user_id", "id user_id payer_user_id plan_id period total_amount handling_amount discount_amount surplus_amount surplus_credit balance_amount status paid_at created_at"),
    "v2_wallet_ledger": ("wallet", "user_id", "id user_id action source_type source_id currency balance_before delta balance_after reversal_of created_at"),
    "v2_invite_code": ("invitation", "user_id", "id user_id status used_by_user_id used_at revoked_at purchase_price_cents purchased_at prepaid_plan_id prepaid_period prepaid_plan_amount prepaid_order_id prepaid_refunded_at wallet_ledger_id created_at"),
    "v2_nexus_recharge": ("payment", "user_id", "id user_id currency amount_cents fee_cents status created_at expires_at paid_at wallet_ledger_id"),
    "v2_stat_user": ("traffic", "user_id", "id user_id server_rate u d record_type record_at created_at"),
}


def snapshot(connection, source_id, include_password_hashes=False):
    members = []
    fields = "id email balance banned uuid plan_id invite_user_id transfer_enable u d expired_at next_reset_at token nexus_sponsor_required nexus_first_purchase_at member_status last_reset_at remind_expire remind_traffic".split()
    if include_password_hashes:
        fields.append("password")
    for row in rows(connection, "v2_user", fields):
        state = "archived" if row["member_status"] == "ARCHIVED" else "suspended" if row["banned"] else "pending" if row["nexus_sponsor_required"] and not row["nexus_first_purchase_at"] else "active"
        members.append({"id": row["id"], "name": row["email"].split("@")[0], "email": row["email"],
                        "balance_cents": row["balance"], "state": state,
                        "inviter_id": row["invite_user_id"] or None, "sponsor_required": bool(row["nexus_sponsor_required"]),
                        "first_purchase_at": row["nexus_first_purchase_at"], "subscription_token": row["token"],
                        "legacy_uuid": row["uuid"], "plan_id": row["plan_id"] or None,
                        "quota_bytes": str(row["transfer_enable"]) if row["transfer_enable"] is not None else None,
                        "used_upload": str(row["u"]), "used_download": str(row["d"]),
                        "expires_at": row["expired_at"], "reset_at": row["next_reset_at"], "last_reset_at": row["last_reset_at"],
                        "legacy_password_hash": row.get("password"), "remind_expire": bool(row["remind_expire"]), "remind_traffic": bool(row["remind_traffic"])})
    plans = []
    for row in rows(connection, "v2_plan", ["id", "name", "transfer_enable", "prices", "group_id"]):
        prices = []
        for period, cents in json.loads(row["prices"] or "{}").items():
            if cents is None or cents == 0:
                continue
            if period not in PERIODS:
                raise ValueError(f"Explicit mapping required for source plan period: {period}")
            label, months, days = PERIODS[period]
            # The deployed Nexus Plan JSON stores major-unit prices, including strings.
            amount = Decimal(str(cents)) * 100
            if not amount.is_finite() or amount != amount.to_integral_value() or not 0 < amount <= 100000000:
                raise ValueError("Plan price is outside supported minor-unit precision")
            cents = int(amount)
            prices.append({"label": label, "cents": cents, "months": months, "days": days})
        plans.append({"id": row["id"], "name": row["name"], "quota_bytes": str(row["transfer_enable"] * 1024**3) if row["transfer_enable"] else None, "prices": prices, "legacy_group_id": row["group_id"]})
    history = []
    for table, (kind, owner, columns) in HISTORY.items():
        for row in rows(connection, table, columns.split(), required=False):
            history.append({"kind": kind, "id": f"{table}:{row['id']}", "member_id": row[owner], "record": row})
    # Reset and payment evidence uses a bounded, version-aware allowlist.
    payment_owners = {}
    for table, kind in [("v2_traffic_reset_logs", "reset"), ("v2_payment_intent", "payment"), ("v2_payment_transaction", "payment"), ("v2_payment_exception", "payment")]:
        available = {r[1] for r in connection.execute(f'PRAGMA table_info("{table}")')}
        wanted = "id user_id purpose status state amount_cents fee_cents amount currency business_type business_id order_id recharge_id intent_id payer_user_id beneficiary_user_id provider payment_id wallet_cents settled_at expires_at created_at updated_at paid_at reset_at reset_time old_u old_d new_u new_d old_upload old_download old_total new_upload new_download new_total reset_type trigger_source".split()
        columns = [c for c in wanted if c in available]
        if "id" not in columns:
            continue
        for row in rows(connection, table, columns):
            owner = row.get("user_id") or row.get("payer_user_id") or payment_owners.get(row.get("intent_id"))
            if table == "v2_payment_intent":
                payment_owners[row["id"]] = owner
            history.append({"kind": kind, "id": f"{table}:{row['id']}", "member_id": owner, "record": row})
    # Retain evidence for deleted legacy members without fabricating live accounts.
    # The original member ID stays in record.user_id; only the live FK is absent.
    live_ids = {member["id"] for member in members}
    for record in history:
        if record["member_id"] not in live_ids:
            record["member_id"] = None
    content = []
    for row in rows(connection, "v2_knowledge", "id category title body show".split(), required=False):
        content.append({"id": row["id"], "kind": "guide", "category": row["category"] or "", "title": row["title"], "body": row["body"], "published": bool(row["show"])})
    for row in rows(connection, "v2_notice", "id title content show".split(), required=False):
        content.append({"id": row["id"], "kind": "notice", "category": "", "title": row["title"], "body": row["content"], "published": bool(row["show"])})
    return {"format": "apsp.nexus.v1", "source_id": source_id, "exported_at": int(time.time()), "members": members, "plans": plans, "history": history, "content": content}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("database")
    parser.add_argument("output")
    parser.add_argument("--source-id", required=True)
    parser.add_argument("--include-password-hashes", action="store_true", help="Preserve existing member login during an authorized live migration")
    args = parser.parse_args()
    with closing(connect(args.database)) as connection:
        value = snapshot(connection, args.source_id, args.include_password_hashes)
    write_private(args.output, value)
    print(f"Exported {len(value['members'])} members, {len(value['plans'])} plans, {len(value['history'])} historical records. Keep the output private.")


if __name__ == "__main__":
    main()
