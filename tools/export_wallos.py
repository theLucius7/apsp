#!/usr/bin/env python3
"""Convert Wallos subscription configuration to the native apsp expense format."""
import argparse
from contextlib import closing
from decimal import Decimal
from migration_snapshot import connect, rows, write_private


def units(value):
    number = Decimal(str(value)) * 10000
    if not number.is_finite() or number != number.to_integral_value() or not 0 <= number <= 9000000000000000:
        raise ValueError("Invalid amount or precision greater than four decimal places")
    return str(int(number))


def snapshot(connection, source_id, user_id, timezone):
    owners = rows(connection, "user", ["id", "main_currency", "budget"])
    owner = next((u for u in owners if u["id"] == user_id), None)
    if owner is None:
        raise ValueError("Wallos account does not exist")
    currencies = {c["id"]: c["code"].upper() for c in rows(connection, "currencies", ["id", "code", "user_id"]) if c["user_id"] == user_id}
    labels, mappings = [], {}
    for table, kind in [("categories", "category"), ("payment_methods", "payment_method"), ("household", "payer")]:
        columns = ["id", "name", "user_id"] + (["enabled"] if table == "payment_methods" else [])
        mappings[kind] = {}
        for row in rows(connection, table, columns):
            if row["user_id"] != user_id:
                continue
            identifier = len(labels) + 1
            mappings[kind][row["id"]] = identifier
            labels.append({"id": identifier, "kind": kind, "name": row["name"], "enabled": bool(row.get("enabled", True))})
    columns = "id user_id name price currency_id start_date next_payment cycle frequency category_id payment_method_id payer_user_id notes url inactive auto_renew notify notify_days_before cancellation_date replacement_subscription_id".split()
    subscriptions = []
    for row in rows(connection, "subscriptions", columns):
        if row["user_id"] != user_id:
            continue
        def mapped(kind, key):
            value = row[key]
            if not value:
                return None
            if value not in mappings[kind]:
                raise ValueError(f"Unmapped {kind} for subscription {row['id']}")
            return mappings[kind][value]
        subscriptions.append({"id": row["id"], "anchor_payment": row["next_payment"], "configuration": {
            "name": row["name"], "amount_units": units(row["price"]), "currency": currencies[row["currency_id"]],
            "start_date": row["start_date"] or None, "next_payment": row["next_payment"],
            "cycle": {1: "day", 2: "week", 3: "month", 4: "year"}[row["cycle"]], "frequency": row["frequency"],
            "category_id": mapped("category", "category_id"), "payment_method_id": mapped("payment_method", "payment_method_id"), "payer_id": mapped("payer", "payer_user_id"),
            "notes": row["notes"] or "", "url": row["url"] or "", "inactive": bool(row["inactive"]),
            "auto_renew": bool(row["auto_renew"]), "notify": bool(row["notify"]), "notify_days_before": row["notify_days_before"] or 0,
            "cancellation_date": row["cancellation_date"] or None, "replacement_id": row["replacement_subscription_id"] or None, "version": None}})
    return {"format": "apsp-expenses-v1", "source_id": source_id, "base_currency": currencies[owner["main_currency"]],
            "budget_units": units(owner["budget"]) if owner["budget"] else None, "timezone": timezone, "labels": labels, "subscriptions": subscriptions}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("database")
    parser.add_argument("output")
    parser.add_argument("--source-id", required=True)
    parser.add_argument("--user-id", type=int, required=True)
    parser.add_argument("--timezone", required=True)
    args = parser.parse_args()
    with closing(connect(args.database)) as connection:
        value = snapshot(connection, args.source_id, args.user_id, args.timezone)
    write_private(args.output, value)
    print(f"Exported {len(value['subscriptions'])} subscriptions and {len(value['labels'])} labels. Keep the output private.")


if __name__ == "__main__":
    main()
